//! One-off items on air. Each airs at its time, the log has it, and then it goes.

use crate::fixtures::*;
use jiff::civil::{DateTime, date};
use radio_engine::{Command, Engine};
use radio_model::{FixMode, Item, Repeat};
use radio_playout::{Alarm, Playout};
use std::time::Duration;

fn songs(tag: &str) -> Engine {
    let main: Vec<_> = (1..=6)
        .map(|i| {
            let path = wav(&format!("{tag}-song{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    engine_with(main)
}

fn promo(tag: &str, title: &str) -> Item {
    file_item(0, title, &wav(tag, 1.0, Side::Right), 1.0, 1.0, 0.0)
}

fn at(h: i8, m: i8, s: i8) -> DateTime {
    date(2026, 11, 21).at(h, m, s, 0)
}

fn schedule(engine: &mut Engine, item: Item, start: DateTime, repeat: Repeat) -> u64 {
    engine
        .dispatch(Command::ScheduleAdd {
            one_off: Box::new(item),
            start,
            fix: FixMode::Hard,
            tolerance: Duration::from_secs(1),
            repeat,
        })
        .unwrap()
        .data
        .and_then(|d| d.as_u64())
        .unwrap()
}

fn one_offs(p: &Playout) -> usize {
    p.snapshot().state.one_offs().len()
}

#[test]
fn a_one_off_file_airs_at_its_time_is_logged_and_then_leaves_the_schedule() {
    let mut engine = songs("oo1");
    let id = schedule(
        &mut engine,
        promo("oo1-promo", "Promo"),
        at(13, 0, 5),
        Repeat::Once,
    );
    let (p, clock) = offline(engine, |_| {});
    let program = run(&p, &clock, 7.0);

    let start = first_audible(&program, 1, 0).expect("the one-off sounds");
    assert!(
        start.abs_diff(5 * SR) <= SR / 200,
        "one-off at {start}, target {}",
        5 * SR
    );
    let rows = p.history(t0(), 20).unwrap();
    assert!(
        rows.iter().any(|r| r.entry.title == "Promo"),
        "the as-played log has the one-off"
    );
    assert_eq!(
        one_offs(&p),
        0,
        "a one-off with no repeat leaves after it aired"
    );
    let planned = p.snapshot().timeline.entries;
    assert!(planned.iter().all(|e| e.item.0 != id));
}

#[test]
fn two_close_hard_one_offs_both_air_on_time() {
    let mut engine = songs("oo2");
    schedule(
        &mut engine,
        promo("oo2-a", "First"),
        at(13, 0, 5),
        Repeat::Once,
    );
    schedule(
        &mut engine,
        promo("oo2-b", "Second"),
        at(13, 0, 8),
        Repeat::Once,
    );
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let program = run(&p, &clock, 10.0);

    let first = first_audible(&program, 1, 0).expect("the first one-off sounds");
    assert!(first.abs_diff(5 * SR) <= SR / 200, "first at {first}");
    let second = first_audible(&program, 1, first + SR + SR / 10).expect("the second sounds");
    assert!(second.abs_diff(8 * SR) <= SR / 200, "second at {second}");
    let titles: Vec<_> = p
        .history(t0(), 40)
        .unwrap()
        .into_iter()
        .map(|r| r.entry.title)
        .collect();
    assert!(titles.contains(&"First".to_owned()) && titles.contains(&"Second".to_owned()));
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert!(
        !raised.iter().any(|a| matches!(
            a,
            Alarm::HardEventLocked { .. }
                | Alarm::HardEventCancelled { .. }
                | Alarm::HardEventMissed { .. }
                | Alarm::OneOffMissed { .. }
        )),
        "{raised:?}"
    );
    assert_eq!(one_offs(&p), 0);
}

#[test]
fn a_one_off_whose_time_passed_unplayed_raises_an_alarm_and_leaves() {
    let mut engine = songs("oo3");
    let id = schedule(
        &mut engine,
        promo("oo3-promo", "Missed promo"),
        at(12, 58, 0),
        Repeat::Once,
    );
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    run(&p, &clock, 1.0);
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert!(
        raised
            .iter()
            .any(|a| matches!(a, Alarm::OneOffMissed { item, .. } if item.0 == id)),
        "{raised:?}"
    );
    assert_eq!(one_offs(&p), 0);
}

#[test]
fn a_daily_one_off_stays_after_it_aired() {
    let mut engine = songs("oo4");
    schedule(
        &mut engine,
        promo("oo4-promo", "Daily"),
        at(13, 0, 3),
        Repeat::Daily,
    );
    let (p, clock) = offline(engine, |_| {});
    run(&p, &clock, 6.0);
    assert!(
        p.history(t0(), 20)
            .unwrap()
            .iter()
            .any(|r| r.entry.title == "Daily")
    );
    assert_eq!(one_offs(&p), 1, "a repeating one-off airs again tomorrow");
}
