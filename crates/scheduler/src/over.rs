//! Items of an over playlist: laid on a song's intro or outro, taking no airtime.
//!
//! After fitting, each entry of a playlist that plays `Over` looks at its neighbours. It lies
//! over the next song's intro if the intro is long enough, else over the current song's
//! outro, else it stays in sequence. One handover (the outro of a song and the intro of the
//! next) takes at most one over item.

use crate::{Entry, Origin, Timeline};
use jiff::Timestamp;
use radio_model::{Constraint, Item, ItemId, PlayMode, Playlist, PlaylistId, Source, Station};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Where on its host an over item lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// From the host's start, over its intro.
    Intro,
    /// Ending at the host's segue, over its outro.
    Outro,
}

/// The song an over item lies on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Over {
    pub host: ItemId,
    /// The host's start in the plan.
    pub host_start: Timestamp,
    pub side: Side,
    /// From the host's start to the over item's start.
    pub offset: Duration,
    /// The duck depth in whole dB, 0 or lower.
    pub duck_db: i16,
}

/// A planned entry that plays over its host. `entry.start` is `host_start + offset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverEntry {
    pub entry: Entry,
    pub over: Over,
}

/// The parts of the station that placement reads.
pub(crate) struct Lookup<'a> {
    items: BTreeMap<ItemId, &'a Item>,
    playlists: BTreeMap<PlaylistId, &'a Playlist>,
}

impl<'a> Lookup<'a> {
    pub(crate) fn new(station: &'a Station) -> Self {
        Self {
            items: station
                .playlists
                .iter()
                .flat_map(|p| p.items.iter().map(|i| (i.id, i)))
                .collect(),
            playlists: station.playlists.iter().map(|p| (p.id, p)).collect(),
        }
    }

    /// The duck depth of an entry that may play over the music.
    fn over_duck(&self, e: &Entry) -> Option<i16> {
        if e.fixed.is_some() || e.locked {
            return None;
        }
        match self.playlists.get(&e.playlist)?.plays {
            PlayMode::Over { duck_db } => Some(duck_db),
            PlayMode::InSequence => None,
        }
    }

    /// The item of an entry that can carry an over item: a file of an in-sequence playlist.
    fn host(&self, e: &Entry) -> Option<&'a Item> {
        let p = self.playlists.get(&e.playlist)?;
        if p.plays != PlayMode::InSequence {
            return None;
        }
        let item = self.items.get(&e.item)?;
        (matches!(item.source, Source::File { .. }) && item.timing.is_some()).then_some(*item)
    }

    /// Whether `next` may follow `prev` directly under its adjacency constraints.
    fn adjacent_ok(&self, prev: &Entry, next: &Entry) -> bool {
        let Some(p) = self.playlists.get(&next.playlist) else {
            return true;
        };
        let rule = match next.origin {
            Origin::Rule { rule, .. } => p.rules.get(rule).map(|r| r.constraints.as_slice()),
            _ => None,
        };
        p.constraints
            .iter()
            .chain(rule.unwrap_or_default())
            .all(|c| match c {
                Constraint::NotAdjacentToSelf => prev.playlist != next.playlist,
                Constraint::NotAdjacentTo(other) => prev.playlist != *other,
                _ => true,
            })
    }
}

/// The offset from the start of `host` at which an over item of `len` starts on `side`, if
/// it fits there.
fn fits(host: &Item, trim: Duration, side: Side, len: Duration) -> Option<Duration> {
    let t = host.timing.as_ref()?;
    match side {
        Side::Intro => {
            let end = t.intro_end.filter(|&e| e > t.cue_in && e <= t.segue)?;
            (end - t.cue_in >= len).then_some(Duration::ZERO)
        }
        Side::Outro => {
            if trim > Duration::ZERO {
                return None;
            }
            let from = t.outro_start.filter(|&o| o > t.cue_in && o <= t.segue)?;
            (t.segue - from >= len).then(|| t.segue - t.cue_in - len)
        }
    }
}

