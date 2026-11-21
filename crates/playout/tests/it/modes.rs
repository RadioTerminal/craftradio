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
