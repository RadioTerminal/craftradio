use crate::constraints::{allowed_with, item_ok};
use crate::history::HistoryOverlay;
use crate::{DropReason, Entry, Fixed, Origin, PlayHistory, Played, Timeline, Warning, allowed};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use radio_model::{Constraint, FixMode, Item, ItemId, Playlist, PlaylistId, Station};
use radio_timing::advance;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

/// The time step used by the subset search.
pub const RESOLUTION: Duration = Duration::from_millis(100);

/// The longest gap the subset search looks at. Longer gaps are searched as this plus
/// ten minutes and report the rest as leftover, which bounds the search's memory and time.
pub const MAX_GAP: Duration = Duration::from_secs(6 * 3600);

/// How far past the remaining gap the search looks, so a slightly late plan can win.
const OVERSHOOT: Duration = Duration::from_secs(600);

/// Picks entries for `gap`, preserving their order, and returns the actual time left.
/// A positive remainder means an early finish; a negative remainder means a late finish.
///
/// Each candidate gives its duration, whether it is movable, and whether it was
/// originally before the event. Non-movable original-before entries are always kept;
/// non-movable after entries are ignored. Pass locked entries as non-movable.
/// Dropping a movable original-before entry costs one displacement. Keeping an
/// after entry costs one displacement.
///
/// Movable lengths are rounded to the nearest `RESOLUTION` (half steps round up).
/// The search covers the remaining gap plus ten minutes, at most [`MAX_GAP`] plus ten minutes. Equal distances prefer fewer
/// displacements, then an earlier finish. The remainder uses unrounded durations.
pub fn select_before(
    gap: Duration,
    cands: &[(
        Duration,
        bool, /* movable */
        bool, /* orig_before */
    )],
) -> (Vec<usize>, SignedDuration) {
    let gap_ns = gap.as_nanos();
    let mandatory: Vec<_> = cands
        .iter()
        .enumerate()
        .filter_map(|(index, (_, movable, orig_before))| {
            (!movable && *orig_before).then_some(index)
        })
        .collect();
    let mandatory_ns = mandatory.iter().fold(0_u128, |sum, &index| {
        sum.saturating_add(cands[index].0.as_nanos())
    });
    let remaining_ns = gap_ns.saturating_sub(mandatory_ns);
    let step = RESOLUTION.as_nanos();
    let cap_ns = MAX_GAP.saturating_add(OVERSHOOT).as_nanos();
    let limit = remaining_ns
        .checked_add(OVERSHOOT.as_nanos())
        .map_or(cap_ns, |l| l.min(cap_ns))
        / step;
    // No movable entry can help if mandatory entries already exceed the gap.
    let active: Vec<_> = cands
        .iter()
        .enumerate()
        .filter(|(_, (_, movable, _))| *movable && mandatory_ns <= gap_ns)
        .map(|(index, (length, _, orig_before))| {
            (index, (length.as_nanos() + step / 2) / step, *orig_before)
        })
        .collect();
    // The original selection is the unique zero-displacement solution. If it
    // reaches the target exactly in search ticks, no other subset can beat it.
    let original_ticks: u128 = active
        .iter()
        .filter(|(_, _, before)| *before)
        .map(|(_, ticks, _)| *ticks)
        .sum();
    if mandatory_ns <= gap_ns && original_ticks * step == remaining_ns && original_ticks <= limit {
        let mut kept = mandatory;
        kept.extend(
            active
                .iter()
                .filter(|(_, _, before)| *before)
                .map(|(i, _, _)| *i),
        );
        kept.sort_unstable();
        let played: u128 = kept.iter().map(|&i| cands[i].0.as_nanos()).sum();
        return (
            kept,
            SignedDuration::from_nanos_i128(gap_ns as i128 - played as i128),
        );
    }
    // Equal lengths need only a count, not a subset grid. For each count,
    // retain original entries first. Equal-cost subsets prefer earlier candidates,
    // just as the descending DP does when taking and skipping have equal cost.
    if let Some(&(_, ticks, _)) = active.first()
        && ticks > 0
        && active.iter().all(|(_, length, _)| *length == ticks)
    {
        let before = active.iter().filter(|(_, _, before)| *before).count();
        let max_count = active.len().min((limit / ticks) as usize);
        let count = (0..=max_count)
            .min_by_key(|&count| {
                let sum = count as u128 * ticks * step;
                (remaining_ns.abs_diff(sum), count.abs_diff(before), sum)
            })
            .unwrap_or(0);
        let mut kept = mandatory;
        kept.extend(
            active
                .iter()
                .filter(|(_, _, before)| *before)
                .take(count)
                .map(|(i, _, _)| *i),
        );
        kept.extend(
            active
                .iter()
                .filter(|(_, _, before)| !*before)
                .take(count.saturating_sub(before))
                .map(|(i, _, _)| *i),
        );
        kept.sort_unstable();
        let played: u128 = kept.iter().map(|&i| cands[i].0.as_nanos()).sum();
        let remainder = (gap_ns as i128 - played as i128).clamp(
            SignedDuration::MIN.as_nanos(),
            SignedDuration::MAX.as_nanos(),
        );
        return (kept, SignedDuration::from_nanos_i128(remainder));
    }
    // Only multiples of the greatest common divisor are reachable. Compress
    // those cells without changing rounded lengths, costs, or tie breaking.
    let divisor = active
        .iter()
        .fold(0, |mut a, (_, ticks, _)| {
            let mut b = *ticks;
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        })
        .max(1);
    let active: Vec<_> = active
        .into_iter()
        .map(|(i, ticks, before)| (i, ticks / divisor, before))
        .collect();
    let limit = limit / divisor;
    let step = step * divisor;
    let reachable_limit = active
        .iter()
        .fold(0_u128, |sum, (_, ticks, _)| sum.saturating_add(*ticks));
    // `limit` is at most (MAX_GAP + OVERSHOOT) / RESOLUTION, which fits any usize.
    let limit = limit.min(reachable_limit) as usize;
    let cells = limit + 1;
    let words = cells.div_ceil(64);
    let mut costs = vec![u32::MAX; cells];
    costs[0] = 0;
    let mut decisions = Vec::with_capacity(active.len());
    for &(_, ticks, orig_before) in &active {
        let mut selected = vec![0_u64; words];
        // Descending updates read the previous candidate's row, including for zero ticks.
        for sum in (0..cells).rev() {
            let skipped = costs[sum].saturating_add(u32::from(orig_before));
            let taken = if ticks <= sum as u128 {
                costs[sum - ticks as usize].saturating_add(u32::from(!orig_before))
            } else {
                u32::MAX
            };
            costs[sum] = skipped.min(taken);
            if taken < skipped {
                selected[sum / 64] |= 1_u64 << (sum % 64);
            }
        }
        decisions.push(selected);
    }
    let mut best_sum = costs
        .iter()
        .enumerate()
        .filter(|(_, cost)| **cost != u32::MAX)
        .min_by_key(|(ticks, cost)| {
            let sum_ns = *ticks as u128 * step;
            (remaining_ns.abs_diff(sum_ns), **cost, sum_ns)
        })
        .map_or(0, |(sum, _)| sum);
    let mut kept = mandatory;
    for ((index, ticks, _), selected) in active.iter().zip(&decisions).rev() {
        if selected[best_sum / 64] & (1_u64 << (best_sum % 64)) != 0 {
            kept.push(*index);
            best_sum -= *ticks as usize;
        }
    }
    kept.sort_unstable();
    let played_ns = kept.iter().fold(0_u128, |sum, &index| {
        sum.saturating_add(cands[index].0.as_nanos())
    });
    // jiff's signed duration has a smaller range than std::time::Duration.
    let leftover = (gap_ns as i128 - played_ns.min(i128::MAX as u128) as i128).clamp(
        SignedDuration::MIN.as_nanos(),
        SignedDuration::MAX.as_nanos(),
    );
    (kept, SignedDuration::from_nanos_i128(leftover))
}

