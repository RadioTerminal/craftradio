//! Time arithmetic and DST-safe local time for the scheduler.

use jiff::Timestamp;
use radio_model::Item;
use std::time::Duration;

/// How long the item sounds from cue_in to cue_out (or planned_len).
pub fn item_len(item: &Item) -> Duration {
    match &item.timing {
        Some(t) => t.cue_out.saturating_sub(t.cue_in),
        None => item.planned_len.unwrap_or(Duration::ZERO),
    }
}

/// Time from `prev` starting to `next` starting: segue − cue_in, or planned_len when
/// there is no timing. With no `next`, `prev` plays to its end (see [`item_len`]); the
/// caller passes `None` when the next start is a hard fixed event.
pub fn advance(prev: &Item, next: Option<&Item>) -> Duration {
    match (&prev.timing, next) {
        (Some(t), Some(_)) => t.segue.saturating_sub(t.cue_in),
        _ => item_len(prev),
    }
}

/// Start time of each item in `seq` when the first starts at `t0`.
pub fn start_times(t0: Timestamp, seq: &[&Item]) -> Vec<Timestamp> {
    let mut at = t0;
    let mut out = Vec::with_capacity(seq.len());
    for (i, item) in seq.iter().enumerate() {
        out.push(at);
        at = at
            .checked_add(advance(item, seq.get(i + 1).copied()))
            .unwrap_or(at);
    }
    out
}
