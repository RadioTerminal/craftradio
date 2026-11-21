//! Dead air (spec §6.14) and file analysis. On silence, playout skips to the next item.
//! If Program stays silent, the emergency fallback folder plays. Playout goes back to the
//! plan at the next fixed event, or when the next planned item is ready.

use crate::alarm::Alarm;
use crate::core::{Core, Phase, add, since, sub};
use crate::ops::Check;
use crate::view::DeckRole;
use crate::work::Done;
use jiff::Timestamp;
use radio_audio::{DeckId, DeckState};
use radio_engine::Mode;
use radio_model::{Item, ItemId, ItemMeta, PlaylistId, Source, Timing};
use radio_scheduler::{Entry, Origin};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A skip on silence loads an entry on demand only if it starts within this time.
const ON_DEMAND_AHEAD: Duration = Duration::from_secs(600);

/// The state of the dead-air logic and of the emergency fallback.
#[derive(Default)]
pub(crate) struct DeadAir {
    /// Playout skipped on silence at this time, and waits to hear audio again.
    pub(crate) skipped_at: Option<Timestamp>,
    /// The emergency fallback plays.
    pub(crate) emergency: bool,
    /// The fallback began, or last checked the plan, at this time.
    checked_at: Option<Timestamp>,
    /// The files of the fallback folder, and the index of the next one.
    files: Vec<PathBuf>,
    next: usize,
    /// Fallback files that failed one after another.
    failures: usize,
}

/// File analysis results by item. `None` means in progress or failed.
#[derive(Default)]
pub(crate) struct Analysis {
    done: BTreeMap<ItemId, Option<Timing>>,
}

fn audio_file(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or_default();
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac"
    )
}

fn list_folder(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.is_file() && audio_file(p));
    files.sort();
    files
}

impl Core {
    /// Tells if the station is quiet on purpose: an operator holds it, or it waits for one.
    fn quiet_on_purpose(&self) -> bool {
        let mode = self.engine.state().mode;
        if mode == Mode::Manual || self.hold {
            return true;
        }
        // A paused item is an operator's wait, in any mode.
        if self
            .slots
            .iter()
            .flatten()
            .any(|s| s.phase == Phase::Paused)
        {
            return true;
        }
        let on_air = self
            .slots
            .iter()
            .flatten()
            .any(|s| matches!(s.role, DeckRole::Plan | DeckRole::Operator) && !s.not_started());
        mode != Mode::Auto && !on_air
    }

    /// Program has been silent for the detector time.
    pub(crate) fn on_silence(&mut self, now: Timestamp) {
        if self.quiet_on_purpose() || self.keep_alive_on_air() {
            return;
        }
        self.alarms.raise(now, Alarm::DeadAir);
        if self.dead_air.emergency {
            return;
        }
        let skipped = match self.start_next(None) {
            Check::Now(false) => self.load_next_now(now),
            _ => true,
        };
        if skipped {
            self.dead_air.skipped_at = Some(now);
        } else {
            self.enter_emergency(now, "nothing to skip to");
        }
    }

    /// Fades out everything on air at once.
    fn cut_all(&mut self) {
        let at = self.mixer.now();
        for d in 0..4 {
            let fade = self.slots[d]
                .as_ref()
                .filter(|s| !s.not_started() && !s.end_sent)
                .is_some();
            if fade {
                self.fade_at(d, at, Duration::ZERO, true);
            }
        }
    }

    /// Loads the next planned entry now and starts it once it is ready. It returns false
    /// when the plan has no entry or no deck is free.
    fn load_next_now(&mut self, now: Timestamp) -> bool {
        let assigned = self.assigned();
        let from = sub(now, Duration::from_secs(1));
        let next = self
            .timeline
            .entries
            .iter()
            // Never a fixed or locked entry, and never one far ahead: those keep their time.
            .filter(|e| e.fixed.is_none() && !e.locked && e.start <= add(now, ON_DEMAND_AHEAD))
            .find(|e| e.start >= from && !assigned.contains(&(e.item, e.start)))
            .cloned();
        let Some(entry) = next else { return false };
        let Some(item) = self.items.get(&entry.item).cloned() else {
            return false;
        };
        let Some(i) = self.free_deck() else {
            return false;
        };
        self.cut_all();
        match self.load(i, entry, item.clone(), DeckRole::Plan, Duration::ZERO) {
            Ok(_) => {
                if let Some(s) = &mut self.slots[i] {
                    s.asap = true;
                }
                true
            }
            Err(reason) => {
                self.skip_broken(&item, reason, now);
                false
            }
        }
    }