/// One adjustment the fitter made to bring a fixed event onto its target time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FitStep {
    Reordered { moved: usize },
    Filled { items: Vec<ItemId> },
    Trimmed { by: Duration },
    EarlyStart { by: Duration },
    LateStart { by: Duration },
}

/// The outcome of fitting one fixed event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitResult {
    pub playlist: PlaylistId,
    pub target: Timestamp,
    pub planned: Timestamp,
    pub error: SignedDuration,
    pub within_tolerance: bool,
    pub steps: Vec<FitStep>,
}

/// The most a track is faded early before a `Soft` event. Before a `Hard` event the
/// on-air track is faded to finish at the target however long that takes.
const MAX_TRIM: Duration = Duration::from_secs(30);

/// How many entries after a fixed event the reorder step may pull in front of it.
const LOOKAHEAD: usize = 10;

/// The least a trimmed entry may still play; shorter, it is dropped instead.
const MIN_PLAY: Duration = Duration::from_secs(1);

/// Before a hard event, a fill item faded at the target must still play at least this long,
/// and at least [`OVERRUN_MIN_SHARE_PERCENT`] of its length.
const OVERRUN_MIN_PLAY: Duration = Duration::from_secs(5);

/// The least share of its length, in percent, an overrunning fill item must still play.
const OVERRUN_MIN_SHARE_PERCENT: u32 = 50;

/// An overrunning fill is only preferred when the nearest fill leaves more dead air than this.
const OVERRUN_DEAD_AIR: Duration = Duration::from_millis(500);

/// How often the reorder and fill steps re-run after a rotation rule rejects their choice.
const RETRIES: usize = 3;

fn later(t: Timestamp, d: Duration) -> Timestamp {
    t.checked_add(d).unwrap_or(Timestamp::MAX)
}

fn is_hard(e: &Entry) -> bool {
    e.fixed.as_ref().is_some_and(|f| f.mode == FixMode::Hard)
}

/// `|a − b|` as an unsigned duration.
fn apart(a: Timestamp, b: Timestamp) -> Duration {
    a.duration_since(b).unsigned_abs()
}

/// A rule insertion the planner may take out to keep a hard constraint: never an anchor
/// (fixed or locked), main, manual or fill entry.
fn droppable_insertion(e: &Entry) -> bool {
    matches!(e.origin, Origin::Rule { .. }) && e.fixed.is_none() && !e.locked
}

fn drop_warning(e: &Entry, reason: DropReason) -> Warning {
    Warning::EntryDropped {
        at: e.start,
        playlist: e.playlist,
        item: e.item,
        reason,
    }
}

fn mask(len: usize, picked: &[usize]) -> Vec<bool> {
    let mut keep = vec![false; len];
    for &j in picked {
        if let Some(k) = keep.get_mut(j) {
            *k = true;
        }
    }
    keep
}

struct Fitter<'a> {
    station: &'a Station,
    needs_history: bool,
    tz: TimeZone,
    items: BTreeMap<ItemId, &'a Item>,
}

