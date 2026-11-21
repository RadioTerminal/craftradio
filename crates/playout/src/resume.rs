//! Resume after a restart (spec §6.11). Playout saves the timeline and the open log rows
//! through the engine. On start it plays the entry that the saved plan has on air now.

use crate::core::{Core, since, sub};
use crate::view::DeckRole;
use jiff::Timestamp;
use radio_engine::{LogStatus, RowId};
use radio_model::{ItemId, OnInterrupt, Source};
use radio_scheduler::Entry;
use radio_scheduler::Played;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const KEY: &str = "playout";
/// Playout saves at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(1);
/// The number of timeline entries that playout saves.
const SAVED_ENTRIES: usize = 50;
/// While something is on air, playout saves at least this often.
const HEARTBEAT: Duration = Duration::from_secs(5);

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
        if let Ok(value) = serde_json::to_value(&saved) {
            self.log.save_runtime_later(KEY, value);
            self.save_due = false;
            self.last_save = Some(now);
        }
    }

    /// Asks for a save every few seconds while something is on air. A crash then cuts
    /// rows close to their real end.
    pub(crate) fn heartbeat(&mut self, now: Timestamp) {
        let on_air = self.slots.iter().flatten().any(|s| !s.not_started());
        if on_air && self.last_save.is_none_or(|t| since(t, now) >= HEARTBEAT) {
            self.save_due = true;
        }
    }

    /// Puts the entry that the saved plan has on air now back on a deck. The
    /// `on_interrupt` of its playlist decides: `Resume` continues at the position and in
    /// the same log row. `Replay` plays it from the start in a new row. `Skip` drops it.
    /// The cut-off row of a replay or a skip ends as interrupted. Streams and live items
    /// start again at once, until their planned end.
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
        let station = &self.engine.state().station;
        let found = station.playlists.iter().find_map(|p| {
            p.items
                .iter()
                .find(|i| i.id == e.item)
                .map(|i| (i.clone(), p.on_interrupt))
        });
        let Some((item, rule)) = found else { return };
        let row = saved
            .rows
            .iter()
            .find(|(id, start, _)| *id == e.item && *start == e.start)
            .map(|r| RowId(r.2));
        let mut entry = e.clone();
        let mut keep_row = None;
        let offset = match (&item.source, rule) {
            (Source::File { .. }, OnInterrupt::Resume) => {
                keep_row = row;
                since(e.start, now)
            }
            (Source::File { .. }, OnInterrupt::Replay) => {
                if let Some(row) = row {
                    self.end_cut_row(row, e.start, saved.at);
                }
                entry.start = now;
                Duration::ZERO
            }
            (_, OnInterrupt::Skip) => {
                if let Some(row) = row {
                    self.end_cut_row(row, e.start, saved.at);
                }
                // The rotation continues after the skipped item.
                self.history.push(Played {
                    item: item.id,
                    playlist: e.playlist,
                    at: e.start,
                    artist: item.artist.clone(),
                    title: item.title.clone(),
                });
                return;
            }
            (Source::Stream { .. } | Source::LiveInput { .. }, _) => {
                keep_row = row;
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
        match self.load(i, entry, item.clone(), DeckRole::Plan, offset) {
            Ok(_) => {
                if let Some(s) = &mut self.slots[i] {
                    s.row = keep_row;
                }
            }
            Err(reason) => self.skip_broken(&item, reason, now),
        }
    }
}
