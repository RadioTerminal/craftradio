//! `plan`: merge and fit, keeping the previous plan steady where a change is not worth it.

use crate::fit::drop_adjacent_clashes;
use crate::merge::merge_with;
use crate::{Entry, FitResult, Origin, PlanInput, Timeline, fit_all, over};
use jiff::{SignedDuration, Timestamp};
use radio_model::{Item, ItemId, PlaylistId};
use radio_timing::{advance, item_len, start_times};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanOptions {
    /// A fixed event closer than this to `now` is frozen: only fill and trim apply to it.
    pub lock_before: Duration,
    /// A new order must beat the previous one's error by more than this to replace it.
    pub replan_threshold: Duration,
    /// Each playlist continues its order after its most recent item in the history or the
    /// pinned entries. When false, each playlist starts at its first item. Off by default.
    pub resume_rotation: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            lock_before: Duration::from_secs(10 * 60),
            replan_threshold: Duration::from_millis(500),
            resume_rotation: false,
        }
    }
}

fn is_anchor(e: &Entry) -> bool {
    e.fixed.is_some() || e.locked
}

type EventKey = (PlaylistId, Timestamp);

/// Anchor positions change only at a splice or a duplicate removal.
struct Windows {
    anchors: Vec<(usize, Option<EventKey>)>,
    events: BTreeMap<EventKey, usize>,
}

impl Windows {
    fn new(entries: &[Entry]) -> Self {
        let mut windows = Self {
            anchors: Vec::new(),
            events: BTreeMap::new(),
        };
        windows.replace(0..0, entries);
        windows
    }

    fn reindex(&mut self) {
        self.events.clear();
        for &(pos, key) in &self.anchors {
            if let Some(key) = key {
                self.events.entry(key).or_insert(pos);
            }
        }
    }

    fn get(&self, fit: &FitResult) -> Option<Range<usize>> {
        let end = *self.events.get(&(fit.playlist, fit.target))?;
        let index = self.anchors.partition_point(|(pos, _)| *pos < end);
        let start = index.checked_sub(1).map_or(0, |i| self.anchors[i].0 + 1);
        Some(start..end)
    }

    fn replace(&mut self, range: Range<usize>, entries: &[Entry]) {
        self.anchors.retain(|(pos, _)| !range.contains(pos));
        for (pos, _) in &mut self.anchors {
            if *pos >= range.end {
                *pos = *pos - range.len() + entries.len();
            }
        }
        self.anchors.extend(
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| is_anchor(e))
                .map(|(i, e)| {
                    (
                        range.start + i,
                        e.fixed.as_ref().map(|f| (e.playlist, f.target)),
                    )
                }),
        );
        self.anchors.sort_by_key(|(pos, _)| *pos);
        self.reindex();
    }

    fn remove(&mut self, removed: &[usize]) {
        for (pos, _) in &mut self.anchors {
            *pos -= removed.partition_point(|removed| *removed < *pos);
        }
        self.reindex();
    }
}

/// The last entry of `before` matching `is_dup` that is not in a kept (frozen) window.
/// The window is the one of the next fixed event: a locked entry inside a frozen window
/// does not end it. An entry with no fixed event after it in `before` is in the window
/// being kept.
fn earlier_dup(before: &[Entry], is_dup: impl Fn(&Entry) -> bool) -> Option<usize> {
    (0..before.len()).rev().find(|&d| {
        is_dup(&before[d])
            && before[d + 1..]
                .iter()
                .find_map(|e| e.fixed.as_ref())
                .is_some_and(|f| !f.frozen)
    })
}

/// Merges and fits the station's playlists into one timeline from `input.now`.
///
/// With `prev`, each fixed window keeps the previous plan's order unless the new fit's
/// |error| is smaller than the previous |error| by more than `replan_threshold`. A window
/// whose event is within `lock_before` of `now` always keeps the previous order: only fill
/// and trim apply there, and entries that left the station are dropped.
///
/// The improvement check compares against the error stored in `prev.fits`, not a re-fit of
/// the old order.
///
/// Entries of a playlist that plays over the music then lie on a song's intro or outro
/// (see [`crate::over`]): they leave `entries` for `over`, and the rest is fitted again.
pub fn plan(input: &PlanInput, opts: &PlanOptions, prev: Option<&Timeline>) -> Timeline {
    if !over::any_over(input.station) {
        return plan_flow(input, opts, prev);
    }
    // The previous plan's over entries count where they were for keeping its order.
    let prev = prev.map(|p| {
        let mut p = p.clone();
        for o in std::mem::take(&mut p.over) {
            let at = p.entries.partition_point(|x| x.start <= o.entry.start);
            p.entries.insert(at, o.entry);
        }
        p
    });
    let mut tl = plan_flow(input, opts, prev.as_ref());
    let look = over::Lookup::new(input.station);
    let placed = over::place(&mut tl, &look);
    if placed.is_empty() {
        return tl;
    }
    refit(&mut tl, input);
    let (keep, went_back) = over::settle(&mut tl, &look, placed);
    if went_back {
        refit(&mut tl, input);
    }
    tl.over = keep;
    over::anchor(&mut tl);
    tl
}