impl<'a> Fitter<'a> {
    fn item(&self, e: &Entry) -> Option<&'a Item> {
        self.items.get(&e.item).copied()
    }

    fn playlist(&self, id: PlaylistId) -> Option<&'a Playlist> {
        let station: &'a Station = self.station;
        station.playlists.iter().find(|p| p.id == id)
    }

    /// Time from `e` starting to the entry after it starting, less `e`'s early fade.
    /// Before a hard fixed entry, or with nothing after it, `e` plays to its end.
    fn step(&self, e: &Entry, next: Option<&Entry>) -> Duration {
        let followed = next.is_some_and(|n| !is_hard(n));
        let base = match self.item(e) {
            // `advance` only asks whether something follows, not what.
            Some(item) => advance(item, followed.then_some(item)),
            None => e.len,
        };
        base.saturating_sub(e.trim)
    }

    /// How long `e` holds the air before an ordinary next entry starts.
    fn natural(&self, e: &Entry) -> Duration {
        self.item(e)
            .map_or(e.len, |item| advance(item, Some(item)))
            .saturating_sub(e.trim)
    }

    /// The rotation rules `e` must keep: its playlist's constraints, plus those of the rule
    /// that inserted it (merge picked it under both).
    fn constraints(&self, e: &Entry) -> impl Iterator<Item = &'a Constraint> {
        let pl = self.playlist(e.playlist);
        let from_rule = match (&e.origin, pl) {
            (Origin::Rule { playlist, rule }, Some(p)) if *playlist == p.id => {
                p.rules.get(*rule).map_or(&[][..], |r| &r.constraints[..])
            }
            _ => &[],
        };
        pl.map_or(&[][..], |p| &p.constraints[..])
            .iter()
            .chain(from_rule)
    }

    /// Whether `e` may follow `prev` under its adjacency rules (`NotAdjacentToSelf`,
    /// `NotAdjacentTo`). These depend only on the neighbour, not on when `e` plays.
    fn may_follow(&self, prev: &Entry, e: &Entry) -> bool {
        let Some(item) = self.item(e) else {
            return true;
        };
        let none = PlayHistory::new();
        self.constraints(e)
            .filter(|c| {
                matches!(
                    c,
                    Constraint::NotAdjacentToSelf | Constraint::NotAdjacentTo(_)
                )
            })
            .all(|c| allowed(c, item, e.playlist, e.start, Some(prev), &none, &self.tz))
    }

    /// Puts `e`, moved across an anchor, in front of `flow`. A rule-inserted entry that
    /// would break an adjacency rule there is dropped, as is a rule-inserted front entry
    /// that may not follow `e`: constraints are hard, and the rule's other insertions stay.
    /// Each drop is reported in `dropped`.
    fn push_moved(&self, flow: &mut VecDeque<Entry>, e: Entry, dropped: &mut Vec<Warning>) {
        let clash = flow
            .front()
            .is_some_and(|front| !self.may_follow(&e, front));
        if clash && droppable_insertion(&e) {
            dropped.push(drop_warning(&e, DropReason::AdjacencyAfterMove));
            return;
        }
        if clash
            && flow.front().is_some_and(droppable_insertion)
            && let Some(front) = flow.pop_front()
        {
            dropped.push(drop_warning(&front, DropReason::AdjacencyAfterMove));
        }
        flow.push_front(e);
    }

    /// Drops the rule-inserted entries at the front of `flow` that may not follow `prev`,
    /// reporting each in `dropped`.
    fn drop_clashing_front(
        &self,
        prev: Option<&Entry>,
        flow: &mut VecDeque<Entry>,
        dropped: &mut Vec<Warning>,
    ) {
        let Some(prev) = prev else {
            return;
        };
        while let Some(front) =
            flow.pop_front_if(|front| droppable_insertion(front) && !self.may_follow(prev, front))
        {
            dropped.push(drop_warning(&front, DropReason::AdjacencyAfterMove));
        }
    }

    fn movable(&self, e: &Entry) -> bool {
        e.fixed.is_none() && !e.locked && self.item(e).is_some_and(|i| i.movable)
    }

    fn trimmable(&self, e: &Entry) -> bool {
        e.fixed.is_none() && !e.locked && self.item(e).is_some_and(|i| i.trimmable)
    }

    /// Precedence for overlapping fixed events: a `Hard` event beats a `Soft` event. Then
    /// the higher playlist priority wins.
    fn rank(&self, e: &Entry) -> (bool, i32) {
        let tier = is_hard(e);
        let priority = self.playlist(e.playlist).map_or_else(
            || e.fixed.as_ref().map_or(0, |f| f.priority),
            |p| p.priority,
        );
        (tier, priority)
    }

    /// Where `e` starts when the material before it hands over at `t`. Anchors never start
    /// early: a `Hard` entry waits for its target, a `Soft` one for its target less its
    /// tolerance, a locked one for its own start.
    fn anchor(&self, e: &Entry, t: Timestamp) -> Timestamp {
        match &e.fixed {
            Some(f) if f.mode == FixMode::Hard => t.max(f.target),
            Some(f) => t.max(f.target.checked_sub(f.tolerance).unwrap_or(Timestamp::MIN)),
            None if e.locked => t.max(e.start),
            None => t,
        }
    }

    /// The starts of `seq` with the material handing over to `seq[0]` at `base`, and when it
    /// hands over to `then`. Already placed anchors keep their start.
    fn times(
        &self,
        base: Timestamp,
        seq: &[Entry],
        then: Option<&Entry>,
    ) -> (Vec<Timestamp>, Timestamp) {
        let mut starts = Vec::with_capacity(seq.len());
        let mut t = base;
        for (i, e) in seq.iter().enumerate() {
            let start = self.anchor(e, t);
            starts.push(start);
            t = later(start, self.step(e, seq.get(i + 1).or(then)));
        }
        (starts, t)
    }

    /// When the material `seq`, starting at `base`, hands over to `then`.
    fn arrival(&self, base: Timestamp, seq: &[Entry], then: &Entry) -> Timestamp {
        self.times(base, seq, Some(then)).1
    }

    /// Sets the start of every entry, the first at `base`, using the anchor rules.
    fn reflow(&self, entries: &mut [Entry], base: Timestamp) {
        let (starts, _) = self.times(base, entries, None);
        for (e, start) in entries.iter_mut().zip(starts) {
            e.start = start;
        }
    }

    fn record(&self, sim: &mut PlayHistory, e: &Entry, at: Timestamp) {
        if !self.needs_history {
            return;
        }
        if let Some(item) = self.item(e) {
            sim.push(Played {
                item: item.id,
                playlist: e.playlist,
                at,
                artist: item.artist.clone(),
                title: item.title.clone(),
            });
        }
    }

    /// The first entry of `seq` marked in `check` that breaks item validity or its playlist's
    /// constraints at its start, given the entry before it (`prev` for `seq[0]`) and `sim`
    /// plus the plays of `seq` before it.
    fn first_violation(
        &self,
        sim: &PlayHistory,
        prev: Option<&Entry>,
        seq: &[Entry],
        starts: &[Timestamp],
        check: &[bool],
    ) -> Option<usize> {
        let last = check.iter().rposition(|check| *check)?;
        let needs_history = seq.iter().zip(check).take(last + 1).any(|(e, check)| {
            *check
                && (self.item(e).is_some_and(|item| item.max_per_day.is_some())
                    || self.constraints(e).any(|c| {
                        matches!(
                            c,
                            Constraint::MinGapSameItem(_)
                                | Constraint::ArtistSeparation(_)
                                | Constraint::TitleSeparation(_)
                                | Constraint::MaxPerHour(_)
                                | Constraint::MaxPerDay(_)
                        )
                    }))
        });
        let mut overlay = HistoryOverlay {
            base: sim,
            scratch: PlayHistory::new(),
        };
        for (i, (e, &at)) in seq.iter().zip(starts).take(last + 1).enumerate() {
            let before = if i == 0 { prev } else { seq.get(i - 1) };
            if check.get(i).copied().unwrap_or(false)
                && let Some(item) = self.item(e)
            {
                let ok = item_ok(item, at, &overlay, &self.tz)
                    && self
                        .constraints(e)
                        .all(|c| allowed_with(c, item, e.playlist, at, before, &overlay, &self.tz));
                if !ok {
                    return Some(i);
                }
            }
            if needs_history {
                self.record(&mut overlay.scratch, e, at);
            }
        }
        None
    }

    /// Returns the start slot of each anchor. The fixed events go first, by rank, target and
    /// list order. Then the locked entries and the soft events go in time order. A locked
    /// entry goes first at the same time. A soft event starts after the locked entry in its
    /// way. The later locked entries start after the soft event, as after a hard event.
    fn resolve(&self, events: &[Entry]) -> Vec<Timestamp> {
        let target = |e: &Entry| e.fixed.as_ref().map_or(e.start, |f| f.target);
        let mut order: Vec<usize> = (0..events.len())
            .filter(|&i| events[i].fixed.is_some())
            .collect();
        order.sort_by_key(|&i| (Reverse(self.rank(&events[i])), target(&events[i]), i));
        let wanted: Vec<Timestamp> = events.iter().map(target).collect();
        let fixed_slots = self.place(events, &order, &wanted, Vec::new());

        let (hard, mut rest): (Vec<usize>, Vec<usize>) =
            (0..events.len()).partition(|&i| is_hard(&events[i]));
        let placed: Vec<(Timestamp, Timestamp)> = hard
            .iter()
            .map(|&i| {
                (
                    fixed_slots[i],
                    later(fixed_slots[i], self.natural(&events[i])),
                )
            })
            .collect();
        let mut wanted = fixed_slots.clone();
        for (i, e) in events.iter().enumerate() {
            if e.fixed.is_none() {
                wanted[i] = e.start;
            }
        }
        rest.sort_by_key(|&i| (wanted[i], events[i].fixed.is_some(), i));
        let mut slots = self.place(events, &rest, &wanted, placed);
        for i in hard {
            slots[i] = fixed_slots[i];
        }
        slots
    }

    /// Gives a slot to each entry of `order`, one after the other. Each entry starts at
    /// its `wanted` time, or at the first free time after it. The entries in `placed` and
    /// the entries before it in `order` keep their times. Other entries get `Timestamp::MIN`.
    fn place(
        &self,
        events: &[Entry],
        order: &[usize],
        wanted: &[Timestamp],
        mut placed: Vec<(Timestamp, Timestamp)>,
    ) -> Vec<Timestamp> {
        let mut slots = vec![Timestamp::MIN; events.len()];
        for &i in order {
            let len = self.natural(&events[i]);
            let mut t = wanted[i];
            // Each pass moves `t` past at least one placed entry, so this ends.
            for _ in 0..=placed.len() {
                let end = later(t, len);
                match placed
                    .iter()
                    .filter(|(s, e)| *s < end && t < *e)
                    .map(|(_, e)| *e)
                    .max()
                {
                    Some(e) => t = e,
                    None => break,
                }
            }
            placed.push((t, later(t, len)));
            slots[i] = t;
        }
        slots
    }

    /// Fill-pool items that may play at `at` after `prev`.
    fn fill_pool(
        &self,
        sim: &PlayHistory,
        at: Timestamp,
        prev: Option<&Entry>,
    ) -> Vec<(&'a Item, &'a Playlist)> {
        let station: &'a Station = self.station;
        station
            .playlists
            .iter()
            .filter(|p| p.enabled && p.is_fill_pool)
            .flat_map(|p| p.items.iter().map(move |i| (i, p)))
            .filter(|(item, p)| {
                item_ok(item, at, sim, &self.tz)
                    && p.constraints
                        .iter()
                        .all(|c| allowed(c, item, p.id, at, prev, sim, &self.tz))
            })
            .collect()
    }
}

