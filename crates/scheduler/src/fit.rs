use crate::constraints::item_ok;
use crate::{Entry, Origin, PlayHistory, Timeline, Warning, allowed};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use radio_model::{FixMode, Item, ItemId, Playlist, PlaylistId, Station};
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

/// The most a track is faded early to bring a fixed event onto its time.
const MAX_TRIM: Duration = Duration::from_secs(30);

/// How many entries after a fixed event the reorder step may pull in front of it.
const LOOKAHEAD: usize = 10;

fn later(t: Timestamp, d: Duration) -> Timestamp {
    t.checked_add(d).unwrap_or(Timestamp::MAX)
}

fn is_hard(e: &Entry) -> bool {
    e.fixed.as_ref().is_some_and(|f| f.mode == FixMode::Hard)
}

struct Fitter<'a> {
    station: &'a Station,
    hist: &'a PlayHistory,
    tz: TimeZone,
    items: BTreeMap<ItemId, &'a Item>,
}

impl<'a> Fitter<'a> {
    fn item(&self, e: &Entry) -> Option<&'a Item> {
        self.items.get(&e.item).copied()
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

    fn movable(&self, e: &Entry) -> bool {
        e.fixed.is_none() && !e.locked && self.item(e).is_some_and(|i| i.movable)
    }

    fn trimmable(&self, e: &Entry) -> bool {
        e.fixed.is_none() && !e.locked && self.item(e).is_some_and(|i| i.trimmable)
    }

    fn priority(&self, e: &Entry) -> i32 {
        self.station
            .playlists
            .iter()
            .find(|p| p.id == e.playlist)
            .map_or_else(
                || e.fixed.as_ref().map_or(0, |f| f.priority),
                |p| p.priority,
            )
    }

    /// When `then` would start if it followed `seq` played back to back, `seq[0]` starting at `t`.
    fn arrival(&self, t: Timestamp, seq: &[Entry], then: &Entry) -> Timestamp {
        seq.iter().enumerate().fold(t, |t, (i, e)| {
            later(t, self.step(e, Some(seq.get(i + 1).unwrap_or(then))))
        })
    }

    /// Sets the start of every entry, the first at `t`. Hard fixed entries never start
    /// before their target; everything else starts when the entry before it hands over.
    fn reflow(&self, entries: &mut [Entry], mut t: Timestamp) {
        for i in 0..entries.len() {
            let start = match &entries[i].fixed {
                Some(f) if f.mode == FixMode::Hard => t.max(f.target),
                _ => t,
            };
            entries[i].start = start;
            t = later(start, self.step(&entries[i], entries.get(i + 1)));
        }
    }

