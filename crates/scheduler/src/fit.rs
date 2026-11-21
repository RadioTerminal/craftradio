use jiff::SignedDuration;
use std::{collections::BTreeMap, time::Duration};

/// The time step used by the subset search.
pub const RESOLUTION: Duration = Duration::from_millis(100);

#[derive(Clone)]
struct Plan {
    movement_cost: i128,
    indices: Vec<usize>,
}

/// Picks entries for `gap`, preserving their order, and returns the actual time left.
/// A positive remainder means an early finish; a negative remainder means a late finish.
///
/// The interface has no event boundary. For movement costs, the original-before entries
/// are the longest prefix whose total duration does not exceed `gap`. All non-movable
/// entries are mandatory; callers must omit non-movable entries from after the event.
/// Locked entries must be passed as non-movable.
///
/// Movable lengths are rounded to the nearest `RESOLUTION` (half steps round up).
/// The search covers the remaining gap plus ten minutes. Equal distances prefer fewer
/// displacements, then an earlier finish. The remainder uses unrounded durations.
pub fn select_before(gap: Duration, cands: &[(Duration, bool)]) -> (Vec<usize>, SignedDuration) {
    let gap_ns = gap.as_nanos();
    let mut prefix_ns = 0_u128;
    let before = cands
        .iter()
        .take_while(|(length, _)| {
            prefix_ns = prefix_ns.saturating_add(length.as_nanos());
            prefix_ns <= gap_ns
        })
        .count();
    let mandatory: Vec<_> = cands
        .iter()
        .enumerate()
        .filter_map(|(index, (_, movable))| (!movable).then_some(index))
        .collect();
    let mandatory_ns = mandatory.iter().fold(0_u128, |sum, &index| {
        sum.saturating_add(cands[index].0.as_nanos())
    });
    let remaining_ns = gap_ns.saturating_sub(mandatory_ns);
    let step = RESOLUTION.as_nanos();
    let limit = (remaining_ns + Duration::from_secs(600).as_nanos()) / step;
    let mut sums = BTreeMap::from([(
        0_u128,
        Plan {
            movement_cost: 0,
            indices: Vec::new(),
        },
    )]);

    if mandatory_ns <= gap_ns {
        for (index, (length, movable)) in cands.iter().enumerate() {
            if !movable {
                continue;
            }
            let ticks = (length.as_nanos() + step / 2) / step;
            // Read the previous row so each entry can be selected only once.
            let mut next = sums.clone();
            for (&sum, plan) in &sums {
                let total = sum + ticks;
                if total > limit {
                    continue;
                }
                // Dropping every original-before entry is the common baseline.
                let cost = plan.movement_cost + if index < before { -1 } else { 1 };
                if next.get(&total).is_none_or(|old| cost < old.movement_cost) {
                    let mut indices = plan.indices.clone();
                    indices.push(index);
                    next.insert(
                        total,
                        Plan {
                            movement_cost: cost,
                            indices,
                        },
                    );
                }
            }
            sums = next;
        }
    }

    let best = sums.iter().min_by_key(|(ticks, plan)| {
        let sum_ns = **ticks * step;
        (remaining_ns.abs_diff(sum_ns), plan.movement_cost, sum_ns)
    });
    let mut kept = mandatory;
    if let Some((_, plan)) = best {
        kept.extend_from_slice(&plan.indices);
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