/// One fixed event being fitted: the material before it is `seg`, starting at `base`.
struct Fit<'f, 'a> {
    f: &'f Fitter<'a>,
    /// Plays already placed before `seg[head]`.
    sim: &'f PlayHistory,
    base: Timestamp,
    /// `seg[..head]` is already recorded in `sim`.
    head: usize,
    ev: &'f Entry,
    target: Timestamp,
    hard: bool,
}

/// A place in the order the reorder step produces or started from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tok {
    Win(usize),
    Event,
    Next,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Free,
    /// Never selected.
    Banned,
    /// Always kept before the event.
    Forced,
}

/// The token before `t` in `list`.
fn prev_of(list: &[Tok], t: Tok) -> Option<Tok> {
    let pos = list.iter().position(|x| *x == t)?;
    list.get(pos.checked_sub(1)?).copied()
}

impl Fit<'_, '_> {
    /// When the event would be reached with `seg` followed by the kept `window` entries.
    fn reach(&self, seg: &[Entry], window: &[(Entry, bool)], keep: &[bool]) -> Timestamp {
        let mut full = seg.to_vec();
        full.extend(
            window
                .iter()
                .zip(keep)
                .filter(|(_, k)| **k)
                .map(|((e, _), _)| e.clone()),
        );
        self.f.arrival(self.base, &full, self.ev)
    }

    /// `select_before`, then once more with the gap corrected by how far the real arrival
    /// (order-dependent lengths: the last entry before a `Hard` event plays to its end)
    /// missed the plan. Keeps the plan whose real arrival is nearer the target.
    fn two_pass(
        &self,
        seg: &[Entry],
        window: &[(Entry, bool)],
        cands: &[(Duration, bool, bool)],
        cursor: Timestamp,
        gap: Duration,
    ) -> Vec<bool> {
        let (picked, leftover) = select_before(gap, cands);
        let first = mask(window.len(), &picked);
        let real = self.reach(seg, window, &first);
        let plan = later(cursor, gap)
            .checked_sub(leftover)
            .unwrap_or(Timestamp::MIN);
        let drift = real.duration_since(plan);
        if drift.unsigned_abs() <= RESOLUTION {
            return first;
        }
        let corrected = SignedDuration::try_from(gap)
            .ok()
            .and_then(|g| g.checked_sub(drift))
            .filter(|g| g.is_positive())
            .map_or(Duration::ZERO, |g| g.unsigned_abs());
        let (picked, _) = select_before(corrected, cands);
        let second = mask(window.len(), &picked);
        let real2 = self.reach(seg, window, &second);
        if apart(real2, self.target) < apart(real, self.target) {
            second
        } else {
            first
        }
    }

