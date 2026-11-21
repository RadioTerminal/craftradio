use crate::fixtures::*;
use radio_engine::{Command, Mode};
use std::time::Duration;

/// A hard event starts within 5 ms of its target on the sample clock. The item on
/// air has faded out by then.
#[test]
fn hard_event_starts_on_target() {
    let main: Vec<_> = (1..=3)
        .map(|i| {
            let path = wav(&format!("song{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(10, "News", &wav("news", 1.0, Side::Right), 1.0, 1.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    add_playlist(
        &mut engine,
        "News",
        vec![news],
        r#"rule({at: {date: "2026-11-21", time: "13:00:05", repeat: "once"},
                place: {fixed: "hard", tolerance: 1}, take: 1, constraints: []});"#,
    );
    let (p, clock) = offline(engine, |_| {});
    let program = run(&p, &clock, 6.5);

    let target = 5 * SR;
    let start = first_audible(&program, 1, 0).expect("the event sounds");
    assert!(
        start.abs_diff(target) <= SR / 200,
        "event at {start}, target {target}"
    );
    assert!(
        peak(&program, 0, target, target + SR / 2) < 0.01,
        "song still on air"
    );
}

/// In `Assist` mode nothing starts until Next, and the next item waits for Next again.
#[test]
fn assist_mode_waits_for_next() {
    let one = file_item(1, "One", &wav("a-one", 2.0, Side::Left), 2.0, 1.5, 0.5);
    let two = file_item(2, "Two", &wav("a-two", 2.0, Side::Right), 2.0, 1.5, 0.5);
    let mut engine = engine_with(vec![one, two]);
    engine
        .dispatch(Command::ModeSet { mode: Mode::Assist })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});

    let idle = run(&p, &clock, 1.0);
    assert!(first_audible(&idle, 0, 0).is_none(), "started without Next");
    assert!(p.snapshot().waiting_for_next);

    let answer = p.send(Command::PlayoutNext).unwrap();
    let first = run(&p, &clock, 4.0);
    let outcome = answer
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert!(outcome.applied_to_audio, "the mixer did not confirm Next");
    assert!(
        first_audible(&first, 0, 0).is_some(),
        "Next did not start item one"
    );
    assert!(
        first_audible(&first, 1, 0).is_none(),
        "item two did not wait"
    );
    assert!(p.snapshot().waiting_for_next);

    let _ = p.send(Command::PlayoutNext).unwrap();
    let second = run(&p, &clock, 0.5);
    assert!(
        first_audible(&second, 1, 0).is_some(),
        "Next did not start item two"
    );
}

/// In `Assist` mode a silent Program is a wait on purpose. Without Next, nothing
/// starts in 20 s and the emergency folder stays off.
#[test]
fn assist_mode_does_not_advance_on_silence() {
    let one = file_item(1, "One", &wav("w-one", 2.0, Side::Left), 2.0, 1.5, 0.5);
    let two = file_item(2, "Two", &wav("w-two", 2.0, Side::Right), 2.0, 1.5, 0.5);
    let mut engine = engine_with(vec![one, two]);
    engine
        .dispatch(Command::ModeSet { mode: Mode::Assist })
        .unwrap();
    let backup = wav("w-backup", 2.0, Side::Both);
    let folder = std::path::Path::new(&backup)
        .parent()
        .unwrap()
        .to_path_buf();
    let (p, clock) = offline(engine, |cfg| {
        cfg.mixer.silence.after = Duration::from_millis(500);
        cfg.fallback_folder = Some(folder);
    });
    let program = run(&p, &clock, 20.0);

    assert!(first_audible(&program, 0, 0).is_none(), "something played");
    assert!(first_audible(&program, 1, 0).is_none(), "something played");
    let view = p.snapshot();
    assert!(view.waiting_for_next && !view.emergency);
    assert!(p.history(t0(), 10).unwrap().is_empty());
}

/// A hard event fires on target even when queued and operator items fill every deck.
#[test]
fn hard_event_is_never_skipped_when_decks_are_full() {
    let main: Vec<_> = (1..=3)
        .map(|i| {
            let path = wav(&format!("full{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(
        10,
        "News",
        &wav("full-news", 1.0, Side::Right),
        1.0,
        1.0,
        0.0,
    );
    news.movable = false;
    news.trimmable = false;
    add_playlist(
        &mut engine,
        "News",
        vec![news],
        r#"rule({at: {date: "2026-11-21", time: "13:00:05", repeat: "once"},
                place: {fixed: "hard", tolerance: 1}, take: 1, constraints: []});"#,
    );
    engine
        .dispatch(Command::ModeSet { mode: Mode::Assist })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    for (deck, item) in [(2, 1), (3, 2)] {
        let item = radio_model::ItemId(item);
        let _ = p.send(Command::DeckLoad { deck, item }).unwrap();
    }
    let program = run(&p, &clock, 6.0);

    let target = 5 * SR;
    let start = first_audible(&program, 1, 0).expect("the event sounds");
    assert!(
        start.abs_diff(target) <= SR / 200,
        "event at {start}, target {target}"
    );
}

/// An operator pause in `Auto` mode is a wait. Silence does not skip, the next item does
/// not start, and the emergency folder stays off.
#[test]
fn pause_in_auto_is_a_wait_not_dead_air() {
    let one = file_item(1, "One", &wav("p-one", 4.0, Side::Left), 4.0, 3.5, 0.5);
    let two = file_item(2, "Two", &wav("p-two", 2.0, Side::Right), 2.0, 1.5, 0.5);
    let backup = wav("p-backup", 2.0, Side::Both);
    let folder = std::path::Path::new(&backup)
        .parent()
        .unwrap()
        .to_path_buf();
    let (p, clock) = offline(engine_with(vec![one, two]), |cfg| {
        cfg.mixer.silence.after = Duration::from_millis(500);
        cfg.fallback_folder = Some(folder);
    });
    let before = run(&p, &clock, 1.0);
    assert!(
        first_audible(&before, 0, 0).is_some(),
        "item one did not start"
    );
    let _ = p.send(Command::DeckPause { deck: 0 }).unwrap();
    let after = run(&p, &clock, 3.0);

    assert!(
        peak(&after, 0, SR / 10, after.len() / 2) < 0.01,
        "the pause did not hold"
    );
    assert!(
        first_audible(&after, 1, 0).is_none(),
        "the next item started"
    );
    let view = p.snapshot();
    assert_eq!(view.decks[0].phase, radio_playout::DeckPhase::Paused);
    assert!(!view.emergency);
    assert!(
        view.alarms
            .iter()
            .all(|a| a.alarm != radio_playout::Alarm::DeadAir)
    );
}
