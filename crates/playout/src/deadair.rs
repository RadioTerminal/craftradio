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
    /// Program went silent while the station was quiet on purpose, and is silent still.
    /// The mixer reports a silence once, so playout looks again when the purpose ends.
    unheard: bool,
    /// The purpose ended at this time, with Program still silent. Program gets the
    /// detector time from here before the silence counts as dead air.
    unheard_since: Option<Timestamp>,
}

/// File analysis results by item. `None` means in progress or failed.
#[derive(Default)]
pub(crate) struct Analysis {
    done: BTreeMap<ItemId, Option<Timing>>,
    /// Items whose loudness is in measurement, or whose measurement failed. A failed one
    /// is not measured again until playout restarts, so a silent file costs one decode.
    pub(crate) measuring: std::collections::BTreeSet<ItemId>,
    /// An analysis found markers since the view last took them.
    pub(crate) markers_changed: bool,
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
            self.dead_air.unheard = true;
            self.dead_air.unheard_since = None;
            return;
        }
        self.dead_air.unheard = false;
        self.dead_air.unheard_since = None;
        self.alarms.raise(now, Alarm::DeadAir);
        if self
            .slots
            .iter()
            .flatten()
            .any(|s| !s.not_started() && matches!(s.item.source, Source::LiveInput { .. }))
        {
            return;
        }
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
                .filter(|s| !s.not_started() && !s.end_sent && s.phase != Phase::Paused)
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
            .filter(|e| e.start >= from && !assigned.contains(&(e.item, e.start)))
            // A fixed or locked entry keeps its time and stops the search. Nothing after
            // it moves in front of it, and nothing far ahead moves either.
            .take_while(|e| e.fixed.is_none() && !e.locked && e.start <= add(now, ON_DEMAND_AHEAD))
            .next()
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
        self.dead_air.unheard = false;
        self.dead_air.unheard_since = None;
        self.alarms.clear(now, |a| *a == Alarm::DeadAir);
    }

    /// Goes to the fallback when the skip did not bring audio back in time. While the
    /// fallback plays, it checks if the plan can play again.
    pub(crate) fn check_dead_air(&mut self, now: Timestamp) {
        let after = self.cfg.mixer.silence.after;
        if self.dead_air.unheard {
            if self.quiet_on_purpose() || self.keep_alive_on_air() {
                self.dead_air.unheard_since = None;
            } else {
                // An item that starts now needs time to sound, as after any silence.
                let from = *self.dead_air.unheard_since.get_or_insert(now);
                if since(from, now) >= after {
                    self.on_silence(now);
                }
            }
        }
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
                    && !s.soft_too_early(now)
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

    /// A fallback file did not open. Once every file has failed in a row, the fallback has
    /// nothing to play, and an alarm says so.
    pub(crate) fn dead_air_file_failed(&mut self, now: Timestamp) {
        self.dead_air.failures += 1;
        if self.dead_air.emergency && self.dead_air.failures == self.dead_air.files.len() {
            let reason = "the files of the emergency folder did not open".to_string();
            self.alarms.raise(now, Alarm::EmergencyFallback { reason });
        }
    }

    /// A fallback file is on air, so the folder works.
    pub(crate) fn dead_air_file_started(&mut self) {
        self.dead_air.failures = 0;
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
        let Some(i) = self.spare_deck() else { return };
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
            // The file opens on a worker; only its start on air shows that it plays.
            Ok(_) => {
                if let Some(s) = &mut self.slots[i] {
                    s.asap = true;
                }
            }
            Err(_) => self.dead_air_file_failed(now),
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

    /// The markers the analysis found, for the view.
    pub(crate) fn found_markers(
        &self,
    ) -> std::collections::BTreeMap<
        ItemId,
        (Option<std::time::Duration>, Option<std::time::Duration>),
    > {
        self.analysis
            .done
            .iter()
            .filter_map(|(id, t)| {
                let t = t.as_ref()?;
                (t.markers == radio_model::MarkerSource::Detected)
                    .then_some((*id, (t.intro_end, t.outro_start)))
            })
            .collect()
    }

    /// A worker finished the analysis of an item.
    pub(crate) fn on_analysed(&mut self, id: ItemId, timing: Option<Timing>) {
        self.analysis.markers_changed |= timing
            .as_ref()
            .is_some_and(|t| t.markers == radio_model::MarkerSource::Detected);
        self.analysis.done.insert(id, timing);
        self.dirty = true;
    }

    /// Gives file items without cue points the timing that analysis found, and items with
    /// cue points but no markers the markers it found. It queues analysis on the workers
    /// for each such item it has not asked for yet.
    pub(crate) fn apply_analysis(&mut self, station: &mut radio_model::Station) {
        // Markers matter only to a playlist that plays over the music.
        let wants_markers = station
            .playlists
            .iter()
            .any(|p| matches!(p.plays, radio_model::PlayMode::Over { .. }));
        for item in station
            .playlists
            .iter_mut()
            .flat_map(|p| p.items.iter_mut())
        {
            let Source::File { path } = &item.source else {
                continue;
            };
            let unmarked = wants_markers
                && item
                    .timing
                    .as_ref()
                    .is_some_and(|t| t.markers == radio_model::MarkerSource::NotAnalysed);
            if unmarked {
                // Only the markers: the item keeps the cue points it has. They live in the
                // plan's copy of the station, like the timing of a file without cue points.
                match self.analysis.done.entry(item.id) {
                    std::collections::btree_map::Entry::Occupied(found) => {
                        if let (Some(found), Some(t)) = (found.get(), item.timing.as_mut()) {
                            t.intro_end = found.intro_end;
                            t.outro_start = found.outro_start;
                            t.markers = found.markers;
                        }
                    }
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(None);
                        let (id, path) = (item.id, path.clone());
                        let analyse = self.cfg.analyse;
                        self.work
                            .run_slow(move || Done::Analysed(id, analyse(Path::new(&path))));
                    }
                }
                continue;
            }
            if item.timing.is_some() {
                continue;
            }
            match self.analysis.done.get(&item.id) {
                Some(found) => item.timing.clone_from(found),
                None => {
                    self.analysis.done.insert(item.id, None);
                    let (id, path) = (item.id, path.clone());
                    let analyse = self.cfg.analyse;
                    self.work
                        .run_slow(move || Done::Analysed(id, analyse(Path::new(&path))));
                }
            }
        }
    }
}
