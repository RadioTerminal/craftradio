//! The background lane of the source. One thread reads the files, the keychain and the
//! as-played log, so the UI thread never waits on them. The snapshot takes the results.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use jiff::Timestamp;
use radio_engine::{AsPlayed, LogHandle};
use radio_library::{Limits, Listing};
use radio_model::Item;

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
    /// Read the as-played log.
    History {
        log: LogHandle,
        since: Timestamp,
        limit: u32,
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
    History(Result<Vec<AsPlayed>, String>),
    /// The alarm export wrote `count` rows to `path`, or failed.
    Exported {
        path: String,
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
                    if tx.send(run(job)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self { jobs, done })
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
        Job::History { log, since, limit } => {
            Done::History(log.history(since, limit).map_err(|e| e.to_string()))
        }
        Job::ExportAlarms { path, rows } => {
            let result =
                std::fs::write(&path, crate::alarms::csv(&rows)).map_err(|e| e.to_string());
            Done::Exported {
                path,
                count: rows.len(),
                result,
            }
        }
    }
}
