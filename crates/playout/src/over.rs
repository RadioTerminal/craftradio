//! Over items: the plan lays them on a song's intro or outro, and playout plays them on the
//! last cart slot, timed from where their song really is, not from the wall clock.

use crate::core::{Core, Phase, add, since};
use crate::work::Done;
use jiff::Timestamp;
use radio_audio::{Clip, Command as Cmd, CommandKind};
use radio_engine::{LogEntry, LogStatus};
use radio_model::{ItemId, Source};
use radio_scheduler::{OverEntry, Played, Side};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

/// How long before its moment an over item goes to the mixer.
const LEAD: Duration = Duration::from_millis(60);
/// How far a deck's entry may lie from the planned start of an over item's host.
const HOST_MATCH: Duration = Duration::from_secs(1);
/// How late an over item may still start.
const GRACE: Duration = Duration::from_millis(40);

/// The cart slot of over items: the last one. The cartwall uses the others.
pub(crate) fn over_slot(cart_slots: usize) -> Option<usize> {
    cart_slots.checked_sub(1)
}

/// The over item in the mixer and the ones that are done.
#[derive(Default)]
pub(crate) struct OverPlayer {
    asked: Option<ItemId>,
    loaded: Option<ItemId>,
    /// Played or skipped: by the tag of the host's airing and the side.
    done: BTreeSet<(u64, Side)>,
}

impl Core {
    /// Keeps the next over item loaded, fires the one whose moment came, and skips the ones
    /// whose song was paused, cut or gone at that moment.
    pub(crate) fn sync_over(&mut self, now: Timestamp) {
        let Some(slot) = over_slot(self.cfg.mixer.cart_slots) else {
            return;
        };
        let mut pending: Vec<OverEntry> = self.timeline.over.clone();
        pending.sort_by_key(|o| o.entry.start);
        let mut next_load = None;
        for o in pending {
            // The airing the plan meant: the same item at the planned start (a later
            // airing of the same song is another host).
            let host = self.slots.iter().flatten().find(|s| {
                s.item.id == o.over.host
                    && s.entry
                        .start
                        .duration_since(o.over.host_start)
                        .unsigned_abs()
                        <= HOST_MATCH
                    && s.started.is_some()
                    && !s.cut
                    && !s.end_sent
            });
            let Some(host) = host else {
                if since(o.entry.start, now) < GRACE || now < o.entry.start {
                    next_load.get_or_insert(o.entry.item);
                }
                continue;
            };
            let key = (host.tag, o.over.side);
            if self.over.done.contains(&key) {
                continue;
            }
            let Some(started) = host.started else {
                continue;
            };
            let moment = add(started, o.over.offset);
            let playing = host.phase == Phase::Playing;
            if now >= moment && since(moment, now) > GRACE {
                // The moment passed while the song was paused, or before playout saw it.
                self.over.done.insert(key);
                continue;
            }
            next_load.get_or_insert(o.entry.item);
            let due = now >= moment || moment.duration_since(now).unsigned_abs() <= LEAD;
            if !due || !playing {
                continue;
            }
            if self.over.loaded != Some(o.entry.item) {
                // Not loaded in time: never fire it late over the wrong part of the song.
                self.over.done.insert(key);
                continue;
            }
            let at = self.map.sample_at(moment).max(self.mixer.now());
            let duck_db = f32::from(o.over.duck_db);
            if self
                .mixer
                .send(Cmd::new(at, CommandKind::FireCart { slot, duck_db }))
                .is_ok()
            {
                self.over.done.insert(key);
                self.log_over(&o, moment);
            }
        }
        if let Some(item) = next_load {
            self.load_over(slot, item);
        }
    }

    /// Asks a worker for the clip of `item` unless the slot has it or waits for it.
    fn load_over(&mut self, slot: usize, item: ItemId) {
        if self.over.asked == Some(item) {
            return;
        }
        let Some(path) = self.items.get(&item).and_then(|i| match &i.source {
            Source::File { path } => Some(path.clone()),
            _ => None,
        }) else {
            return;
        };
        self.over.asked = Some(item);
        self.over.loaded = None;
        let _ = self.mixer.set_cart(slot, None);
        self.work.run(move || {
            Done::Cart(
                slot,
                item,
                Clip::from_file(Path::new(&path)).map_err(|e| e.to_string()),
            )
        });
    }

    /// A worker decoded the clip of an over item.
    pub(crate) fn on_over_clip(&mut self, slot: usize, item: ItemId, result: Result<Clip, String>) {
        if self.over.asked != Some(item) {
            return;
        }
        match result.map(|clip| self.mixer.set_cart(slot, Some(clip))) {
            Ok(Ok(())) => self.over.loaded = Some(item),
            _ => self.over.asked = None,
        }
    }

    /// Writes the over item to the as-played log, from `moment` for its length.
    fn log_over(&mut self, o: &OverEntry, moment: Timestamp) {
        let Some(item) = self.items.get(&o.entry.item).cloned() else {
            return;
        };
        let playlist = self
            .engine
            .state()
            .station
            .playlists
            .iter()
            .find(|p| p.id == o.entry.playlist)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let origin = match o.over.side {
            Side::Intro => "over intro",
            Side::Outro => "over outro",
        };
        let rule = match &o.entry.origin {
            radio_scheduler::Origin::Rule { playlist, rule } => {
                Some(format!("{}#{rule}", playlist.0))
            }
            _ => None,
        };
        let entry = LogEntry {
            start: moment,
            playlist,
            title: item.title.clone(),
            artist: item.artist.clone(),
            origin: origin.to_string(),
            rule,
        };
        self.history.push(Played {
            item: item.id,
            playlist: o.entry.playlist,
            at: moment,
            artist: item.artist.clone(),
            title: item.title.clone(),
        });
        let tag = self.next_tag;
        self.next_tag += 1;
        self.ended
            .insert(tag, (add(moment, o.entry.len), LogStatus::Aired));
        let done = self.work.done_tx.clone();
        self.log.log_start_later(&entry, move |result| {
            let result = result.map_err(|e| e.to_string());
            let _ = done.send(Done::Row { tag, result });
        });
    }
}
