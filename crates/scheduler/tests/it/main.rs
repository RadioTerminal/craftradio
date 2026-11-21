use jiff::SignedDuration;
use radio_scheduler::select_before;
use std::time::Duration;

#[test]
fn exact_fit_found() {
    let cands = [(200, true), (180, true), (220, true), (190, false)]
        .map(|(s, before)| (Duration::from_secs(s), true, before));
    let (kept, leftover) = select_before(Duration::from_secs(600), &cands);
    assert_eq!(kept, [0, 1, 2]);
    assert_eq!(leftover, SignedDuration::ZERO);
}

#[test]
fn prefers_fewer_moves_on_tie() {
    let cands = [(400, true), (300, false), (200, false)]
        .map(|(s, before)| (Duration::from_secs(s), true, before));
    let (kept, leftover) = select_before(Duration::from_secs(450), &cands);
    // Keeping the original prefix costs no moves; pulling in 300 + 200 costs three.
    assert_eq!(kept, [0]);
    assert!(leftover.is_positive());
    // Equal movement counts prefer the earlier finish.
    let cands = [400, 200, 300].map(|s| (Duration::from_secs(s), true, false));
    let (kept, leftover) = select_before(Duration::from_secs(250), &cands);
    assert_eq!(kept, [1]);
    assert!(leftover.is_positive());
}

#[test]
fn non_movable_always_kept() {
    let cands = [
        (Duration::from_secs(700), false, true),
        (Duration::from_secs(100), true, false),
        (Duration::from_secs(1), false, false),
    ];
    let (kept, leftover) = select_before(Duration::from_secs(600), &cands);
    assert_eq!(kept, [0]);
    assert!(leftover.is_negative());
}

#[test]
fn fewer_moves_can_prefer_a_late_finish() {
    // Both single-item plans miss by 50 s. Keeping the original item costs zero moves.
    let cands = [
        (Duration::from_secs(200), true, false),
        (Duration::from_secs(300), true, true),
    ];
    let (kept, leftover) = select_before(Duration::from_secs(250), &cands);
    assert_eq!(kept, [1]);
    assert!(leftover.is_negative());
}
