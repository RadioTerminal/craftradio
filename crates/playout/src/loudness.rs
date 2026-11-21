//! Loudness normalisation on the decks (spec §6.5, Loudness). Each file item plays at the
//! gain that brings it to the station target. The gain comes from the measurement in the
//! engine cache, else from the tags of the file, else unity.

use crate::Measured;
use crate::core::{Core, Phase, add};
use crate::work::Done;
use jiff::Timestamp;
use radio_audio::{Command as Cmd, CommandKind, DeckId};
use radio_engine::Command;
use radio_model::{GainSource, Item, ItemGain, ItemId, Loudness, Source};
use std::path::Path;
use std::time::Duration;

/// Playout measures the items that start within this time. Thus a measurement is
/// normally ready before the item goes on air.
const MEASURE_AHEAD: Duration = Duration::from_secs(15 * 60);
/// Playout measures at most this many entries ahead at a time.
const MEASURE_ENTRIES: usize = 12;
/// A gain change on air ramps over this time.
pub(crate) const GAIN_RAMP: Duration = Duration::from_millis(1500);
/// A smaller gain change than this does not go to the mixer.
const GAIN_STEP_DB: f32 = 0.01;

impl Core {
    /// The gain of an item now: the cached measurement first, then the tags of its file.
    pub(crate) fn item_gain(&self, id: ItemId, tagged: Option<Loudness>) -> ItemGain {
        let state = self.engine.state();
        let known = state
            .loudness_of(id)
            .map(|l| (l, GainSource::Measured))
            .or(tagged.map(|l| (l, GainSource::Tag)));
        state.loudness.gain(known, state.trim_of(id))
    }

    /// Queues a measurement of the file of `item` on the analysis lane. It does nothing
    /// when the engine has a measurement, or when one is in progress. Items outside the
    /// station (the emergency folder) get no measurement. The engine keeps loudness only
    /// for station items.
    pub(crate) fn ensure_measured(&mut self, item: &Item) {
        let Source::File { path } = &item.source else {
            return;
        };
        let state = self.engine.state();
        if !state.loudness.enabled
            || state.loudness_of(item.id).is_some()
            || !self.items.contains_key(&item.id)
            || !self.analysis.measuring.insert(item.id)
        {
            return;
        }
        let (id, path, measure) = (item.id, path.clone(), self.cfg.measure);
        self.work
            .run_slow(move || Done::Measured(id, measure(Path::new(&path))));
    }

    /// Measures the next planned items that start soon.
    pub(crate) fn measure_ahead(&mut self, now: Timestamp) {
        let until = add(now, MEASURE_AHEAD);
        let soon: Vec<Item> = self
            .timeline
            .entries
            .iter()
            .filter(|e| e.start <= until)
            .take(MEASURE_ENTRIES)
            .filter_map(|e| self.items.get(&e.item).cloned())
            .collect();
        for item in &soon {
            self.ensure_measured(item);
        }
    }

    /// A worker measured a file. The engine keeps the result by file hash. A failed
    /// measurement stays in the set of items in progress, so it does not run again.
    pub(crate) fn on_measured(&mut self, id: ItemId, measured: Option<Measured>) {
        let Some(m) = measured else { return };
        let cmd = Command::ItemSetLoudness {
            item: id,
            hash: m.hash,
            size: m.size,
            lufs: m.loudness.lufs,
            peak_db: m.loudness.peak_db,
        };
        if self.engine.dispatch(cmd).is_ok() {
            self.analysis.measuring.remove(&id);
        }
        // The measurement emits no intent. This call only takes its event.
        let _ = self.drain_engine();
    }

    /// Sends a new item gain to each deck whose gain changed. A measurement, a trim or a
    /// settings change can cause this. A playing deck ramps to the new gain. It returns
    /// false when the mixer queue refused a change. Then the slot keeps its old gain, and
    /// the next tick tries again.
    pub(crate) fn refresh_gains(&mut self) -> bool {
        let mut all = true;
        for i in 0..4 {
            let Some(s) = &self.slots[i] else { continue };
            let Some(old) = s.gain else { continue };
            let new = self.item_gain(s.item.id, s.tagged);
            if new == old {
                continue;
            }
            let (tag, phase, playing) = (s.tag, s.phase, !s.not_started());
            if phase == Phase::Opening {
                // The mixer has no load yet: the load takes the new gain.
                if let Some(s) = &mut self.slots[i] {
                    s.options.gain_db = new.db;
                    s.gain = Some(new);
                }
                continue;
            }
            if (new.db - old.db).abs() < GAIN_STEP_DB {
                // No audible change: the view takes the new details, the gain stays.
                if let Some(s) = &mut self.slots[i] {
                    s.gain = Some(ItemGain { db: old.db, ..new });
                }
                continue;
            }
            let over = if playing { GAIN_RAMP } else { Duration::ZERO };
            let kind = CommandKind::SetItemGain {
                deck: DeckId::ALL[i],
                db: new.db,
                over,
            };
            let at = self.mixer.now();
            match self.mixer.send(Cmd::new(at, kind).for_item(tag)) {
                Ok(()) => {
                    if let Some(s) = &mut self.slots[i] {
                        s.gain = Some(new);
                    }
                }
                Err(_) => all = false,
            }
        }
        all
    }
}
