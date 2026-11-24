use crate::fixtures::*;
use radio_engine::Command;
use radio_model::{ItemId, Source};
use radio_playout::Capture;
use std::time::Duration;

struct Fake(rtrb::Producer<f32>);
impl Capture for Fake {
    fn error(&mut self) -> Option<String> {
        while self.0.slots() >= 2 {
            self.0.push(0.2).unwrap();
            self.0.push(0.0).unwrap();
        }
        None
    }
}
fn fake(
    name: &str,
    channels: [u16; 2],
    mut tx: rtrb::Producer<f32>,
) -> Result<Box<dyn Capture>, String> {
    if name != "Test input" {
        return Err("Input device missing".into());
    }
    assert_eq!(channels, [2, 3]);
    while tx.slots() >= 2 {
        tx.push(0.2).unwrap();
        tx.push(0.0).unwrap();
    }
    Ok(Box::new(Fake(tx)))
}
fn live(name: &str) -> radio_model::Item {
    let mut item = radio_testkit::track(1, "Studio interval", "Test", 1);
    item.source = Source::LiveInput {
        device: name.into(),
        channels: [2, 3],
    };
    item.timing = None;
    item.planned_len = Some(Duration::from_secs(1));
    item
}
#[test]
fn live_capture_ends_at_planned_end() {
    let (p, clock) = offline(engine_with(vec![live("Test input")]), |cfg| {
        cfg.open_input = fake
    });
    let out = run(&p, &clock, 0.7);
    assert!(peak(&out, 0, SR / 10, SR / 2) > 0.1);
    let _ = p.send(Command::StationStopAfterCurrent).unwrap();
    let tail = run(&p, &clock, 0.8);
    assert!(peak(&tail, 0, SR / 2, SR * 7 / 10) < 0.01);
}
#[test]
fn missing_live_device_raises_alarm_and_skips() {
    let (p, clock) = offline(engine_with(vec![live("Missing input")]), |cfg| {
        cfg.open_input = fake
    });
    run(&p, &clock, 0.1);
    assert!(p.snapshot().alarms.iter().any(|a| matches!(
        a.alarm,
        radio_playout::Alarm::ItemSkipped {
            item: ItemId(1),
            ..
        }
    )));
}
#[test]
fn operator_can_end_live_capture() {
    let (p, clock) = offline(engine_with(vec![live("Test input")]), |cfg| {
        cfg.open_input = fake
    });
    run(&p, &clock, 0.2);
    let deck = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.phase == radio_playout::DeckPhase::Playing)
        .unwrap();
    let _ = p.send(Command::StationStopAfterCurrent).unwrap();
    let _ = p
        .send(Command::DeckSkip {
            deck: deck as u8,
            expect_item: Some(ItemId(1)),
        })
        .unwrap();
    let out = run(&p, &clock, 0.3);
    assert!(peak(&out, 0, SR / 10, SR / 4) < 0.01);
}

#[test]
fn following_fixed_event_ends_live_input() {
    let mut input = live("Test input");
    input.planned_len = Some(Duration::from_secs(10));
    input.trimmable = false;
    let mut engine = engine_with(vec![input]);
    let event = file_item(
        10,
        "Fixed interval",
        &wav("live-fixed", 1.0, Side::Right),
        1.0,
        1.0,
        0.0,
    );
    add_playlist(
        &mut engine,
        "Fixed",
        vec![event],
        r#"rule({at: {date: "2026-11-21", time: "13:00:02", repeat: "once"}, place: {fixed: "hard", tolerance: 1}, take: 1, constraints: []});"#,
    );
    let (p, clock) = offline(engine, |cfg| cfg.open_input = fake);
    let _ = p.send(Command::StationStopAfterCurrent).unwrap();
    // Start the live deck explicitly: Stop after current must still allow the fixed event.
    run(&p, &clock, 0.1);
    let deck = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.item == Some(ItemId(1)))
        .unwrap();
    let _ = p
        .send(Command::DeckPlay {
            deck: deck as u8,
            expect_item: Some(ItemId(1)),
            fade_in: Duration::ZERO,
        })
        .unwrap();
    let out = run(&p, &clock, 2.5);
    assert!(peak(&out, 0, SR / 5, SR) > 0.1);
    assert!(peak(&out, 1, 2 * SR, SR * 24 / 10) > 0.01);
    assert!(peak(&out, 0, 2 * SR, SR * 24 / 10) < 0.01);
}

#[test]
fn late_operator_start_does_not_extend_the_live_interval() {
    let mut engine = engine_with(vec![live("Test input")]);
    engine
        .dispatch(Command::ModeSet {
            mode: radio_engine::Mode::Assist,
        })
        .unwrap();
    let (p, clock) = offline(engine, |cfg| cfg.open_input = fake);
    run(&p, &clock, 0.4);
    let deck = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.item == Some(ItemId(1)))
        .unwrap();
    let _ = p
        .send(Command::DeckPlay {
            deck: deck as u8,
            expect_item: Some(ItemId(1)),
            fade_in: Duration::ZERO,
        })
        .unwrap();
    let out = run(&p, &clock, 0.9);
    assert!(peak(&out, 0, SR / 10, SR / 3) > 0.1);
    assert!(
        peak(&out, 0, SR * 7 / 10, SR * 8 / 10) < 0.01,
        "late start extended the live interval"
    );
}

static UNPLUGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct Failing(rtrb::Producer<f32>);
impl Capture for Failing {
    fn error(&mut self) -> Option<String> {
        while self.0.slots() >= 2 {
            self.0.push(0.2).unwrap();
            self.0.push(0.0).unwrap();
        }
        UNPLUGGED
            .load(std::sync::atomic::Ordering::Relaxed)
            .then(|| "Input device unplugged".to_owned())
    }
}
fn failing(_: &str, _: [u16; 2], tx: rtrb::Producer<f32>) -> Result<Box<dyn Capture>, String> {
    Ok(Box::new(Failing(tx)))
}

/// A live input that fails on air frees its deck, so the next item plays.
#[test]
fn a_live_input_that_fails_on_air_gives_way_to_the_next_item() {
    let mut input = live("Test input");
    input.planned_len = Some(Duration::from_secs(30));
    let next = file_item(
        2,
        "Next",
        &wav("live-next", 3.0, Side::Right),
        3.0,
        2.5,
        0.5,
    );
    let (p, clock) = offline(engine_with(vec![input, next]), |cfg| {
        cfg.open_input = failing
    });
    let before = run(&p, &clock, 0.5);
    assert!(
        peak(&before, 0, SR / 10, SR / 2) > 0.1,
        "the input is on air"
    );
    UNPLUGGED.store(true, std::sync::atomic::Ordering::Relaxed);
    let out = run(&p, &clock, 2.0);
    assert!(
        p.snapshot().decks.iter().all(|d| d.item != Some(ItemId(1))),
        "the failed live input still holds a deck"
    );
    assert!(
        first_audible(&out, 1, 0).is_some(),
        "the next item never started"
    );
}
