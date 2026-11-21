//! Dead air (spec §6.14) and file analysis. On silence, playout skips to the next item.
//! If Program stays silent, the emergency fallback folder plays until an operator acts.

use crate::alarm::Alarm;
use crate::core::{Core, Phase, since};
use crate::view::DeckRole;
use jiff::Timestamp;
use radio_engine::Mode;
use radio_model::{Item, ItemId, ItemMeta, PlaylistId, Source, Timing};
use radio_scheduler::{Entry, Origin};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// The state of the dead-air logic and of the emergency fallback.
#[derive(Default)]
pub(crate) struct DeadAir {
    /// Playout skipped on silence at this time, and waits to hear audio again.
    pub(crate) skipped_at: Option<Timestamp>,
    /// The emergency fallback plays.
    pub(crate) emergency: bool,
    /// The files of the fallback folder, and the index of the next one.
    files: Vec<PathBuf>,
    next: usize,
    /// Fallback files that failed one after another.
    failures: usize,
}

/// File analysis results, from worker threads.
pub(crate) struct Analysis {
    done: BTreeMap<ItemId, Option<Timing>>,
    tx: Sender<(ItemId, Option<Timing>)>,
    rx: Receiver<(ItemId, Option<Timing>)>,
}

impl Default for Analysis {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            done: BTreeMap::new(),
            tx,
            rx,
        }
    }
}

fn audio_file(p: &std::path::Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or_default();
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac"
    )
}

impl Core {
    /// Program has been silent for the detector time.
    pub(crate) fn on_silence(&mut self, now: Timestamp) {
        if self.engine.state().mode == Mode::Manual || self.keep_alive_on_air() {
            return;
        }
        self.alarms.raise(now, Alarm::DeadAir);
        if self.dead_air.emergency {
            return;
        }
        let fallback = matches!(self.start_next(None), crate::ops::Check::Now(false));
        if fallback {
            self.enter_emergency(now, "nothing to skip to");
        } else {
            self.dead_air.skipped_at = Some(now);
        }
    }

    /// Program has audio again.
    pub(crate) fn on_sound(&mut self, now: Timestamp) {
        self.dead_air.skipped_at = None;
        self.alarms.clear(now, |a| *a == Alarm::DeadAir);
    }

    /// Goes to the fallback when the skip did not bring audio back in time.
    pub(crate) fn check_dead_air(&mut self, now: Timestamp) {
        let after = self.cfg.mixer.silence.after;
        if self
            .dead_air
            .skipped_at
            .is_some_and(|t| since(t, now) >= after)
        {
            self.dead_air.skipped_at = None;
            self.enter_emergency(now, "skipping did not end the silence");
        }
        if self.dead_air.emergency {
            self.play_emergency(now);
        }
    }

    fn enter_emergency(&mut self, now: Timestamp, why: &str) {
        let files = self.cfg.fallback_folder.as_ref().map(|dir| {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .map(|rd| rd.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            files.retain(|p| p.is_file() && audio_file(p));
            files.sort();
            files
        });
        let files = files.unwrap_or_default();
        let reason = if files.is_empty() {
            format!("{why}; the emergency folder has no audio files")
        } else {
            why.to_string()
        };
        self.alarms.raise(now, Alarm::EmergencyFallback { reason });
        let at = self.mixer.now();
        for i in 0..4 {
            let fade = self.slots[i]
                .as_ref()
                .filter(|s| !s.not_started() && !s.end_sent)
                .is_some();
            if fade {
                self.fade_at(i, at, Duration::ZERO, true);
            }
        }
        self.dead_air = DeadAir {
            emergency: true,
            files,
            ..DeadAir::default()
        };
    }

    /// Leaves the fallback. Operator commands call this.
    pub(crate) fn leave_emergency(&mut self, now: Timestamp) {
        if self.dead_air.emergency {
            self.dead_air.emergency = false;
            self.alarms
                .clear(now, |a| matches!(a, Alarm::EmergencyFallback { .. }));
        }
    }

    /// Keeps one fallback file on air while the fallback plays.
    fn play_emergency(&mut self, now: Timestamp) {
        let busy = self.slots.iter().flatten().any(|s| {
            s.role == DeckRole::Emergency || matches!(s.phase, Phase::Cued(_) | Phase::Playing)
        });
        let d = &self.dead_air;
        if busy || d.files.is_empty() || d.failures >= d.files.len() {
            return;
        }
        let Some(i) = self.free_deck() else { return };
        let path = d.files[d.next % d.files.len()].clone();
        self.dead_air.next += 1;
        let n = self.dead_air.next as u64;
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let item = Item {
            id: ItemId(u64::MAX - n),
            meta: ItemMeta::default(),
            title,
            artist: None,
            source: Source::File {
                path: path.to_string_lossy().into_owned(),
            },
            timing: None,
            planned_len: None,
            movable: false,
            trimmable: false,
            locked: true,
            valid_from: None,
            valid_to: None,
            max_per_day: None,
        };
        let entry = Entry {
            playlist: PlaylistId(0),
            item: item.id,
            start: now,
            len: Duration::ZERO,
            origin: Origin::Manual,
            locked: true,
            fixed: None,
            trim: Duration::ZERO,
        };
        match self.load(i, entry, item, DeckRole::Emergency, Duration::ZERO) {
            Ok(_) => {
                self.dead_air.failures = 0;
                let at = self.mixer.now();
                self.play_at(i, at);
            }
            Err(_) => self.dead_air.failures += 1,
        }
    }

    fn keep_alive_on_air(&self) -> bool {
        self.slots.iter().flatten().any(|s| {
            matches!(s.phase, Phase::Playing) && matches!(s.item.source, Source::Stream { .. })
        })
    }

    /// Gives file items without cue points the timing that analysis found. It starts
    /// analysis on a worker thread for each new file item without timing.
    pub(crate) fn apply_analysis(&mut self, station: &mut radio_model::Station) {
        while let Ok((id, timing)) = self.analysis.rx.try_recv() {
            self.analysis.done.insert(id, timing);
        }
        for item in station
            .playlists
            .iter_mut()
            .flat_map(|p| p.items.iter_mut())
        {
            let Source::File { path } = &item.source else {
                continue;
            };
            if item.timing.is_some() {
                continue;
            }
            match self.analysis.done.get(&item.id) {
                Some(found) => item.timing.clone_from(found),
                None => {
                    self.analysis.done.insert(item.id, None);
                    let (tx, id, path) = (self.analysis.tx.clone(), item.id, path.clone());
                    let _ = std::thread::Builder::new()
                        .name("playout-analyse".into())
                        .spawn(move || {
                            let timing = radio_library::analyse(&path).ok().map(|a| a.timing);
                            let _ = tx.send((id, timing));
                        });
                }
            }
        }
    }
}
