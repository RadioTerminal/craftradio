use jiff::{SignedDuration, Timestamp};
use radio_model::{ItemId, PlaylistId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The time step used by the subset search.
pub const RESOLUTION: Duration = Duration::from_millis(100);

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
/// The search covers the remaining gap plus ten minutes. Equal distances prefer fewer
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
    let limit = (remaining_ns + Duration::from_secs(600).as_nanos()) / step;
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
    let limit = usize::try_from(limit.min(reachable_limit)).unwrap_or(usize::MAX - 1);
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