    /// The start slot of each fixed entry. Higher priority is placed first, then the earlier
    /// target, then the earlier listed. An entry whose time is taken moves to the first free
    /// time after the entries in its way.
    fn resolve(&self, events: &[Entry]) -> Vec<Timestamp> {
        let target = |e: &Entry| e.fixed.as_ref().map_or(e.start, |f| f.target);
        let mut order: Vec<usize> = (0..events.len()).collect();
        order.sort_by_key(|&i| (Reverse(self.priority(&events[i])), target(&events[i]), i));
        let mut placed: Vec<(Timestamp, Timestamp)> = Vec::with_capacity(events.len());
        let mut slots = vec![Timestamp::MIN; events.len()];
        for i in order {
            let len = self.natural(&events[i]);
            let mut t = target(&events[i]);
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
    fn fill_pool(&self, at: Timestamp, prev: Option<&Entry>) -> Vec<(&'a Item, &'a Playlist)> {
        let station: &'a Station = self.station;
        station
            .playlists
            .iter()
            .filter(|p| p.enabled && p.is_fill_pool)
            .flat_map(|p| p.items.iter().map(move |i| (i, p)))
            .filter(|(item, p)| {
                item_ok(item, at, self.hist, &self.tz)
                    && p.constraints
                        .iter()
                        .all(|c| allowed(c, item, p.id, at, prev, self.hist, &self.tz))
            })
            .collect()
    }
}

/// Fits every fixed entry of `tl` onto its target, in this order (spec §6.3):
///
/// 1. **Reorder** the entries between the previous fixed entry (or at most [`MAX_GAP`]
///    before) and this one, plus up to ten after it, with [`select_before`]. Locked and
///    non-movable entries stay where they are.
/// 2. **Fill** a remaining gap from the enabled fill-pool playlists, checked with the
///    playlists' constraints and item validity.
/// 3. **Trim**: when late, fade the last entry early (at most 30 s; never a locked or
///    non-trimmable item such as an ad). Before a `Hard` event the fade finishes at the
///    target. A `Soft` event late within its tolerance starts late instead of trimming.
/// 4. **Alarm**: outside tolerance gives `Warning::OutsideTolerance`, and a `Hard` event
///    with silence before it also gives `Warning::DeadAir` with the length of the silence.
///
/// Overlapping fixed entries: the higher playlist priority keeps its time (then the
/// earlier target, then the earlier listed); the other starts right after it and gets
/// `Warning::FixedConflict`. It is not fitted itself.
///
/// Afterwards every start is recomputed, so nothing overlaps a fixed entry: an entry
/// starts at the previous one's segue, or at its end before a hard fixed entry. A `Hard`
/// entry starts at its target, or later when what precedes it cannot be cut in time. A
/// `Soft` entry starts where the material before it hands over. `FitResult::planned` is
/// that hand-over time for both modes.
pub fn fit_all(tl: &mut Timeline, station: &Station, hist: &PlayHistory) {
    let Some(t0) = tl.entries.iter().map(|e| e.start).min() else {
        return;
    };
    let f = Fitter {
        station,
        hist,
        tz: TimeZone::get(&station.tz).unwrap_or(TimeZone::UTC),
        items: station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| (i.id, i)))
            .collect(),
    };
    let (events, flow): (Vec<Entry>, Vec<Entry>) =
        tl.entries.drain(..).partition(|e| e.fixed.is_some());
    let mut flow = VecDeque::from(flow);
    if events.is_empty() {
        tl.entries = flow.into();
        return;
    }
    let slots = f.resolve(&events);
    let mut order: Vec<usize> = (0..events.len()).collect();
    order.sort_by_key(|&i| (slots[i], i));

