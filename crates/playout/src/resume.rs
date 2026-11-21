//! Resume after a restart (spec §6.11). Playout saves the timeline and the open log rows
//! through the engine. On start it plays the entry that the saved plan has on air now.

use crate::core::{Core, since, sub};
use crate::view::DeckRole;
use jiff::Timestamp;
use radio_engine::{LogStatus, RowId};
use radio_model::{ItemId, Source};
use radio_scheduler::Entry;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const KEY: &str = "playout";
/// Playout saves at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(1);
/// The number of timeline entries that playout saves.
const SAVED_ENTRIES: usize = 50;

#[derive(Serialize, Deserialize)]
struct Saved {
    at: Timestamp,
    entries: Vec<Entry>,
    /// The open log row of each entry on air.
    rows: Vec<(ItemId, Timestamp, i64)>,
}

impl Core {
    /// Ends a row that a crash cut off, at the last save before the crash. A row that
    /// already has an end (a clean stop) keeps it.
    fn end_cut_row(&mut self, row: RowId, start: Timestamp, at: Timestamp) {
        let open = self
            .engine
            .history(sub(start, Duration::from_secs(1)), 100)
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| r.id == row))
            .is_some_and(|r| r.end.is_none());
        if open {
            let _ = self.engine.log_end(row, at, LogStatus::Interrupted);
        }
    }

    /// Saves the timeline and the open rows, at most once a second.
    pub(crate) fn save_state(&mut self, now: Timestamp) {
        if !self.save_due || self.last_save.is_some_and(|t| since(t, now) < SAVE_EVERY) {
            return;
        }
        let rows = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.role == DeckRole::Plan)
            .filter_map(|s| s.row.map(|r| (s.item.id, s.entry.start, r.0)))
            .collect();
        let saved = Saved {
            at: now,
            entries: self
                .timeline
                .entries
                .iter()
                .take(SAVED_ENTRIES)
                .cloned()
                .collect(),
            rows,
        };
        if let Ok(value) = serde_json::to_value(&saved)
            && self.engine.save_runtime(KEY, &value).is_ok()
        {
            self.save_due = false;
            self.last_save = Some(now);
        }
    }

    /// Puts the entry that the saved plan has on air now back on a deck. A trimmable item
    /// continues at its position. An item that must not get shorter (an ad) plays from
    /// the start again. Its cut-off row ends as interrupted. Streams and live items start
    /// again at once, until their planned end.
    pub(crate) fn resume(&mut self, now: Timestamp) {
        let saved = self
            .engine
            .runtime(KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value::<Saved>(v).ok());
        let Some(saved) = saved else { return };
        let end = |e: &Entry| crate::core::add(e.start, e.len.saturating_sub(e.trim));
        // During a crossfade two entries overlap; the later one is the item on air.
        let Some(e) = saved
            .entries
            .iter()
            .rfind(|e| e.start <= now && now < end(e))
            .cloned()
        else {
            return;
        };
        let item = self
            .engine
            .state()
            .station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter())
            .find(|i| i.id == e.item)
            .cloned();
        let Some(item) = item else { return };
        let row = saved
            .rows
            .iter()
            .find(|(id, start, _)| *id == e.item && *start == e.start)
            .map(|r| RowId(r.2));
        let mut entry = e.clone();
        let offset = match item.source {
            Source::File { .. } if item.trimmable => since(e.start, now),
            Source::File { .. } => {
                if let Some(row) = row {
                    self.end_cut_row(row, e.start, saved.at);
                }
                entry.start = now;
                Duration::ZERO
            }
            Source::Stream { .. } | Source::LiveInput { .. } => {
                entry.len = since(now, end(&e));
                entry.trim = Duration::ZERO;
                entry.start = now;
                Duration::ZERO
            }
        };
        if !offset.is_zero() {
            entry.start = sub(now, offset);
        }
        let Some(i) = self.free_deck() else { return };
        if let Err(reason) = self.load(i, entry, item.clone(), DeckRole::Plan, offset) {
            self.skip_broken(&item, reason, now);
        }
    }
}
