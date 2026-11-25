//! The background lane of the source. One thread reads the files, the keychain and the
//! as-played log, so the UI thread never waits on them. The snapshot takes the results.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use jiff::Timestamp;
use radio_engine::{AsPlayed, LogHandle};
use radio_library::{Limits, Listing};
use radio_model::Item;
use radio_ui_egui::view::HistoryReport;

use crate::SecretStore;
use crate::edit::file_item;

/// Work for the background lane.
pub(crate) enum Job {
    Reconcile(Arc<radio_engine::State>),
    /// List and probe a library folder.
    Scan {
        path: String,
        limits: Limits,
    },
    /// Probe files and folders (a folder brings its subfolders) for request `req`.
    Probe {
        req: u64,
        paths: Vec<String>,
        recursive: bool,
        limits: Limits,
    },
    /// Read, or write when `set` has a value, the keychain entry `key`.
    Secret {
        key: String,
        set: Option<String>,
        store: Arc<dyn SecretStore + Sync>,
    },
    /// Remove the keychain entry `key`.
    Forget {
        key: String,
        store: Arc<dyn SecretStore + Sync>,
    },
    /// Fetch the pairing document of the server at `address`, as try `seq`, with `key` or the
    /// key stored under `paired_entry`. It runs on a thread of its own, so a slow server
    /// never holds up the other jobs.
    Pair {
        seq: u64,
        address: String,
        key: Option<radio_ui_egui::view::Secret>,
        paired_entry: Option<String>,
        store: Arc<dyn SecretStore + Sync>,
    },
    /// Send the whole list of outputs to the server at `address`, as save `seq`, with the
    /// key stored under `entry`, the entry of that address. It runs on a thread of its own,
    /// as a pairing try does.
    PutOutputs {
        seq: u64,
        address: String,
        entry: String,
        outputs: Vec<radio_model::OutputSpec>,
        store: Arc<dyn SecretStore + Sync>,
    },
    /// Build the Icecast settings for playout from `state` and the keychain, as request `seq`.
    Icecast {
        seq: u64,
        state: Arc<radio_engine::State>,
        store: Arc<dyn SecretStore + Sync>,
        station_name: String,
    },
    /// Read the as-played log.
    History {
        log: LogHandle,
        since: Timestamp,
        until: Timestamp,
        limit: u32,
        /// The range that a person picked, which the answer carries back.
        range: Option<(Timestamp, Timestamp)>,
    },
    /// Write a report of the as-played log as CSV to the file at `path`.
    ExportHistory {
        log: LogHandle,
        path: String,
        report: HistoryReport,
        since: Timestamp,
        until: Timestamp,
        playlist: Option<String>,
        /// The time zone of the station, for the local times of a report.
        zone: String,
    },
    /// Read the cover in the tags of the file at `path`.
    Cover {
        path: String,
    },
    /// Read the station logo at `path`. `modified` is the file's time when it was asked for.
    Logo {
        path: std::path::PathBuf,
        modified: Option<std::time::SystemTime>,
    },
    /// Write alarms as CSV to the file at `path`.
    ExportAlarms {
        path: String,
        rows: Vec<radio_ui_egui::view::AlarmView>,
    },
}

