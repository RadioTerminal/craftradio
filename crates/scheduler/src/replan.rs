//! `plan`: merge and fit, keeping the previous plan steady where a change is not worth it.

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

fn ids(entries: &[Entry]) -> Vec<ItemId> {
    entries.iter().map(|e| e.item).collect()
}

/// Merges and fits the station's playlists into one timeline from `input.now`.
///
/// With `prev`, each fixed window keeps the previous plan's order unless the new fit's
/// |error| is smaller than the previous |error| by more than `replan_threshold`. A window
/// whose event is within `lock_before` of `now` always keeps the previous order: only fill
/// and trim apply there, and entries that left the station are dropped.
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

    let mut forced_ids = BTreeSet::new();
    for fit in &tl.fits {
        let Some(old) = prev
            .fits
            .iter()
            .find(|f| f.playlist == fit.playlist && f.target == fit.target)
        else {
            continue;
        };
        let frozen = fit.target.duration_since(input.now) <= lock;
        let better = fit
            .error
            .unsigned_abs()
            .saturating_add(opts.replan_threshold)
            < old.error.unsigned_abs();
        if better && !frozen {
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
        let Some(first) = raw.entries.get(raw_win.start) else {
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
        let starts = start_times(first.start, &seq);
        let last = keep.len().saturating_sub(1);
        let forced: Vec<Entry> = keep
            .iter()
            .zip(&seq)
            .zip(starts)
            .enumerate()
            .map(|(k, ((e, item), start))| {
                let mut e = (*e).clone();
                e.start = start;
                e.len = item_len(item);
                e.trim = Duration::ZERO;
                // Pinned in place, except the last one, which stays free to be trimmed.
                e.locked = e.locked || k < last;
                e
            })
            .collect();
        forced_ids.extend(forced.iter().map(|e| e.item));
        let forced_len = forced.len();
        raw.entries.splice(raw_win.clone(), forced);
        if frozen {
            // Stops the fitter pulling entries from behind the event into the window.
            if let Some(next) = raw.entries.get_mut(raw_win.start + forced_len + 1)
                && !is_anchor(next)
            {
                next.locked = true;
                forced_ids.insert(next.item);
            }
        }
        raw.entries.sort_by_key(|e| e.start);
    }
    if forced_ids.is_empty() {
        return tl;
    }
    fit_all(&mut raw, input.station, input.history);
    for e in &mut raw.entries {
        let own = !input.pinned.iter().any(|p| p.item == e.item);
        if own && e.origin != Origin::Manual && forced_ids.contains(&e.item) {
            e.locked = items.get(&e.item).is_some_and(|i| i.locked);
        }
    }
    raw
}
