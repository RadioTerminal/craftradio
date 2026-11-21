//! Weaves the named playlists into the main playlist's timeline by their rules.

use crate::constraints::item_ok;
use crate::{FitResult, Picker, PlayHistory, Played, allowed};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use radio_model::{
    Constraint, FixMode, Item, ItemId, Placement, Playlist, PlaylistId, Station, Trigger,
};
use radio_timing::{advance, item_len, occurrences};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::time::Duration;

/// Zero-length items would stall the walk; every entry advances time by at least this.
const MIN_STEP: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Main,
    Rule {
        playlist: PlaylistId,
        rule: usize,
    },
    Manual,
    /// Added from a fill pool by the fitter to close a gap before a fixed event.
    Fill,
}

/// A fixed-time event's wish: where it should start and how far it may miss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixed {
    pub target: Timestamp,
    pub mode: FixMode,
    pub tolerance: Duration,
    pub priority: i32,
    /// The window before this event keeps its order: the fitter only fills and trims there
    /// and pulls no entry across the window's boundaries. Set by `plan`.
    #[serde(default)]
    pub frozen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub playlist: PlaylistId,
    pub item: ItemId,
    pub start: Timestamp,
    pub len: Duration,
    pub origin: Origin,
    pub locked: bool,
    pub fixed: Option<Fixed>,
    pub trim: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Warning {
    NoEligibleItem {
        playlist: PlaylistId,
        at: Timestamp,
    },
    DeadAir {
        at: Timestamp,
        gap: Duration,
    },
    FixedConflict {
        playlist: PlaylistId,
        wanted: Timestamp,
        got: Timestamp,
    },
    OutsideTolerance {
        playlist: PlaylistId,
        error: SignedDuration,
    },
    /// A locked entry could not keep its start: a non-trimmable item ran into it.
    LockedMoved {
        item: ItemId,
        by: Duration,
    },
    /// The station's time zone is unknown; local-time rules were checked in UTC.
    UnknownTimeZone {
        tz: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub entries: Vec<Entry>,
    pub fits: Vec<FitResult>,
    pub warnings: Vec<Warning>,
}

pub struct PlanInput<'a> {
    pub station: &'a Station,
    pub history: &'a PlayHistory,
    pub now: Timestamp,
    pub horizon: Duration,
    /// Locked, manual and on-deck entries. They are kept as given and the walk plans around them.
    pub pinned: &'a [Entry],
}

/// How a between-tracks rule decides it is due.
enum Due {
    Tracks { n: u32, seen: u32 },
    Times { at: Vec<Timestamp>, next: usize },
}

struct TrackRule {
    playlist: usize,
    rule: usize,
    priority: i32,
    due: Due,
}

impl TrackRule {
    /// Whether the rule fires at a boundary at `at`; consumes what it fires on.
    fn take_due(&mut self, at: Timestamp) -> bool {
        match &mut self.due {
            Due::Tracks { n, seen } => {
                let due = *seen >= *n;
                if due {
                    *seen = 0;
                }
                due
            }
            Due::Times { at: times, next } => {
                let before = *next;
                while times.get(*next).is_some_and(|t| *t <= at) {
                    *next += 1;
                }
                *next > before
            }
        }
    }
}

struct FixedRule {
    playlist: usize,
    rule: usize,
    target: Timestamp,
    mode: FixMode,
    tolerance: Duration,
}

struct Walk<'a> {
    station: &'a Station,
    tz: TimeZone,
    items: BTreeMap<ItemId, (&'a Item, PlaylistId)>,
    hist: PlayHistory,
    pickers: Vec<Picker>,
    entries: Vec<Entry>,
    warnings: Vec<Warning>,
    /// Where the next entry starts.
    next_at: Timestamp,
}

impl<'a> Walk<'a> {
    fn step(item: &Item) -> Duration {
        advance(item, Some(item)).max(MIN_STEP)
    }

    fn record(&mut self, pl: PlaylistId, item: &Item, at: Timestamp) {
        self.hist.push(Played {
            item: item.id,
            playlist: pl,
            at,
            artist: item.artist.clone(),
            title: item.title.clone(),
        });
    }