/// A result of the background lane.
pub(crate) enum Done {
    Reconciled {
        commands: Vec<radio_engine::Command>,
        lines: Vec<String>,
    },
    Scan {
        path: String,
        listing: Listing,
        items: Vec<Item>,
        unreadable: usize,
    },
    Probed {
        req: u64,
        items: Vec<Item>,
        skipped: usize,
        errors: Vec<String>,
    },
    /// The keychain entry `key` holds a password, or the keychain failed.
    Secret {
        key: String,
        wrote: bool,
        result: Result<bool, String>,
    },
    /// The keychain entry `key` is gone, or the keychain failed.
    Forgot { result: Result<(), String> },
    /// The answer of a pairing try.
    Pair(crate::pairing::PairAnswer),
    /// The answer of a save of the outputs.
    PutOutputs(crate::pairing::PutAnswer),
    /// The Icecast settings of request `seq`, or why there are none.
    Icecast {
        seq: u64,
        result: Result<Option<radio_stream::IcecastConfig>, String>,
    },
    /// The rows, the row limit of the read, and the picked range that the read was for.
    History(
        Result<Vec<AsPlayed>, String>,
        u32,
        Option<(Timestamp, Timestamp)>,
    ),
    /// The cover of the file at `path`, or `None` when it has none.
    Cover {
        path: String,
        bytes: Option<Arc<[u8]>>,
    },
    /// The station logo at `path`, as it was when its time was `modified`.
    Logo {
        path: std::path::PathBuf,
        modified: Option<std::time::SystemTime>,
        result: Result<radio_ui_egui::view::Cover, String>,
    },
    /// The export wrote `count` rows of `what` to `path`, or failed.
    Exported {
        path: String,
        what: &'static str,
        count: usize,
        result: Result<(), String>,
    },
}

/// The background lane: one thread that runs the jobs in order.
pub(crate) struct Worker {
    pub(crate) jobs: Sender<Job>,
    pub(crate) done: Receiver<Done>,
}