    let mut out: Vec<Entry> = Vec::with_capacity(events.len() + flow.len());
    for (k, &i) in order.iter().enumerate() {
        let ev = events[i].clone();
        let Some(fx) = ev.fixed.clone() else {
            continue;
        };
        let target = fx.target;
        let head = out.pop();
        let base = head.as_ref().map_or(t0, |e| e.start);
        let mut seg: Vec<Entry> = head.into_iter().collect();
        let mut steps = Vec::new();

        if slots[i] == target {
            // Entries that end more than MAX_GAP before the target stay as they are.
            while let Some(front) = flow.front() {
                let at = f.arrival(base, &seg, front);
                if later(later(at, f.natural(front)), MAX_GAP) >= target {
                    break;
                }
                seg.extend(flow.pop_front());
            }
            let mut window: Vec<(Entry, bool)> = Vec::new();
            while flow.front().is_some_and(|e| e.start < target) {
                window.extend(flow.pop_front().map(|e| (e, true)));
            }
            let next_slot = order.get(k + 1).map(|&j| slots[j]);
            let mut ahead = 0;
            while ahead < LOOKAHEAD
                && flow
                    .front()
                    .is_some_and(|e| next_slot.is_none_or(|n| e.start < n))
            {
                window.extend(flow.pop_front().map(|e| (e, false)));
                ahead += 1;
            }
            let cursor = f.arrival(base, &seg, window.first().map_or(&ev, |(e, _)| e));
            let gap = if cursor < target {
                target.duration_since(cursor).unsigned_abs()
            } else {
                Duration::ZERO
            };
            let cands: Vec<_> = window
                .iter()
                .map(|(e, before)| (f.natural(e), f.movable(e), *before))
                .collect();
            let (kept, _) = select_before(gap, &cands);
            let mut keep = vec![false; window.len()];
            for &j in &kept {
                keep[j] = true;
            }
            let moved = window
                .iter()
                .zip(&keep)
                .filter(|((_, before), kept)| before != *kept)
                .count();
            if moved > 0 {
                steps.push(FitStep::Reordered { moved });
            }
            let mut rest = Vec::new();
            for ((e, _), kept) in window.into_iter().zip(keep) {
                if kept { seg.push(e) } else { rest.push(e) }
            }
            for e in rest.into_iter().rev() {
                flow.push_front(e);
            }

            // Fill.
            let arr = f.arrival(base, &seg, &ev);
            if arr < target {
                let leftover = target.duration_since(arr).unsigned_abs();
                let pool = f.fill_pool(arr, seg.last());
                let cands: Vec<_> = pool
                    .iter()
                    .map(|(item, _)| (advance(item, Some(item)), true, false))
                    .collect();
                let (picked, _) = select_before(leftover, &cands);
                let fills: Vec<Entry> = picked
                    .iter()
                    .map(|&j| {
                        let (item, pl) = pool[j];
                        Entry {
                            playlist: pl.id,
                            item: item.id,
                            start: arr,
                            len: radio_timing::item_len(item),
                            origin: Origin::Fill,
                            locked: false,
                            fixed: None,
                            trim: Duration::ZERO,
                        }
                    })
                    .collect();
                if !fills.is_empty() {
                    let mut with = seg.clone();
                    with.extend(fills.iter().cloned());
                    let new_arr = f.arrival(base, &with, &ev);
                    // Keep the fill only if it brings the event closer to its time.
                    if new_arr.duration_since(target).unsigned_abs() < leftover {
                        steps.push(FitStep::Filled {
                            items: fills.iter().map(|e| e.item).collect(),
                        });
                        seg = with;
                    }
                }
            }

            // Trim.
            let arr = f.arrival(base, &seg, &ev);
            if arr > target {
                let late = arr.duration_since(target).unsigned_abs();
                let wait = fx.mode == FixMode::Soft && late <= fx.tolerance;
                if let Some(last) = seg.last().filter(|e| !wait && f.trimmable(e)) {
                    let room = MAX_TRIM
                        .saturating_sub(last.trim)
                        .min(f.step(last, Some(&ev)));
                    let by = late.min(room);
                    if by > Duration::ZERO {
                        if let Some(last) = seg.last_mut() {
                            last.trim = last.trim.saturating_add(by);
                        }
                        steps.push(FitStep::Trimmed { by });
                    }
                }
            }
        }

        let planned = f.arrival(base, &seg, &ev);
        let error = planned.duration_since(target);
        let miss = error.unsigned_abs();
        if slots[i] != target {
            tl.warnings.push(Warning::FixedConflict {
                playlist: ev.playlist,
                wanted: target,
                got: match fx.mode {
                    FixMode::Hard => planned.max(target),
                    FixMode::Soft => planned,
                },
            });
        } else if fx.mode == FixMode::Soft && miss > Duration::ZERO {
            steps.push(if error.is_positive() {
                FitStep::LateStart { by: miss }
            } else {
                FitStep::EarlyStart { by: miss }
            });
        }
        let within_tolerance = miss <= fx.tolerance;
        if !within_tolerance {
            tl.warnings.push(Warning::OutsideTolerance {
                playlist: ev.playlist,
                error,
            });
            if fx.mode == FixMode::Hard && error.is_negative() {
                tl.warnings.push(Warning::DeadAir {
                    at: planned,
                    gap: miss,
                });
            }
        }
        tl.fits.push(FitResult {
            playlist: ev.playlist,
            target,
            planned,
            error,
            within_tolerance,
            steps,
        });
        seg.push(ev);
        f.reflow(&mut seg, base);
        out.extend(seg);
    }
    out.extend(flow);
    f.reflow(&mut out, t0);
    tl.entries = out;
}