/// Fits `tl` again after entries left or joined its flow. The warnings of the first fit
/// stay; its fit results are replaced.
fn refit(tl: &mut Timeline, input: &PlanInput) {
    let warnings = std::mem::take(&mut tl.warnings);
    tl.fits.clear();
    fit_all(tl, input.station, input.history);
    for w in warnings {
        if !tl.warnings.contains(&w) {
            tl.warnings.push(w);
        }
    }
}

fn plan_flow(input: &PlanInput, opts: &PlanOptions, prev: Option<&Timeline>) -> Timeline {
    let mut raw = merge_with(input, opts.resume_rotation);
    let mut tl = if prev.is_some() {
        raw.clone()
    } else {
        std::mem::replace(
            &mut raw,
            Timeline {
                over: Vec::new(),
                entries: Vec::new(),
                fits: Vec::new(),
                warnings: Vec::new(),
            },
        )
    };
    fit_all(&mut tl, input.station, input.history);
    let Some(prev) = prev else {
        return tl;
    };
    let items: BTreeMap<ItemId, &Item> = input
        .station
        .playlists
        .iter()
        .flat_map(|p| p.items.iter().map(|i| (i.id, i)))
        .collect();
    let lock = SignedDuration::try_from(opts.lock_before).unwrap_or(SignedDuration::MAX);

    let old_windows = Windows::new(&prev.entries);
    let new_windows = Windows::new(&tl.entries);
    let mut raw_windows = Windows::new(&raw.entries);
    let mut old_fits = BTreeMap::new();
    for fit in &prev.fits {
        old_fits.entry((fit.playlist, fit.target)).or_insert(fit);
    }
    let mut kept_any = false;
    for fit in &tl.fits {
        let Some(old) = old_fits.get(&(fit.playlist, fit.target)) else {
            continue;
        };
        let near = fit.target.duration_since(input.now) <= lock;
        let better = fit
            .error
            .unsigned_abs()
            .saturating_add(opts.replan_threshold)
            < old.error.unsigned_abs();
        if better && !near {
            continue;
        }
        let (Some(old_win), Some(new_win)) = (old_windows.get(fit), new_windows.get(fit)) else {
            continue;
        };
        let old_win = &prev.entries[old_win];
        if old_win
            .iter()
            .map(|e| e.item)
            .eq(tl.entries[new_win].iter().map(|e| e.item))
        {
            continue;
        }
        let Some(raw_win) = raw_windows.get(fit) else {
            continue;
        };
        let Some(first_start) = raw.entries.get(raw_win.start).map(|e| e.start) else {
            continue;
        };
        // Pinned entries (on air or on a deck) stay as given, at the head of the window.
        let held: Vec<Entry> = raw.entries[raw_win.clone()]
            .iter()
            .filter(|e| input.pinned.contains(e))
            .cloned()
            .collect();
        // Each held entry stands for the first old entry of its item. A later airing of
        // the same item stays, such as a rule insertion that repeats in the window.
        let mut unmatched: Vec<ItemId> = held.iter().map(|e| e.item).collect();
        let keep: Vec<&Entry> = old_win
            .iter()
            .filter(|e| match unmatched.iter().position(|h| *h == e.item) {
                Some(k) => {
                    unmatched.remove(k);
                    false
                }
                None => true,
            })
            .filter(|e| e.start >= input.now && e.origin != Origin::Manual)
            .filter(|e| items.contains_key(&e.item))
            .collect();
        let seq: Vec<&Item> = keep
            .iter()
            .filter_map(|e| items.get(&e.item).copied())
            .collect();
        let first_start = match held.last() {
            Some(h) => {
                let step = items
                    .get(&h.item)
                    .map_or(h.len, |item| advance(item, seq.first().copied()));
                h.start
                    .checked_add(step)
                    .unwrap_or(h.start)
                    .max(first_start)
            }
            None => first_start,
        };
        let starts = start_times(first_start, &seq);
        // What would start at or after the event is no longer part of the window.
        let n = starts.iter().take_while(|s| **s < fit.target).count();
        let mut forced = held;
        forced.extend(
            keep.iter()
                .zip(&seq)
                .zip(&starts)
                .take(n)
                .map(|((e, item), start)| {
                    let mut e = (*e).clone();
                    e.start = *start;
                    e.len = item_len(item);
                    e.trim = Duration::ZERO;
                    e
                }),
        );
        let forced_set: BTreeSet<ItemId> = forced.iter().map(|e| e.item).collect();
        let raw_ids: BTreeSet<ItemId> = raw.entries[raw_win.clone()]
            .iter()
            .map(|e| e.item)
            .collect();
        // Main items the kept window lacks go back as candidates after the event.
        let mut leftover: Vec<Entry> = raw.entries[raw_win.clone()]
            .iter()
            .filter(|e| e.origin == Origin::Main && !is_anchor(e) && !forced_set.contains(&e.item))
            .cloned()
            .collect();
        let forced_len = forced.len();
        raw_windows.replace(raw_win.clone(), &forced);
        raw.entries.splice(raw_win.clone(), forced);
        let head = raw_win.start;
        let mut ev = head + forced_len;
        let mut removed = Vec::new();
        // An item brought into the window from later must not air there twice. Items the
        // raw window had already were spliced out with it; their later airings are rotation.
        for id in forced_set.difference(&raw_ids) {
            let is_dup = |e: &Entry| e.origin == Origin::Main && !is_anchor(e) && e.item == *id;
            // The nearest later airing, else the nearest earlier one in a window not kept.
            let dup = raw.entries[ev + 1..]
                .iter()
                .position(is_dup)
                .map(|d| ev + 1 + d)
                .or_else(|| earlier_dup(&raw.entries[..head], is_dup));
            if let Some(d) = dup {
                removed.push(d);
            }
        }
        removed.sort_unstable();
        ev -= removed.partition_point(|d| *d < ev);
        raw_windows.remove(&removed);
        if !removed.is_empty() {
            let mut index = 0;
            raw.entries.retain(|_| {
                let keep = removed.binary_search(&index).is_err();
                index += 1;
                keep
            });
        }
        let Some(event) = raw.entries.get_mut(ev) else {
            continue;
        };
        if let Some(f) = event.fixed.as_mut() {
            f.frozen = true;
        }
        let after = event.start;
        for (k, e) in leftover.iter_mut().enumerate() {
            e.start = after
                .checked_add(Duration::from_millis(k as u64 + 1))
                .unwrap_or(after);
        }
        // The old stable sort put existing entries before leftovers at equal times.
        // Merge only the prefix the leftovers overlap; the rest is already ordered.
        if let Some(last) = leftover.last() {
            // The partition points below need entries in start order.
            debug_assert!(
                raw.entries.windows(2).all(|w| w[0].start <= w[1].start),
                "raw.entries is sorted by start"
            );
            let start = raw.entries.partition_point(|e| e.start <= after);
            let end = start + raw.entries[start..].partition_point(|e| e.start <= last.start);
            let mut merged: Vec<Entry> = raw.entries[start..end].to_vec();
            merged.extend(leftover);
            merged.sort_by_key(|e| e.start);
            raw_windows.replace(start..end, &merged);
            raw.entries.splice(start..end, merged);
        }
        kept_any = true;
    }
    if !kept_any {
        return tl;
    }
    // Taking out the duplicates can leave two rule insertions side by side.
    drop_adjacent_clashes(&mut raw.entries, input.station, &mut raw.warnings);
    // If the edits reproduce the fitted candidate exactly, reuse its fit results.
    // The frozen flag controls fitting only and is not part of the returned plan.
    let same_entries = raw.entries.len() == tl.entries.len()
        && raw.entries.iter().zip(&tl.entries).all(|(a, b)| {
            let mut a = a.clone();
            if let Some(f) = &mut a.fixed {
                f.frozen = false;
            }
            a == *b
        });
    if same_entries
        && raw.warnings.is_empty()
        && tl.warnings.is_empty()
        && tl
            .fits
            .iter()
            .all(|f| f.error.is_zero() && f.steps.is_empty())
        && raw
            .entries
            .iter()
            .all(|e| !e.locked && e.origin != Origin::Fill && e.trim.is_zero())
    {
        return tl;
    }
    fit_all(&mut raw, input.station, input.history);
    // `frozen` is a planning-time flag; it does not outlive the plan.
    for f in raw.entries.iter_mut().filter_map(|e| e.fixed.as_mut()) {
        f.frozen = false;
    }
    raw
}