impl Worker {
    pub(crate) fn start() -> Option<Self> {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        std::thread::Builder::new()
            .name("ui-background".into())
            .spawn(move || {
                for job in rx {
                    if let Job::Pair {
                        seq,
                        address,
                        key,
                        paired_entry,
                        store,
                    } = job
                    {
                        let reply = tx.clone();
                        let asked = address.clone();
                        let spawned = std::thread::Builder::new().name("ui-pairing".into()).spawn(
                            move || {
                                let answer =
                                    crate::pairing::run(seq, address, key, paired_entry, &store);
                                let _ = reply.send(Done::Pair(answer));
                            },
                        );
                        if spawned.is_err() {
                            let answer = crate::pairing::PairAnswer {
                                seq,
                                address: asked,
                                key: None,
                                typed: false,
                                result: Err(crate::pairing::PairError::Refused(
                                    "the app could not start the request".into(),
                                )),
                            };
                            if tx.send(Done::Pair(answer)).is_err() {
                                break;
                            }
                        }
                        continue;
                    }
                    if let Job::PutOutputs {
                        seq,
                        address,
                        entry,
                        outputs,
                        store,
                    } = job
                    {
                        let reply = tx.clone();
                        let asked = address.clone();
                        let spawned = std::thread::Builder::new().name("ui-outputs".into()).spawn(
                            move || {
                                let answer =
                                    crate::pairing::run_put(seq, address, &entry, &outputs, &store);
                                let _ = reply.send(Done::PutOutputs(answer));
                            },
                        );
                        if spawned.is_err() {
                            let answer = not_started_put(seq, asked);
                            if tx.send(Done::PutOutputs(answer)).is_err() {
                                break;
                            }
                        }
                        continue;
                    }
                    if tx.send(run(job)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self { jobs, done })
    }
}

/// The answer of a save of the outputs whose request could not start.
fn not_started_put(seq: u64, address: String) -> crate::pairing::PutAnswer {
    crate::pairing::PutAnswer {
        seq,
        address,
        result: Err(crate::pairing::PutError::Refused(
            "the app could not start the request".into(),
        )),
    }
}

fn run(job: Job) -> Done {
    match job {
        Job::Reconcile(state) => {
            let mut commands = Vec::new();
            let lines = crate::folder_sync::reconcile(&state, |cmd| {
                commands.push(cmd);
                Ok(())
            });
            Done::Reconciled { commands, lines }
        }
        Job::Scan { path, limits } => {
            let listing = radio_library::list_audio(Path::new(&path), true, limits);
            let mut items = Vec::new();
            let mut unreadable = 0;
            for f in &listing.files {
                match file_item(f) {
                    Some(i) => items.push(i),
                    None => unreadable += 1,
                }
            }
            Done::Scan {
                path,
                listing,
                items,
                unreadable,
            }
        }
        Job::Probe {
            req,
            paths,
            recursive,
            limits,
        } => {
            let mut files = Vec::new();
            let mut errors = Vec::new();
            let mut skipped = 0;
            for p in &paths {
                let path = Path::new(p);
                if path.is_dir() {
                    let list = radio_library::list_audio(path, recursive, limits);
                    skipped += list.errors.len();
                    errors.extend(list.errors);
                    files.extend(list.files);
                } else {
                    files.push(p.clone());
                }
            }
            files.truncate(limits.max_files);
            let mut items = Vec::new();
            for f in files {
                match file_item(&f) {
                    Some(item) => items.push(item),
                    None => skipped += 1,
                }
            }
            Done::Probed {
                req,
                items,
                skipped,
                errors,
            }
        }
        Job::Secret { key, set, store } => {
            let wrote = set.is_some();
            let result = match set {
                Some(secret) => store.set(&key, &secret).map(|()| !secret.is_empty()),
                None => store.get(&key).map(|p| p.is_some_and(|p| !p.is_empty())),
            };
            Done::Secret { key, wrote, result }
        }
        Job::Forget { key, store } => Done::Forgot {
            result: store.delete(&key),
        },
        // The lane runs a pairing try on a thread of its own before it gets here.
        Job::Pair { seq, address, .. } => Done::Pair(crate::pairing::PairAnswer {
            seq,
            address,
            key: None,
            typed: false,
            result: Err(crate::pairing::PairError::Refused(
                "the app could not start the request".into(),
            )),
        }),
        // The lane sends the outputs on a thread of its own before it gets here.
        Job::PutOutputs { seq, address, .. } => Done::PutOutputs(not_started_put(seq, address)),
        Job::Icecast {
            seq,
            state,
            store,
            station_name,
        } => Done::Icecast {
            seq,
            result: crate::icecast_config(&state, store.as_ref(), &station_name),
        },
        Job::History {
            log,
            since,
            until,
            limit,
            range,
        } => Done::History(
            log.history_between(since, until, limit)
                .map_err(|e| e.to_string()),
            limit,
            range,
        ),
        Job::ExportHistory {
            log,
            path,
            report,
            since,
            until,
            playlist,
            zone,
        } => {
            let csv = match report {
                HistoryReport::Log => log.export_csv(since, until),
                HistoryReport::Plays => log.plays_csv(since, until, playlist.as_deref()),
                HistoryReport::Airings => log.airings_csv(since, until, playlist.as_deref(), &zone),
            };
            let count = csv
                .as_ref()
                .map_or(0, |c| c.lines().count().saturating_sub(1));
            let result = csv
                .map_err(|e| e.to_string())
                .and_then(|c| std::fs::write(&path, c).map_err(|e| e.to_string()));
            Done::Exported {
                path,
                what: match report {
                    HistoryReport::Log => "as-played rows",
                    HistoryReport::Plays => "titles of the royalty report",
                    HistoryReport::Airings => "airings",
                },
                count,
                result,
            }
        }
        Job::Cover { path } => {
            let bytes = radio_library::cover(&path).map(|c| Arc::from(c.bytes));
            Done::Cover { path, bytes }
        }
        Job::Logo { path, modified } => {
            let stamp = modified
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            let result = radio_stream::load_picture(&path).map(|p| radio_ui_egui::view::Cover {
                uri: format!("bytes://branding-logo/{}?{stamp}", path.display()),
                bytes: p.data.into(),
            });
            Done::Logo {
                path,
                modified,
                result,
            }
        }
        Job::ExportAlarms { path, rows } => {
            let result =
                std::fs::write(&path, crate::alarms::csv(&rows)).map_err(|e| e.to_string());
            Done::Exported {
                path,
                what: "alarms",
                count: rows.len(),
                result,
            }
        }
    }
}