/// The key of a handover: the starts of the entries on either side of it.
type Handover = (Option<Timestamp>, Option<Timestamp>);

/// Takes the entries that lie over the music out of `tl.entries` and returns them.
pub(crate) fn place(tl: &mut Timeline, look: &Lookup) -> Vec<OverEntry> {
    let work = std::mem::take(&mut tl.entries);
    let mut out: Vec<Entry> = Vec::with_capacity(work.len());
    let mut placed = Vec::new();
    let mut used: BTreeSet<Handover> = BTreeSet::new();
    for (i, e) in work.iter().enumerate() {
        let Some(duck_db) = look.over_duck(e) else {
            out.push(e.clone());
            continue;
        };
        let prev = out.last();
        let next = work[i + 1..].iter().find(|n| look.over_duck(n).is_none());
        let key = (prev.map(|p| p.start), next.map(|n| n.start));
        let len = look
            .items
            .get(&e.item)
            .map_or(e.len, |it| radio_timing::item_len(it));
        let adjacent = match (prev, next) {
            (Some(p), Some(n)) => look.adjacent_ok(p, n),
            _ => true,
        };
        let intro = next.and_then(|n| {
            let h = look.host(n)?;
            fits(h, n.trim, Side::Intro, len).map(|off| (n, Side::Intro, off))
        });
        let outro = || {
            prev.and_then(|p| {
                let h = look.host(p)?;
                fits(h, p.trim, Side::Outro, len).map(|off| (p, Side::Outro, off))
            })
        };
        let spot = (adjacent && !used.contains(&key))
            .then(|| intro.or_else(outro))
            .flatten();
        match spot {
            Some((h, side, offset)) => {
                used.insert(key);
                let mut entry = e.clone();
                entry.start = h.start + offset;
                entry.len = len;
                placed.push(OverEntry {
                    entry,
                    over: Over {
                        host: h.item,
                        host_start: h.start,
                        side,
                        offset,
                        duck_db,
                    },
                });
            }
            None => out.push(e.clone()),
        }
    }
    tl.entries = out;
    placed
}

/// The index of the entry that is `o`'s host after a refit: the same item, the nearest
/// start.
fn find_host(entries: &[Entry], o: &OverEntry) -> Option<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.item == o.over.host)
        .min_by_key(|(_, e)| e.start.duration_since(o.over.host_start).unsigned_abs())
        .map(|(i, _)| i)
}

/// Checks the placed entries against the refitted plan. Those whose host is gone or no
/// longer fits them go back into `tl.entries`, in sequence next to where they were. It
/// returns the ones that stay over, and whether any went back.
pub(crate) fn settle(
    tl: &mut Timeline,
    look: &Lookup,
    placed: Vec<OverEntry>,
) -> (Vec<OverEntry>, bool) {
    let mut keep = Vec::with_capacity(placed.len());
    let mut back = Vec::new();
    for o in placed {
        let host = find_host(&tl.entries, &o).and_then(|i| {
            let h = &tl.entries[i];
            let item = look.host(h)?;
            fits(item, h.trim, o.over.side, o.entry.len)?;
            Some(i)
        });
        match host {
            Some(_) => keep.push(o),
            None => back.push(o.entry),
        }
    }
    let went_back = !back.is_empty();
    for e in back {
        let at = tl.entries.partition_point(|x| x.start <= e.start);
        tl.entries.insert(at, e);
    }
    (keep, went_back)
}

/// Sets each over entry's start from its host's final start.
pub(crate) fn anchor(tl: &mut Timeline) {
    let entries = &tl.entries;
    for o in &mut tl.over {
        if let Some(i) = find_host(entries, o) {
            o.over.host_start = entries[i].start;
            o.entry.start = entries[i].start + o.over.offset;
        }
    }
}

/// True when some playlist of the station plays over the music.
pub(crate) fn any_over(station: &Station) -> bool {
    station
        .playlists
        .iter()
        .any(|p| matches!(p.plays, PlayMode::Over { .. }))
}