    fn place(
        &mut self,
        pl: &Playlist,
        item: &Item,
        start: Timestamp,
        origin: Origin,
        fixed: Option<Fixed>,
    ) {
        self.record(pl.id, item, start);
        self.entries.push(Entry {
            playlist: pl.id,
            item: item.id,
            start,
            len: item_len(item),
            origin,
            locked: item.locked,
            fixed,
            trim: Duration::ZERO,
        });
        // A fixed entry can start before the walk's position; never move the walk backwards.
        let end = start
            .checked_add(Self::step(item))
            .unwrap_or(Timestamp::MAX);
        self.next_at = self.next_at.max(end);
    }

    fn adopt(&mut self, e: &Entry) {
        let known = self.items.get(&e.item).copied();
        let step = known.map_or(e.len.max(MIN_STEP), |(item, _)| Self::step(item));
        if let Some((item, _)) = known {
            self.record(e.playlist, item, e.start);
        }
        let end = e.start.checked_add(step).unwrap_or(Timestamp::MAX);
        self.next_at = self.next_at.max(end);
        self.entries.push(e.clone());
    }

    /// Next item of playlist `pi` that passes item validity, the playlist's constraints and `extra`.
    fn pick(&mut self, pi: usize, extra: &[Constraint], at: Timestamp) -> Option<&'a Item> {
        let station: &'a Station = self.station;
        let pl = &station.playlists[pi];
        let (hist, tz, prev) = (&self.hist, &self.tz, self.entries.last());
        self.pickers[pi].next(pl, hist, |item| {
            item_ok(item, at, hist, tz)
                && pl
                    .constraints
                    .iter()
                    .chain(extra)
                    .all(|c| allowed(c, item, pl.id, at, prev, hist, tz))
        })
    }

    /// Inserts the `take` items of one rule, warning if the playlist cannot supply them all.
    fn fire(&mut self, pi: usize, ri: usize, at: Timestamp, fixed: Option<Fixed>) {
        let station: &'a Station = self.station;
        let pl = &station.playlists[pi];
        let rule = &pl.rules[ri];
        let mut got = 0;
        for k in 0..rule.take {
            let start = match (&fixed, k) {
                (Some(f), 0) => f.target,
                _ => self.next_at.max(at),
            };
            let Some(item) = self.pick(pi, &rule.constraints, start) else {
                break;
            };
            let origin = Origin::Rule {
                playlist: pl.id,
                rule: ri,
            };
            let first_fixed = if k == 0 { fixed.clone() } else { None };
            self.place(pl, item, start, origin, first_fixed);
            got += 1;
        }
        if got < rule.take {
            self.warnings.push(Warning::NoEligibleItem {
                playlist: pl.id,
                at,
            });
        }
    }

    fn fire_fixed(&mut self, f: &FixedRule) {
        let priority = self.station.playlists[f.playlist].priority;
        let fixed = Fixed {
            target: f.target,
            mode: f.mode,
            tolerance: f.tolerance,
            priority,
            frozen: false,
        };
        self.fire(f.playlist, f.rule, f.target, Some(fixed));
    }
}

