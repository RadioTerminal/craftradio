use jiff::SignedDuration;
use radio_scheduler::select_before;
use std::time::Duration;

#[test]
fn exact_fit_found() {
    let cands = [200, 180, 220, 190].map(|s| (Duration::from_secs(s), true));
    let (kept, leftover) = select_before(Duration::from_secs(600), &cands);
    assert_eq!(kept, [0, 1, 2]);
    assert_eq!(leftover, SignedDuration::ZERO);
}

#[test]
fn prefers_fewer_moves_on_tie() {
    let cands = [400, 300, 200].map(|s| (Duration::from_secs(s), true));
    let (kept, leftover) = select_before(Duration::from_secs(450), &cands);
    // Keeping the original prefix costs no moves; pulling in 300 + 200 costs three.
    assert_eq!(kept, [0]);
    assert!(leftover.is_positive());
    // Equal movement counts prefer the earlier finish.
    let cands = [400, 200, 300].map(|s| (Duration::from_secs(s), true));
    let (kept, leftover) = select_before(Duration::from_secs(250), &cands);
    assert_eq!(kept, [1]);
    assert!(leftover.is_positive());
}

#[test]
fn non_movable_always_kept() {
    let cands = [
        (Duration::from_secs(700), false),
        (Duration::from_secs(100), true),
    ];
    let (kept, leftover) = select_before(Duration::from_secs(600), &cands);
    assert_eq!(kept, [0]);
    assert!(leftover.is_negative());
}
