//! `plan`: merge and fit, keeping the previous plan steady where a change is not worth it.

use crate::{Entry, FitResult, Origin, PlanInput, Timeline, Warning, fit_all, merge};
use jiff::{SignedDuration, Timestamp};
use radio_model::{FixMode, Item, ItemId, PlaylistId};
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

    let mut forced_ids = BTreeSet::new();
    let mut barrier_ids = BTreeSet::new();
    let mut soft_tails: Vec<(PlaylistId, Timestamp, Vec<Entry>)> = Vec::new();
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
            .enumerate()
            .map(|(k, (e, item))| {
                let mut e = (*e).clone();
                e.start = starts[k];
                e.len = item_len(item);
                e.trim = Duration::ZERO;
                // Pinned in place while the hand-over to the next one is before the event;
                // the rest stay free so fill and trim can absorb an overrun.
                let settled = starts.get(k + 1).is_some_and(|next| *next <= fit.target);
                e.locked = e.locked || (k < last && settled);
                e
            })
            .collect();
        let forced_set: BTreeSet<ItemId> = forced.iter().map(|e| e.item).collect();
        // Main items the old window lacked go back as candidates after the event.
        let mut leftover: Vec<Entry> = raw.entries[raw_win.clone()]
            .iter()
            .filter(|e| e.origin == Origin::Main && !is_anchor(e) && !forced_set.contains(&e.item))
            .cloned()
            .collect();
        forced_ids.extend(forced_set.iter().copied());
        let forced_len = forced.len();
        raw.entries.splice(raw_win.clone(), forced);
        let ev = raw_win.start + forced_len;
        // An item the old window kept must not air again later.
        for id in &forced_set {
            let dup = raw.entries[ev + 1..]
                .iter()
                .position(|e| e.origin == Origin::Main && !is_anchor(e) && e.item == *id);
            if let Some(d) = dup {
                raw.entries.remove(ev + 1 + d);
            }
        }
        let mut after = raw.entries.get(ev).map_or(fit.target, |e| e.start);
        let mut tail = None;
        if frozen {
            let hard = raw
                .entries
                .get(ev)
                .and_then(|e| e.fixed.as_ref())
                .is_some_and(|f| f.mode == FixMode::Hard);
            if hard {
                // Stops the fitter pulling entries from behind the event into the window.
                if let Some(next) = raw.entries.get_mut(ev + 1)
                    && !is_anchor(next)
                {
                    next.locked = true;
                    barrier_ids.insert(next.item);
                    after = next.start;
                    forced_ids.insert(next.item);
                }
            } else if raw.entries[ev + 1..].iter().all(|e| !is_anchor(e)) {
                // A soft event hands over at an unknown time: keep what follows out of the fit.
                tail = Some(raw.entries.split_off(ev + 1));
            }
        }
        if let Some(t) = tail {
            soft_tails.push((fit.playlist, fit.target, t));
        }
        for (k, mut e) in leftover.drain(..).enumerate() {
            e.start = after
                .checked_add(Duration::from_millis(k as u64 + 1))
                .unwrap_or(after);
            raw.entries.push(e);
        }
        raw.entries.sort_by_key(|e| e.start);
    }
    if forced_ids.is_empty() {
        return tl;
    }
    fit_all(&mut raw, input.station, input.history);
    for (playlist, target, tail) in soft_tails {
        let Some(ev) = raw.entries.iter().find(|e| {
            e.playlist == playlist && e.fixed.as_ref().is_some_and(|f| f.target == target)
        }) else {
            continue;
        };
        let next = tail.first().and_then(|t| items.get(&t.item).copied());
        let begin = items.get(&ev.item).map_or(ev.start, |item| {
            ev.start
                .checked_add(advance(item, next))
                .unwrap_or(ev.start)
        });
        let seq: Vec<&Item> = tail
            .iter()
            .filter_map(|e| items.get(&e.item).copied())
            .collect();
        if seq.len() == tail.len() {
            for (mut e, start) in tail.into_iter().zip(start_times(begin, &seq)) {
                e.start = start;
                raw.entries.push(e);
            }
        } else {
            raw.entries.extend(tail);
        }
        raw.entries.sort_by_key(|e| e.start);
    }
    let own = |id: &ItemId| {
        !input.pinned.iter().any(|p| p.item == *id) && items.get(id).is_some_and(|i| !i.locked)
    };
    // Locks taken only to hold the kept order are not the user's; never warn about them.
    raw.warnings.retain(|w| {
        !matches!(w, Warning::LockedMoved { item, .. }
            if (forced_ids.contains(item) || barrier_ids.contains(item)) && own(item))
    });
    for e in &mut raw.entries {
        let own = !input.pinned.iter().any(|p| p.item == e.item);
        if own && e.origin != Origin::Manual && forced_ids.contains(&e.item) {
            e.locked = items.get(&e.item).is_some_and(|i| i.locked);
        }
    }
    raw
}