    /// Program has audio again.
    pub(crate) fn on_sound(&mut self, now: Timestamp) {
        self.dead_air.skipped_at = None;
        self.alarms.clear(now, |a| *a == Alarm::DeadAir);
    }

    /// Goes to the fallback when the skip did not bring audio back in time. While the
    /// fallback plays, it checks if the plan can play again.
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
        if !self.dead_air.emergency {
            return;
        }
        self.return_from_emergency(now);
        if self.dead_air.emergency {
            self.play_emergency(now);
        }
    }

    /// Goes back to the plan when a fixed event is due. At each retry, it also goes back
    /// when the next planned item is ready on a deck.
    fn return_from_emergency(&mut self, now: Timestamp) {
        let soon = add(now, self.cfg.command_lead);
        let fixed_due = self.slots.iter().flatten().any(|s| {
            s.role == DeckRole::Plan
                && s.not_started()
                && s.entry.fixed.is_some()
                && s.entry.start <= soon
        });
        let retry = self
            .dead_air
            .checked_at
            .is_none_or(|t| since(t, now) >= self.cfg.emergency_retry);
        if !fixed_due && !retry {
            return;
        }
        self.dead_air.checked_at = Some(now);
        let ready = (0..4).any(|i| {
            self.slots[i].as_ref().is_some_and(|s| {
                s.role == DeckRole::Plan
                    && s.not_started()
                    && self.mixer.deck_state(DeckId::ALL[i]) == DeckState::Ready
            })
        });
        if !fixed_due && !ready {
            return;
        }
        self.leave_emergency(now);
        if !fixed_due {
            let _ = self.start_next(None);
        }
    }

    fn enter_emergency(&mut self, now: Timestamp, why: &str) {
        self.alarms.raise(
            now,
            Alarm::EmergencyFallback {
                reason: why.to_string(),
            },
        );
        if let Some(dir) = self.cfg.fallback_folder.clone() {
            self.work.run(move || Done::Folder(list_folder(&dir)));
        }
        self.cut_all();
        self.dead_air = DeadAir {
            emergency: true,
            checked_at: Some(now),
            ..DeadAir::default()
        };
    }

    /// A worker listed the fallback folder.
    pub(crate) fn on_folder(&mut self, files: Vec<PathBuf>, now: Timestamp) {
        if !self.dead_air.emergency {
            return;
        }
        if files.is_empty() {
            let reason = "the emergency folder has no audio files".to_string();
            self.alarms.raise(now, Alarm::EmergencyFallback { reason });
        }
        self.dead_air.files = files;
        self.dead_air.next = 0;
    }

    /// Leaves the fallback, and the fallback file fades out. Operator commands and the
    /// return checks call this.
    pub(crate) fn leave_emergency(&mut self, now: Timestamp) {
        if !self.dead_air.emergency {
            return;
        }
        self.dead_air.emergency = false;
        self.alarms
            .clear(now, |a| matches!(a, Alarm::EmergencyFallback { .. }));
        let at = self.mixer.now();
        for i in 0..4 {
            let fade = self.slots[i]
                .as_ref()
                .is_some_and(|s| s.role == DeckRole::Emergency && !s.end_sent);
            if fade {
                self.fade_at(i, at, crate::core::CUT_FADE, true);
            }
        }
    }

    pub(crate) fn dead_air_file_failed(&mut self) {
        self.dead_air.failures += 1;
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
                if let Some(s) = &mut self.slots[i] {
                    s.asap = true;
                }
            }
            Err(_) => self.dead_air.failures += 1,
        }
    }

    fn keep_alive_on_air(&self) -> bool {
        self.slots.iter().flatten().any(|s| {
            matches!(s.phase, Phase::Playing)
                && matches!(
                    s.item.source,
                    Source::Stream {
                        keep_alive: true,
                        ..
                    }
                )
        })
    }

    /// A worker finished the analysis of an item.
    pub(crate) fn on_analysed(&mut self, id: ItemId, timing: Option<Timing>) {
        self.analysis.done.insert(id, timing);
        self.dirty = true;
    }

    /// Gives file items without cue points the timing that analysis found. It queues
    /// analysis on the workers for each new file item without timing.
    pub(crate) fn apply_analysis(&mut self, station: &mut radio_model::Station) {
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
                    let (id, path) = (item.id, path.clone());
                    self.work.run_slow(move || {
                        Done::Analysed(id, radio_library::analyse(&path).ok().map(|a| a.timing))
                    });
                }
            }
        }
    }
}
