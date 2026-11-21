use jiff::Timestamp;
use radio_testkit::{jingle, live, track};
use radio_timing::{advance, start_times};
use std::time::Duration;

#[test]
fn advance_uses_segue() {
    let a = track(1, "a", "x", 200);
    let b = track(2, "b", "y", 180);
    assert_eq!(advance(&a, Some(&b)), Duration::from_secs(198));
}

#[test]
fn advance_last_item_plays_to_cue_out() {
    assert_eq!(
        advance(&track(1, "a", "x", 200), None),
        Duration::from_secs(200)
    );
}

#[test]
fn advance_live_uses_planned_len() {
    assert_eq!(advance(&live(3, 3600), None), Duration::from_secs(3600));
}

#[test]
fn start_times_accumulate() {
    let t0: Timestamp = "2026-11-21T14:00:00Z".parse().unwrap();
    let (a, b, j) = (
        track(1, "a", "x", 200),
        track(2, "b", "y", 180),
        jingle(3, 6),
    );
    let got = start_times(t0, &[&a, &b, &j]);
    let want: Vec<Timestamp> = [
        "2026-11-21T14:00:00Z",
        "2026-11-21T14:03:18Z",
        "2026-11-21T14:06:16Z",
    ]
    .iter()
    .map(|s| s.parse().unwrap())
    .collect();
    assert_eq!(got, want);
}
