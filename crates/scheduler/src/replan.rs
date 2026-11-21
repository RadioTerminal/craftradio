//! `plan`: merge and fit, keeping the previous plan steady where a change is not worth it.

use crate::fit::drop_adjacent_clashes;
use crate::{Entry, FitResult, Origin, PlanInput, Timeline, fit_all, merge};
use jiff::SignedDuration;
use radio_model::{Item, ItemId};
use radio_timing::{item_len, start_times};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanOptions {
    /// A fixed event closer than this to `now` is frozen: only fill and trim apply to it.
    pub lock_before: Duration,
    /// A new order must beat the previous one's error by more than this to replace it.
    pub replan_threshold: Duration,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            lock_before: Duration::from_secs(10 * 60),
            replan_threshold: Duration::from_millis(500),
        }
    }
}

fn is_anchor(e: &Entry) -> bool {
    e.fixed.is_some() || e.locked
}

/// The entries between the previous anchor and the fixed entry that `fit` describes.
fn window_of(entries: &[Entry], fit: &FitResult) -> Option<Range<usize>> {
    let end = entries.iter().position(|e| {
        e.playlist == fit.playlist && e.fixed.as_ref().is_some_and(|f| f.target == fit.target)
    })?;
    let start = entries[..end]
        .iter()
        .rposition(is_anchor)
        .map_or(0, |i| i + 1);
    Some(start..end)
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

fn ids(entries: &[Entry]) -> Vec<ItemId> {
    entries.iter().map(|e| e.item).collect()
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
pub fn plan(input: &PlanInput, opts: &PlanOptions, prev: Option<&Timeline>) -> Timeline {
    let mut raw = merge(input);
    let mut tl = raw.clone();
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

    let mut kept_any = false;
    for fit in &tl.fits {
        let Some(old) = prev
            .fits
            .iter()
            .find(|f| f.playlist == fit.playlist && f.target == fit.target)
        else {
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
        let (Some(old_win), Some(new_win)) =
            (window_of(&prev.entries, fit), window_of(&tl.entries, fit))
        else {
            continue;
        };
        let old_win = &prev.entries[old_win];
        if ids(old_win) == ids(&tl.entries[new_win]) {
            continue;
        }
        let Some(raw_win) = window_of(&raw.entries, fit) else {
            continue;
        };
        let Some(first_start) = raw.entries.get(raw_win.start).map(|e| e.start) else {
            continue;
        };
        let keep: Vec<&Entry> = old_win
            .iter()
            .filter(|e| e.start >= input.now && e.origin != Origin::Manual)
            .filter(|e| items.contains_key(&e.item))
            .collect();
        let seq: Vec<&Item> = keep
            .iter()
            .filter_map(|e| items.get(&e.item).copied())
            .collect();
        let starts = start_times(first_start, &seq);
        // What would start at or after the event is no longer part of the window.
        let n = starts.iter().take_while(|s| **s < fit.target).count();
        let forced: Vec<Entry> = keep
            .iter()
            .zip(&seq)
            .zip(&starts)
            .take(n)
            .map(|((e, item), start)| {
                let mut e = (*e).clone();
                e.start = *start;
                e.len = item_len(item);
                e.trim = Duration::ZERO;
                e
            })
            .collect();
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
        raw.entries.splice(raw_win.clone(), forced);
        let mut head = raw_win.start;
        let mut ev = head + forced_len;
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
                raw.entries.remove(d);
                if d < head {
                    head -= 1;
                    ev -= 1;
                }
            }
        }
        let Some(event) = raw.entries.get_mut(ev) else {
            continue;
        };
        if let Some(f) = event.fixed.as_mut() {
            f.frozen = true;
        }
        let after = event.start;
        for (k, mut e) in leftover.drain(..).enumerate() {
            e.start = after
                .checked_add(Duration::from_millis(k as u64 + 1))
                .unwrap_or(after);
            raw.entries.push(e);
        }
        raw.entries.sort_by_key(|e| e.start);
        kept_any = true;
    }
    if !kept_any {
        return tl;
    }
    // Taking out the duplicates can leave two rule insertions side by side.
    drop_adjacent_clashes(&mut raw.entries, input.station);
    fit_all(&mut raw, input.station, input.history);
    // `frozen` is a planning-time flag; it does not outlive the plan.
    for f in raw.entries.iter_mut().filter_map(|e| e.fixed.as_mut()) {
        f.frozen = false;
    }
    raw
}
