//! Clean-up of one-off items. A one-off with no repeat leaves the schedule after it airs.
//! A one-off that missed its time raises an alarm, and then it leaves too.

use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_model::{ItemId, Repeat};

use crate::alarm::Alarm;
use crate::core::{Core, HARD_LOCK, add, sub};

/// A one-off that is not on a deck this long after its time and tolerance did not air.
const MISS_GRACE: Duration = Duration::from_secs(5);

impl Core {
    /// Notes that a one-off ended on air, so the next sweep removes it.
    pub(crate) fn note_aired_one_off(&mut self, item: ItemId) {
        let state = self.engine.state();
        if state.one_offs().iter().any(|o| o.item.id == item) {
            self.aired_one_offs.insert(item);
        }
    }

    /// Removes the one-offs with no repeat that aired or missed their time.
    ///
    /// A removal renumbers the rules of the One-off playlist. So the sweep waits while a
    /// one-off is on a deck or starts soon.
    pub(crate) fn sweep_one_offs(&mut self, now: Timestamp) {
        let state = self.engine.state();
        let Some(playlist) = radio_engine::one_off_playlist(&state.station) else {
            self.aired_one_offs.clear();
            return;
        };
        let on_deck = self
            .slots
            .iter()
            .flatten()
            .any(|s| s.entry.playlist == playlist);
        let soon = add(now, self.cfg.preload + HARD_LOCK);
        let near = self
            .timeline
            .entries
            .iter()
            .any(|e| e.playlist == playlist && e.start <= soon && add(e.start, e.len) >= now);
        if on_deck || near {
            return;
        }
        let tz = TimeZone::get(&state.station.tz).unwrap_or(TimeZone::UTC);
        let mut aired = Vec::new();
        let mut missed = Vec::new();
        for o in state.one_offs() {
            if o.repeat != Repeat::Once {
                continue;
            }
            let id = o.item.id;
            if self.aired_one_offs.contains(&id) {
                aired.push(id);
                continue;
            }
            let target = radio_timing::resolve_local(&tz, o.start);
            if now <= add(target, o.tolerance + MISS_GRACE) {
                continue;
            }
            let early = sub(target, o.tolerance + MISS_GRACE);
            if self.history.last_played(id).is_some_and(|t| t >= early) {
                aired.push(id);
            } else {
                missed.push((id, o.item.title.clone(), target));
            }
        }
        for id in aired {
            self.aired_one_offs.remove(&id);
            let _ = self.engine.expire_one_off(id);
        }
        for (item, title, target) in missed {
            self.alarms.raise(
                now,
                Alarm::OneOffMissed {
                    item,
                    title,
                    target,
                },
            );
            let _ = self.engine.expire_one_off(item);
        }
        self.aired_one_offs.retain(|id| {
            self.engine
                .state()
                .one_offs()
                .iter()
                .any(|o| o.item.id == *id)
        });
    }
}
