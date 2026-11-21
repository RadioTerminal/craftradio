//! One-off items: the system playlist "One-off" holds them. Each item has an At rule of its own.

use jiff::civil::date;
use jiff::{SignedDuration, Timestamp};
use radio_model::{
    Constraint, FixMode, ItemId, Placement, PlaylistRole, Repeat, Rule, Station, Trigger,
};
use radio_scheduler::{Entry, Timeline, Warning};
use radio_testkit::{jingle, playlist, station, track};
use std::time::Duration;

use super::plan::run_for;

fn at(minute: i8) -> Timestamp {
    date(2026, 11, 21)
        .at(13, minute, 0, 0)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .unwrap()
        .timestamp()
}

fn one_off_rule(item: u64, minute: i8, repeat: Repeat, mode: FixMode, tol: u64) -> Rule {
    Rule {
        trigger: Trigger::At {
            at: date(2026, 11, 21).at(13, minute, 0, 0),
            repeat,
        },
        placement: Placement::Fixed {
            mode,
            tolerance: Duration::from_secs(tol),
        },
        take: 1,
        constraints: vec![Constraint::OnlyItem(ItemId(item))],
    }
}

/// Main music, a fill pool and a One-off playlist. The rule order differs from the item
/// order, so a plain pick airs the wrong item.
fn one_off_station(rules: Vec<Rule>) -> Station {
    let mut m = playlist(
        1,
        "main",
        (1..=40)
            .map(|i| track(i, &format!("track-{i}"), &format!("artist-{i}"), 187))
            .collect(),
    );
    m.is_main = true;
    let mut one = playlist(5, "One-off", vec![jingle(501, 60), jingle(502, 45)]);
    one.role = PlaylistRole::OneOff;
    one.rules = rules;
    let mut pool = playlist(9, "fill", (0..12).map(|i| jingle(900 + i, 20)).collect());
    pool.is_fill_pool = true;
    station("UTC", vec![m, one, pool])
}

fn entry_of(tl: &Timeline, item: u64) -> Vec<&Entry> {
    tl.entries
        .iter()
        .filter(|e| e.item == ItemId(item))
        .collect()
}

fn dropped(tl: &Timeline, item: u64) -> bool {
    tl.warnings
        .iter()
        .any(|w| matches!(w, Warning::EntryDropped { item: i, .. } if *i == ItemId(item)))
}

#[test]
fn each_one_off_rule_airs_its_own_item_at_its_time() {
    let st = one_off_station(vec![
        one_off_rule(502, 20, Repeat::Daily, FixMode::Hard, 1),
        one_off_rule(501, 40, Repeat::Weekly, FixMode::Hard, 1),
    ]);
    let tl = run_for(&st, &[], None, Duration::from_secs(3600));
    let b = entry_of(&tl, 502);
    let a = entry_of(&tl, 501);
    assert_eq!(b.len(), 1, "{:?}", tl.entries);
    assert_eq!(a.len(), 1, "{:?}", tl.entries);
    assert_eq!(b[0].start, at(20), "the 13:20 rule airs its own item");
    assert_eq!(a[0].start, at(40), "the 13:40 rule airs its own item");
}

#[test]
fn a_hard_one_off_starts_exactly_at_its_time() {
    let st = one_off_station(vec![one_off_rule(501, 23, Repeat::Once, FixMode::Hard, 1)]);
    let tl = run_for(&st, &[], None, Duration::from_secs(3600));
    let e = entry_of(&tl, 501);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].start, at(23));
    assert!(!dropped(&tl, 501));
}

#[test]
fn a_soft_one_off_starts_within_its_tolerance_and_is_never_dropped() {
    let tol = 3;
    let st = one_off_station(vec![one_off_rule(
        501,
        31,
        Repeat::Once,
        FixMode::Soft,
        tol,
    )]);
    let tl = run_for(&st, &[], None, Duration::from_secs(3600));
    let e = entry_of(&tl, 501);
    assert_eq!(e.len(), 1, "the fitter keeps the one-off");
    let off = e[0].start.duration_since(at(31)).abs();
    assert!(
        off <= SignedDuration::from_secs(tol as i64),
        "starts {off} from its time"
    );
    assert!(!dropped(&tl, 501));
}

#[test]
fn a_one_off_with_no_repeat_plays_once() {
    let st = one_off_station(vec![one_off_rule(501, 10, Repeat::Once, FixMode::Hard, 1)]);
    let tl = run_for(&st, &[], None, Duration::from_secs(26 * 3600));
    assert_eq!(entry_of(&tl, 501).len(), 1);
}