    /// Chooses which `window` entries play before the event. The order it makes is checked
    /// against the rotation rules: every pulled-forward entry, and every entry whose
    /// neighbour before it changed, the event's displaced entries and `next` (the flow entry
    /// after the window) included. A failure bans the offending choice (or forces a displaced
    /// entry to stay before the event) and searches again, at most `RETRIES` times; then
    /// the original order.
    fn choose(
        &self,
        seg: &[Entry],
        window: &[(Entry, bool)],
        next: Option<&Entry>,
        gap: Duration,
    ) -> Vec<bool> {
        let f = self.f;
        let cursor = f.arrival(self.base, seg, window.first().map_or(self.ev, |(e, _)| e));
        let n = window.len();
        let next_tok = next.map(|_| Tok::Next);
        let original: Vec<Tok> = (0..n)
            .filter(|&j| window[j].1)
            .map(Tok::Win)
            .chain([Tok::Event])
            .chain((0..n).filter(|&j| !window[j].1).map(Tok::Win))
            .chain(next_tok)
            .collect();
        let token_index = |t: Tok| match t {
            Tok::Win(j) => j,
            Tok::Event => n,
            Tok::Next => n + 1,
        };
        let mut original_prev = vec![None; n + 2];
        for (pos, &t) in original.iter().enumerate() {
            original_prev[token_index(t)] = pos.checked_sub(1).map(|i| original[i]);
        }
        let mut picks = vec![Pick::Free; n];
        for _ in 0..=RETRIES {
            let cands: Vec<_> = window
                .iter()
                .zip(&picks)
                .map(|((e, before), pick)| {
                    // Banned looks like an unmovable later entry, forced like an earlier one.
                    let (movable, before) = match pick {
                        Pick::Banned => (false, false),
                        Pick::Forced => (false, true),
                        Pick::Free => (f.movable(e), *before),
                    };
                    (f.natural(e), movable, before)
                })
                .collect();
            let keep = self.two_pass(seg, window, &cands, cursor, gap);
            let order: Vec<Tok> = (0..n)
                .filter(|&j| keep[j])
                .map(Tok::Win)
                .chain([Tok::Event])
                .chain((0..n).filter(|&j| !keep[j]).map(Tok::Win))
                .chain(next_tok)
                .collect();
            let mut full = seg.to_vec();
            full.extend(order.iter().filter_map(|t| match t {
                Tok::Win(j) => Some(window[*j].0.clone()),
                Tok::Event => Some(self.ev.clone()),
                Tok::Next => next.cloned(),
            }));
            let (starts, _) = f.times(self.base, &full, None);
            let mut check = vec![false; full.len()];
            for (pos, &t) in order.iter().enumerate() {
                let pulled = matches!(t, Tok::Win(j) if keep[j] && !window[j].1);
                let rewired = pos.checked_sub(1).map(|i| order[i]) != original_prev[token_index(t)];
                check[seg.len() + pos] = t != Tok::Event && (pulled || rewired);
            }
            let h = self.head.min(full.len());
            let Some(bad) = f.first_violation(
                self.sim,
                full[..h].last(),
                &full[h..],
                &starts[h..],
                &check[h..],
            ) else {
                return keep;
            };
            let Some(&tok) = (bad + h)
                .checked_sub(seg.len())
                .and_then(|pos| order.get(pos))
            else {
                break;
            };
            let free_movable = |j: usize| picks[j] == Pick::Free && f.movable(&window[j].0);
            match (tok, prev_of(&order, tok)) {
                // Kept but rejected: ban it, or the movable entry now before it.
                (Tok::Win(j), _) if keep[j] && free_movable(j) => picks[j] = Pick::Banned,
                (Tok::Win(j), Some(Tok::Win(p))) if keep[j] && keep[p] && free_movable(p) => {
                    picks[p] = Pick::Banned
                }
                // Displaced and rejected after the event, or the entry it now leads into:
                // keep the displaced original-before entry in front of the event instead.
                (Tok::Win(j), _) if !keep[j] && window[j].1 && free_movable(j) => {
                    picks[j] = Pick::Forced
                }
                (Tok::Win(_) | Tok::Next, Some(Tok::Win(p)))
                    if !keep[p] && window[p].1 && free_movable(p) =>
                {
                    picks[p] = Pick::Forced
                }
                _ => break,
            }
        }
        window.iter().map(|(_, before)| *before).collect()
    }

    /// Fill entries for the gap before the event, checked against the rotation rules after
    /// `seg` and after each other. A rejected item is banned and the search re-run.
    ///
    /// Before a `Hard` event an early finish is dead air that nothing later can close, while
    /// a late one is faded to the target by the trim step. So when the nearest fill (or no
    /// fill) leaves more than [`OVERRUN_DEAD_AIR`] of dead air, a fill that runs over by about
    /// as much is tried too. It wins only if its last item, faded at the target, still plays
    /// at least [`OVERRUN_MIN_PLAY`] and [`OVERRUN_MIN_SHARE_PERCENT`] of its length.
    fn fill(&self, seg: &[Entry]) -> Option<Vec<Entry>> {
        let f = self.f;
        let arr = f.arrival(self.base, seg, self.ev);
        if arr >= self.target {
            return None;
        }
        // Followed by fill, the last entry hands over at its segue rather than playing to its
        // end as it does before a hard event, so the fill starts there.
        let start = seg.last().map_or(arr, |last| {
            let ordinary = Entry {
                fixed: None,
                ..last.clone()
            };
            f.arrival(self.base, seg, &ordinary)
        });
        let gap = apart(self.target, arr);
        let leftover = apart(self.target, start);
        let nearest = self.fill_for(seg, start, leftover);
        // The dead air the nearest fill leaves, or the whole gap without one.
        let early = match &nearest {
            Some((_, reached)) if *reached < self.target => apart(self.target, *reached),
            Some(_) => Duration::ZERO,
            None => leftover,
        };
        if self.hard && early > OVERRUN_DEAD_AIR {
            let over = self.fill_for(seg, start, leftover.saturating_add(early));
            if let Some((fills, reached)) = over
                && reached >= self.target
                && fills.last().is_some_and(|last| {
                    let len = f.step(last, Some(self.ev));
                    let play = len.saturating_sub(apart(reached, self.target));
                    let share = len * OVERRUN_MIN_SHARE_PERCENT / 100;
                    f.trimmable(last) && play >= OVERRUN_MIN_PLAY.max(share).max(MIN_PLAY)
                })
            {
                return Some(fills);
            }
        }
        // Keep the fill only if it brings the event closer to its time.
        nearest
            .filter(|(_, reached)| apart(*reached, self.target) < gap)
            .map(|(fills, _)| fills)
    }