/// Merges the station's playlists into one timeline from `now` over `horizon`.
///
/// Walks forward taking the next main item and, at each boundary between tracks, fires the
/// due `BetweenTracks` rules of enabled non-main playlists by descending playlist priority.
/// `Fixed` rules become entries at their target time, in time order, and are not fitted
/// (that is the fitter's job). Rules that find no eligible item warn and are skipped; the main
/// playlist continues. Simulated plays count toward constraints and caps. The walk stops at
/// the horizon, so a rule due only after the last main item of the horizon does not fire.
/// An unknown station time zone falls back to UTC; validate the station first.
pub fn merge(input: &PlanInput) -> Timeline {
    let station = input.station;
    let tz = TimeZone::get(&station.tz).unwrap_or(TimeZone::UTC);
    let now = input.now;
    let end = now.checked_add(input.horizon).unwrap_or(Timestamp::MAX);
    let items = station
        .playlists
        .iter()
        .flat_map(|p| p.items.iter().map(move |i| (i.id, (i, p.id))))
        .collect();
    let mut w = Walk {
        station,
        tz: tz.clone(),
        items,
        hist: input.history.clone(),
        pickers: station.playlists.iter().map(Picker::new).collect(),
        entries: Vec::new(),
        warnings: Vec::new(),
        next_at: now,
    };

    let Some(main) = station.playlists.iter().position(|p| p.is_main) else {
        let gap = end.duration_since(now).unsigned_abs();
        w.warnings.push(Warning::DeadAir { at: now, gap });
        return Timeline {
            entries: w.entries,
            fits: Vec::new(),
            warnings: w.warnings,
        };
    };

    let mut rules = Vec::new();
    let mut fixed = Vec::new();
    for (pi, pl) in station.playlists.iter().enumerate() {
        if !pl.enabled || pl.is_main {
            continue;
        }
        for (ri, rule) in pl.rules.iter().enumerate() {
            let clock = !matches!(
                rule.trigger,
                Trigger::EveryNTracks(_) | Trigger::ScriptEvent(_)
            );
            match (&rule.placement, &rule.trigger) {
                (Placement::BetweenTracks, Trigger::EveryNTracks(n)) if *n > 0 => {
                    let due = Due::Tracks { n: *n, seen: 0 };
                    rules.push(TrackRule {
                        playlist: pi,
                        rule: ri,
                        priority: pl.priority,
                        due,
                    });
                }
                (Placement::BetweenTracks, trigger) if clock => {
                    let at = occurrences(&tz, trigger, now, end);
                    let due = Due::Times { at, next: 0 };
                    rules.push(TrackRule {
                        playlist: pi,
                        rule: ri,
                        priority: pl.priority,
                        due,
                    });
                }
                (Placement::Fixed { mode, tolerance }, trigger) if clock => {
                    fixed.extend(
                        occurrences(&tz, trigger, now, end)
                            .into_iter()
                            .map(|target| FixedRule {
                                playlist: pi,
                                rule: ri,
                                target,
                                mode: *mode,
                                tolerance: *tolerance,
                            }),
                    );
                }
                _ => {}
            }
        }
    }
    fixed.sort_by_key(|f| {
        (
            f.target,
            Reverse(station.playlists[f.playlist].priority),
            f.playlist,
            f.rule,
        )
    });
    let mut pinned = input.pinned.to_vec();
    pinned.sort_by_key(|e| e.start);

    let (mut next_fixed, mut next_pinned) = (0, 0);
    loop {
        while pinned
            .get(next_pinned)
            .is_some_and(|e| e.start <= w.next_at)
        {
            w.adopt(&pinned[next_pinned]);
            next_pinned += 1;
        }
        if w.next_at >= end {
            break;
        }
        while let Some(f) = fixed.get(next_fixed).filter(|f| f.target <= w.next_at) {
            w.fire_fixed(f);
            next_fixed += 1;
        }
        let at = w.next_at;
        let mut firing: Vec<usize> = (0..rules.len())
            .filter(|&i| rules[i].take_due(at))
            .collect();
        firing.sort_by_key(|&i| Reverse(rules[i].priority));
        for i in firing {
            let (pi, ri) = (rules[i].playlist, rules[i].rule);
            w.fire(pi, ri, at, None);
        }
        if w.next_at >= end {
            break;
        }

        let at = w.next_at;
        let Some(item) = w.pick(main, &[], at) else {
            let gap = end.duration_since(at).unsigned_abs();
            w.warnings.push(Warning::DeadAir { at, gap });
            break;
        };
        w.place(&station.playlists[main], item, at, Origin::Main, None);
        for r in &mut rules {
            if let Due::Tracks { seen, .. } = &mut r.due {
                *seen = seen.saturating_add(1);
            }
        }
    }
    for f in &fixed[next_fixed..] {
        w.fire_fixed(f);
    }
    for e in pinned[next_pinned..].iter().filter(|e| e.start < end) {
        w.adopt(e);
    }
    w.entries.sort_by_key(|e| e.start);
    Timeline {
        entries: w.entries,
        fits: Vec::new(),
        warnings: w.warnings,
    }
}