    /// The fill nearest to `leftover` from `start`, with when it hands over to the event.
    fn fill_for(
        &self,
        seg: &[Entry],
        start: Timestamp,
        leftover: Duration,
    ) -> Option<(Vec<Entry>, Timestamp)> {
        let f = self.f;
        let pool = f.fill_pool(self.sim, start, seg.last());
        let mut banned = vec![false; pool.len()];
        for _ in 0..=RETRIES {
            let cands: Vec<_> = pool
                .iter()
                .zip(&banned)
                .map(|((item, _), &ban)| (advance(item, Some(item)), !ban, false))
                .collect();
            let (picked, _) = select_before(leftover, &cands);
            if picked.is_empty() {
                return None;
            }
            let fills: Vec<Entry> = picked
                .iter()
                .map(|&j| {
                    let (item, pl) = pool[j];
                    Entry {
                        playlist: pl.id,
                        item: item.id,
                        start,
                        len: radio_timing::item_len(item),
                        origin: Origin::Fill,
                        locked: false,
                        fixed: None,
                        trim: Duration::ZERO,
                    }
                })
                .collect();
            let mut with = seg.to_vec();
            with.extend(fills.iter().cloned());
            let (starts, reached) = f.times(self.base, &with, Some(self.ev));
            let h = self.head.min(with.len());
            let check: Vec<bool> = (h..with.len()).map(|i| i >= seg.len()).collect();
            match f.first_violation(self.sim, with[..h].last(), &with[h..], &starts[h..], &check) {
                Some(bad) => match (bad + h).checked_sub(seg.len()) {
                    Some(k) if k < picked.len() => banned[picked[k]] = true,
                    _ => return None,
                },
                None => {
                    // A hard event must not be made late by an item that cannot be faded.
                    let overshoots = self.hard
                        && reached > self.target
                        && fills.last().is_some_and(|e| !f.trimmable(e));
                    match picked.last() {
                        Some(&last) if overshoots => banned[last] = true,
                        _ => return Some((fills, reached)),
                    }
                }
            }
        }
        None
    }
}

/// Drops each rule-inserted, unanchored entry of `entries` (in time order) that breaks an
/// adjacency rule against the entry before it. Used where a step outside the fitter takes
/// entries out from between rule insertions.
pub(crate) fn drop_adjacent_clashes(
    entries: &mut Vec<Entry>,
    station: &Station,
    warnings: &mut Vec<Warning>,
) {
    let f = Fitter {
        station,
        needs_history: false,
        tz: TimeZone::get(&station.tz).unwrap_or(TimeZone::UTC),
        items: station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| (i.id, i)))
            .collect(),
    };
    let mut kept: Vec<Entry> = Vec::with_capacity(entries.len());
    for e in entries.drain(..) {
        if droppable_insertion(&e) && kept.last().is_some_and(|prev| !f.may_follow(prev, &e)) {
            warnings.push(drop_warning(&e, DropReason::AdjacencyAfterReplan));
            continue;
        }
        kept.push(e);
    }
    *entries = kept;
}

/// Fits every fixed entry of `tl` onto its target, in this order (spec §6.3):
///
/// 1. **Reorder** the entries between the previous fixed or locked entry (or at most
///    [`MAX_GAP`] before) and this one, plus up to ten after it, with [`select_before`].
///    Locked and non-movable entries stay where they are. Pulled-forward entries and new
///    neighbours are checked against their playlists' rotation rules.
/// 2. **Fill** a remaining gap from the enabled fill-pool playlists, checked with the
///    playlists' constraints and item validity.
/// 3. **Trim**: before a `Hard` event the entry on air is faded to finish at the target
///    (an entry that would be left under 1 s is moved after the event instead).
///    Before a `Soft` event it is faded early by at most 30 s, and only when the event
///    would be later than its tolerance. Locked and non-trimmable items (ads) never are.
/// 4. **Alarm**: outside tolerance gives `Warning::OutsideTolerance`. Silence before any
///    entry gives `Warning::DeadAir`.
///
/// A fixed entry with `frozen` set skips the reorder and moves nothing across its window's
/// boundaries: it only fills and trims, taking the entries before it as they are. The window
/// of the event just before a frozen one pulls nothing from it, and what that event displaces
/// waits until the frozen window is past.
///
/// Locked entries are anchors fitted the same way, as a hard event with zero tolerance at
/// their own start (handing over at the previous entry's segue). Material that does not fit
/// before one moves after it. Only a non-trimmable item in the way makes a locked entry
/// start late, with `Warning::LockedMoved`.
///
/// A `Hard` event beats a locked entry. A locked entry that loses its slot to it starts
/// right after it, with `Warning::LockedMoved`. A `Soft` event starts after the locked
/// entry in its way, with `Warning::FixedConflict`. The locked entries after that start
/// after the `Soft` event, with `Warning::LockedMoved`.
///
/// Overlapping fixed entries: the higher rank keeps its time (then the earlier target, then
/// the earlier listed); the other starts right after it and gets `Warning::FixedConflict`.
/// It is not fitted itself.
///
/// Afterwards every start is recomputed: an entry starts at the previous one's segue, or
/// at its end before a hard fixed entry. Anchors never start early: a `Hard` entry starts at
/// its target (later only after an untrimmable item), a `Soft` one no earlier than its
/// target less its tolerance, a locked one at its own start. `FitResult::planned` is when
/// the material before the event hands over to it; `error` is that less the target.
pub fn fit_all(tl: &mut Timeline, station: &Station, hist: &PlayHistory) {
    let Some(t0) = tl.entries.iter().map(|e| e.start).min() else {
        return;
    };
    let tz = TimeZone::get(&station.tz).unwrap_or_else(|_| {
        let warning = Warning::UnknownTimeZone {
            tz: station.tz.clone(),
        };
        if !tl.warnings.contains(&warning) {
            tl.warnings.push(warning);
        }
        TimeZone::UTC
    });
    let f = Fitter {
        station,
        needs_history: station.playlists.iter().any(|p| {
            p.items.iter().any(|i| i.max_per_day.is_some())
                || p.constraints
                    .iter()
                    .chain(p.rules.iter().flat_map(|r| &r.constraints))
                    .any(|c| {
                        matches!(
                            c,
                            Constraint::MinGapSameItem(_)
                                | Constraint::ArtistSeparation(_)
                                | Constraint::TitleSeparation(_)
                                | Constraint::MaxPerHour(_)
                                | Constraint::MaxPerDay(_)
                        )
                    })
        }),
        tz,
        items: station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| (i.id, i)))
            .collect(),
    };
    // Locked entries are anchors too: fitted like a hard event at their own start.
    let (events, flow): (Vec<Entry>, Vec<Entry>) = tl
        .entries
        .drain(..)
        .partition(|e| e.fixed.is_some() || e.locked);
    let mut flow = VecDeque::from(flow);
    // Entries displaced by the event before a frozen window wait here until that window is past.
    let mut deferred: VecDeque<Entry> = VecDeque::new();
    // Rule insertions taken out to keep adjacency rules, reported once the fit is done.
    let mut dropped: Vec<Warning> = Vec::new();
    let slots = f.resolve(&events);
    let mut order: Vec<usize> = (0..events.len()).collect();
    order.sort_by_key(|&i| (slots[i], i));

    let mut sim = if f.needs_history {
        hist.clone()
    } else {
        PlayHistory::new()
    };
    let mut out: Vec<Entry> = Vec::with_capacity(events.len() + flow.len());
    for (k, &i) in order.iter().enumerate() {
        let mut trim_moved = false;
        let ev = &events[i];
        let lock = ev.fixed.is_none();
        let fx = ev.fixed.clone().unwrap_or(Fixed {
            target: ev.start,
            mode: FixMode::Hard,
            tolerance: Duration::ZERO,
            priority: i32::MAX,
            frozen: false,
        });
        let target = fx.target;
        let head = out.pop();
        let base = head.as_ref().map_or(t0, |e| e.start);
        let mut seg: Vec<Entry> = head.into_iter().collect();
        let fit = Fit {
            f: &f,
            sim: &sim,
            base,
            head: seg.len(),
            ev,
            target,
            hard: fx.mode == FixMode::Hard,
        };
        let mut steps = Vec::new();

        // The window ahead of a frozen one may not pull from it or displace into it.
        let next_frozen = order
            .get(k + 1)
            .is_some_and(|&j| events[j].fixed.as_ref().is_some_and(|f| f.frozen));
        let shut = fx.frozen || next_frozen;
        let park = |flow: &mut VecDeque<Entry>,
                    deferred: &mut VecDeque<Entry>,
                    dropped: &mut Vec<Warning>,
                    e: Entry| {
            if next_frozen && !fx.frozen {
                deferred.push_front(e);
            } else {
                f.push_moved(flow, e, dropped);
            }
        };

        if fx.frozen && slots[i] != target {
            // Its slot went elsewhere and the window is not fitted; what was displaced still
            // follows the frozen entries.
            while let Some(e) = deferred.pop_back() {
                f.push_moved(&mut flow, e, &mut dropped);
            }
        }
        if slots[i] == target {
            // Entries that end more than MAX_GAP before the target stay as they are.
            if let Some(front) = flow.front() {
                let mut t = f.arrival(base, &seg, front);
                while let Some(front) = flow.front() {
                    let start = f.anchor(front, t);
                    let end = later(start, f.natural(front));
                    if later(end, MAX_GAP) >= target {
                        break;
                    }
                    seg.extend(flow.pop_front());
                    t = end;
                }
            }
            let mut window: Vec<(Entry, bool)> = Vec::new();
            while flow.front().is_some_and(|e| e.start < target) {
                window.extend(flow.pop_front().map(|e| (e, true)));
            }
            if fx.frozen {
                // The window is past: what the event before it displaced follows it.
                while let Some(e) = deferred.pop_back() {
                    f.push_moved(&mut flow, e, &mut dropped);
                }
            }
            let fixed_len = seg.len();
            let next_slot = order.get(k + 1).map(|&j| slots[j]);
            let mut ahead = 0;
            while !shut
                && ahead < LOOKAHEAD
                && flow
                    .front()
                    .is_some_and(|e| next_slot.is_none_or(|n| e.start < n))
            {
                window.extend(flow.pop_front().map(|e| (e, false)));
                ahead += 1;
            }

            // Reorder.
            let cursor = f.arrival(base, &seg, window.first().map_or(ev, |(e, _)| e));
            let gap = if cursor < target {
                apart(target, cursor)
            } else {
                Duration::ZERO
            };
            // A frozen window keeps its order: nothing moves across its boundaries.
            let keep = if fx.frozen {
                window.iter().map(|(_, before)| *before).collect()
            } else {
                fit.choose(&seg, &window, flow.front(), gap)
            };
            let mut moved = window
                .iter()
                .zip(&keep)
                .filter(|((_, before), kept)| before != *kept)
                .count();
            let mut rest = Vec::new();
            for ((e, _), kept) in window.into_iter().zip(keep) {
                if kept { seg.push(e) } else { rest.push(e) }
            }
            for e in rest.into_iter().rev() {
                park(&mut flow, &mut deferred, &mut dropped, e);
            }

            // Fill.
            if let Some(fills) = fit.fill(&seg) {
                seg.extend(fills);
            }

            // Trim.
            let mut trimmed = None;
            if f.arrival(base, &seg, ev) > target {
                if fx.mode == FixMode::Hard && !fx.frozen {
                    // What would start at or after the target goes after the event instead.
                    while seg.len() > fixed_len {
                        let (starts, _) = f.times(base, &seg, Some(ev));
                        if starts.last().is_none_or(|s| *s < target) {
                            break;
                        }
                        match seg.last() {
                            Some(e) if e.origin == Origin::Fill => {
                                seg.pop();
                            }
                            Some(e) if f.movable(e) => {
                                if let Some(e) = seg.pop() {
                                    park(&mut flow, &mut deferred, &mut dropped, e);
                                    moved += 1;
                                    trim_moved = true;
                                }
                            }
                            _ => break,
                        }
                    }
                }
                // Fade the last entry; one that would be left under MIN_PLAY goes instead.
                for _ in 0..=seg.len() {
                    let arr = f.arrival(base, &seg, ev);
                    if arr <= target {
                        break;
                    }
                    let late = apart(arr, target);
                    if fx.mode == FixMode::Soft && late <= fx.tolerance {
                        break;
                    }
                    let Some(last) = seg.last().filter(|e| f.trimmable(e)) else {
                        break;
                    };
                    let cap = match fx.mode {
                        FixMode::Hard => Duration::MAX,
                        FixMode::Soft => MAX_TRIM.saturating_sub(last.trim),
                    };
                    let play = f.step(last, Some(ev));
                    let by = late.min(cap).min(play);
                    if play.saturating_sub(by) < MIN_PLAY {
                        // Keeping the anchor's time wins: move the entry after it, or drop
                        // fill. Otherwise fade it down to MIN_PLAY and stop.
                        let droppable =
                            last.origin == Origin::Fill || (f.movable(last) && !fx.frozen);
                        if seg.len() <= fit.head || !droppable {
                            let by = play.saturating_sub(MIN_PLAY);
                            if by > Duration::ZERO
                                && let Some(last) = seg.last_mut()
                            {
                                last.trim = last.trim.saturating_add(by);
                                trimmed = Some(by);
                            }
                            break;
                        }
                        if let Some(e) = seg.pop().filter(|e| e.origin != Origin::Fill) {
                            park(&mut flow, &mut deferred, &mut dropped, e);
                            moved += 1;
                            trim_moved = true;
                        }
                        continue;
                    }
                    if by > Duration::ZERO
                        && let Some(last) = seg.last_mut()
                    {
                        last.trim = last.trim.saturating_add(by);
                        trimmed = Some(by);
                    }
                    break;
                }
            }
            if moved > 0 {
                steps.push(FitStep::Reordered { moved });
            }
            // The hard step above may have taken fill back out, so list what stayed.
            let items: Vec<ItemId> = seg
                .get(fixed_len..)
                .unwrap_or_default()
                .iter()
                .filter(|e| e.origin == Origin::Fill)
                .map(|e| e.item)
                .collect();
            if !items.is_empty() {
                steps.push(FitStep::Filled { items });
            }
            if let Some(by) = trimmed {
                steps.push(FitStep::Trimmed { by });
            }
        }

        let planned = f.arrival(base, &seg, ev);
        let error = planned.duration_since(target);
        let miss = error.unsigned_abs();
        if lock {
            if error.is_positive() {
                tl.warnings.push(Warning::LockedMoved {
                    item: ev.item,
                    by: miss,
                });
            }
        } else if slots[i] != target {
            tl.warnings.push(Warning::FixedConflict {
                playlist: ev.playlist,
                wanted: target,
                got: f.anchor(ev, planned),
            });
        } else if error.is_positive() {
            steps.push(FitStep::LateStart { by: miss });
        } else if error.is_negative() && fx.mode == FixMode::Soft {
            steps.push(FitStep::EarlyStart {
                by: miss.min(fx.tolerance),
            });
        }
        let within_tolerance = miss <= fx.tolerance;
        if !lock && !within_tolerance {
            tl.warnings.push(Warning::OutsideTolerance {
                playlist: ev.playlist,
                error,
            });
        }
        if !lock {
            tl.fits.push(FitResult {
                playlist: ev.playlist,
                target,
                planned,
                error,
                within_tolerance,
                steps,
            });
        }
        let head = fit.head;
        seg.push(ev.clone());
        f.reflow(&mut seg, base);
        for e in &seg[head..] {
            f.record(&mut sim, e, e.start);
        }
        out.extend(seg);
        if trim_moved {
            // Trim can move more entries than the reorder step checked. Choose a valid
            // continuation with the history that now includes the event.
            let mut accepted = Vec::new();
            let next_slot = order.get(k + 1).map(|&j| slots[j]);
            let end = flow
                .iter()
                .position(|e| next_slot.is_some_and(|next| e.start >= next))
                .unwrap_or(flow.len());
            let mut pending: Vec<Entry> = flow.drain(..end).collect();
            let mut history = sim.clone();
            let mut previous = out.last().cloned();
            while !pending.is_empty() {
                let limit = pending
                    .iter()
                    .position(|e| !f.movable(e))
                    .unwrap_or(pending.len());
                let start = |entry: &Entry| {
                    let cursor = previous
                        .as_ref()
                        .map_or(t0, |prev| later(prev.start, f.step(prev, Some(entry))));
                    f.anchor(entry, cursor)
                };
                let choice = if limit == 0 {
                    Some(0)
                } else {
                    (0..limit).find(|&index| {
                        let entry = &pending[index];
                        f.first_violation(
                            &history,
                            previous.as_ref(),
                            std::slice::from_ref(entry),
                            &[start(entry)],
                            &[true],
                        )
                        .is_none()
                    })
                };
                match choice {
                    Some(index) => {
                        let mut entry = pending.remove(index);
                        entry.start = start(&entry);
                        f.record(&mut history, &entry, entry.start);
                        previous = Some(entry.clone());
                        accepted.push(entry);
                    }
                    None => {
                        let entry = pending.remove(0);
                        if droppable_insertion(&entry) {
                            dropped.push(drop_warning(&entry, DropReason::RotationAfterTrim));
                        } else {
                            let mut entry = entry;
                            entry.start = start(&entry);
                            tl.warnings.push(Warning::NoEligibleItem {
                                playlist: entry.playlist,
                                at: entry.start,
                            });
                            f.record(&mut history, &entry, entry.start);
                            previous = Some(entry.clone());
                            accepted.push(entry);
                        }
                    }
                }
            }
            for entry in accepted.into_iter().rev() {
                flow.push_front(entry);
            }
        }
        // What was moved after the event now follows it.
        f.drop_clashing_front(out.last(), &mut flow, &mut dropped);
    }
    while let Some(e) = deferred.pop_back() {
        f.push_moved(&mut flow, e, &mut dropped);
    }
    f.drop_clashing_front(out.last(), &mut flow, &mut dropped);
    out.extend(flow);
    f.reflow(&mut out, t0);
    dead_air(&f, tl, &out, t0);
    tl.warnings.extend(dropped);
    tl.entries = out;
}

/// Replaces the timeline's `DeadAir` warnings with the silences of the re-flowed `out`:
/// before its first entry (from `t0`, or earlier where an old warning began), between any
/// two entries, and after the real end of the last one up to the horizon the old
/// warnings reached.
fn dead_air(f: &Fitter, tl: &mut Timeline, out: &[Entry], t0: Timestamp) {
    let mut old = Vec::new();
    tl.warnings.retain(|w| match w {
        Warning::DeadAir { at, gap } => {
            old.push((*at, later(*at, *gap)));
            false
        }
        _ => true,
    });
    let mut push = |at: Timestamp, until: Timestamp| {
        if until > at {
            tl.warnings.push(Warning::DeadAir {
                at,
                gap: apart(until, at),
            });
        }
    };
    if let Some(first) = out.first() {
        let from = old.iter().map(|o| o.0).min().map_or(t0, |at| at.min(t0));
        push(from, first.start);
    }
    for w in out.windows(2) {
        push(later(w[0].start, f.step(&w[0], Some(&w[1]))), w[1].start);
    }
    if let (Some(last), Some(end)) = (out.last(), old.iter().map(|o| o.1).max()) {
        push(later(last.start, f.step(last, None)), end);
    }
}
