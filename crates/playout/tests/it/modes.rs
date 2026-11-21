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
    let _ = p
        .send(Command::DeckPause {
            deck: 0,
            expect_item: Some(radio_model::ItemId(1)),
        })
        .unwrap();
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

fn news_at(time: &str) -> String {
    format!(
        r#"rule({{at: {{date: "2026-11-21", time: "{time}", repeat: "once"}},
                place: {{fixed: "hard", tolerance: 1}}, take: 1, constraints: []}});"#
    )
}

/// A hard event moves while it is on a deck, more than 10 s before its target. It airs
/// once at the new time, and it raises no missed-event alarm.
#[test]
fn moved_hard_event_airs_once_at_the_new_time() {
    let main: Vec<_> = (1..=3)
        .map(|i| {
            let path = wav(&format!("mv{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(10, "News", &wav("mv-news", 1.0, Side::Right), 1.0, 1.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    let playlist = add_playlist(&mut engine, "News", vec![news], &news_at("13:00:12"));
    let (p, clock) = offline(engine, |_| {});
    let before = run(&p, &clock, 1.0);
    let _ = p
        .send(Command::PlaylistSetRulesScript {
            playlist,
            script: news_at("13:00:14"),
        })
        .unwrap();
    let after = run(&p, &clock, 14.5);
    let program = [before, after].concat();

    let target = 14 * SR;
    let start = first_audible(&program, 1, 0).expect("the event sounds");
    assert!(
        start.abs_diff(target) <= SR / 200,
        "event at {start}, target {target}"
    );
    assert!(
        first_audible(&program, 1, target + SR + SR / 10).is_none(),
        "it aired twice"
    );
    let view = p.snapshot();
    assert!(
        view.alarms
            .iter()
            .all(|a| !matches!(a.alarm, radio_playout::Alarm::HardEventMissed { .. }))
    );
}

/// The same event moves 4 s before its target, inside the last 10 s. It still airs on
/// its target, and an alarm says that the change applies after it. It airs only once:
/// the occurrence at the new time does not air, and does not cut the music either.
#[test]
fn last_second_move_does_not_cancel_hard_event() {
    let main: Vec<_> = (1..=3)
        .map(|i| {
            let path = wav(&format!("ls{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(10, "News", &wav("ls-news", 1.0, Side::Right), 1.0, 1.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    let playlist = add_playlist(&mut engine, "News", vec![news], &news_at("13:00:05"));
    let (p, clock) = offline(engine, |_| {});
    let before = run(&p, &clock, 1.0);
    let _ = p
        .send(Command::PlaylistSetRulesScript {
            playlist,
            script: news_at("13:00:07"),
        })
        .unwrap();
    // Past the new time (:07) and the length of the event.
    let after = run(&p, &clock, 9.0);
    let program = [before, after].concat();

    let target = 5 * SR;
    let start = first_audible(&program, 1, 0).expect("the event sounds");
    assert!(
        start.abs_diff(target) <= SR / 200,
        "event at {start}, target {target}"
    );
    assert!(
        first_audible(&program, 1, start + SR + SR / 10).is_none(),
        "it aired twice"
    );
    let moved_to = 7 * SR;
    assert!(
        peak(&program, 0, moved_to - SR / 10, moved_to + SR / 2) > 0.01,
        "the music was cut at the new time"
    );
    let rows = p.history(t0(), 20).unwrap();
    let news = rows.iter().filter(|r| r.entry.title == "News").count();
    assert_eq!(news, 1, "the event aired more than once");
    let view = p.snapshot();
    assert!(
        view.alarms
            .iter()
            .any(|a| matches!(a.alarm, radio_playout::Alarm::HardEventLocked { .. }))
    );
    assert!(
        view.alarms
            .iter()
            .all(|a| !matches!(a.alarm, radio_playout::Alarm::HardEventMissed { .. }))
    );
}

/// Every deck is on air when a hard event is due, so it loads after its target. Within
/// its tolerance it still airs, once and without a break.
#[test]
fn hard_event_loaded_late_still_airs() {
    // No planned music: the operator fills all four decks.
    let mut engine = engine_with(vec![]);
    let carts: Vec<_> = (20..24)
        .map(|i| {
            file_item(
                i,
                &format!("Cart {i}"),
                &wav(&format!("late{i}"), 6.0, Side::Left),
                6.0,
                6.0,
                0.0,
            )
        })
        .collect();
    add_playlist(&mut engine, "Carts", carts, "");
    let mut news = file_item(
        10,
        "News",
        &wav("late-news", 1.0, Side::Right),
        1.0,
        1.0,
        0.0,
    );
    news.movable = false;
    news.trimmable = false;
    add_playlist(&mut engine, "News", vec![news], &news_at("13:00:03"));
    // A short preload: the event comes into view only when every deck is on air.
    let (p, clock) = offline(engine, |cfg| cfg.preload = Duration::from_secs(2));
    let first = Vec::new();
    for (deck, item) in [(0u8, 20u64), (1, 21), (2, 22), (3, 23)] {
        let item = radio_model::ItemId(item);
        let _ = p.send(Command::DeckLoad { deck, item }).unwrap();
        let _ = p
            .send(Command::DeckPlay {
                deck,
                expect_item: Some(item),
            })
            .unwrap();
    }
    let program = [first, run(&p, &clock, 5.0)].concat();

    let target = 3 * SR;
    let start = first_audible(&program, 1, 0).expect("the late event sounds");
    assert!(
        start >= target && start < target + SR,
        "event at {start}, target {target}"
    );
    for w in 0..16 {
        let from = start + w * SR / 20;
        assert!(
            peak(&program, 1, from, from + SR / 20) > 0.01,
            "a break at {from}"
        );
    }
    let rows = p.history(t0(), 20).unwrap();
    let news = rows.iter().filter(|r| r.entry.title == "News").count();
    assert_eq!(news, 1, "the event loaded more than once");
    let view = p.snapshot();
    assert!(
        view.alarms
            .iter()
            .all(|a| !matches!(a.alarm, radio_playout::Alarm::HardEventMissed { .. }))
    );
}

/// Every deck holds a paused operator item when a hard event is due, so it loads 3 s
/// after its target, within its tolerance. It still plays its full length, and the next
/// item starts after it, not at the target plus the length.
#[test]
fn late_hard_event_plays_in_full_and_the_next_item_follows_it() {
    let mut engine = engine_with(vec![]);
    let carts: Vec<_> = (20..24)
        .map(|i| {
            let path = wav(&format!("full{i}"), 9.0, Side::Silent);
            file_item(i, &format!("Cart {i}"), &path, 9.0, 9.0, 0.0)
        })
        .collect();
    add_playlist(&mut engine, "Carts", carts, "");
    let path = wav("full-news", 4.0, Side::Right);
    let mut news = file_item(10, "News", &path, 4.0, 4.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    let script = r#"rule({at: {date: "2026-11-21", time: "13:00:03", repeat: "once"},
                place: {fixed: "hard", tolerance: 5}, take: 1, constraints: []});"#;
    add_playlist(&mut engine, "News", vec![news], script);
    let (p, clock) = offline(engine, |cfg| cfg.preload = Duration::from_secs(2));
    let decks = [(0u8, 20u64), (1, 21), (2, 22), (3, 23)];
    for (deck, item) in decks {
        let item = radio_model::ItemId(item);
        let _ = p.send(Command::DeckLoad { deck, item }).unwrap();
        let _ = p
            .send(Command::DeckPlay {
                deck,
                expect_item: Some(item),
            })
            .unwrap();
    }
    let mut program = run(&p, &clock, 0.2);
    for (deck, item) in decks {
        let expect_item = Some(radio_model::ItemId(item));
        let _ = p.send(Command::DeckPause { deck, expect_item }).unwrap();
    }
    // Paused decks are neither cut nor taken off for the event: it waits for a deck.
    program.extend(run(&p, &clock, 5.8));
    for (deck, item) in decks {
        let expect_item = Some(radio_model::ItemId(item));
        let _ = p.send(Command::DeckSkip { deck, expect_item }).unwrap();
    }
    program.extend(run(&p, &clock, 0.5));
    // Music to follow the event, added while it plays.
    let song = file_item(
        30,
        "Song",
        &wav("full-song", 2.0, Side::Left),
        2.0,
        2.0,
        0.0,
    );
    let _ = p
        .send(Command::PlaylistAddItems {
            playlist: radio_model::PlaylistId(1),
            items: vec![song],
            at: None,
        })
        .unwrap();
    program.extend(run(&p, &clock, 5.5));

    let target = 3 * SR;
    let start = first_audible(&program, 1, 0).expect("the late event sounds");
    assert!(
        start >= target + 3 * SR && start < target + 4 * SR,
        "event at {start}, target {target}"
    );
    for w in 0..78 {
        let from = start + w * SR / 20;
        assert!(
            peak(&program, 1, from, from + SR / 20) > 0.01,
            "the event was cut at {from}"
        );
    }
    let next = first_audible(&program, 0, 0).expect("the next item sounds");
    assert!(
        next + SR / 100 >= start + 4 * SR,
        "the next item at {next} starts inside the event ({start} plus 4 s)"
    );
    let view = p.snapshot();
    assert!(
        view.alarms
            .iter()
            .all(|a| !matches!(a.alarm, radio_playout::Alarm::HardEventMissed { .. }))
    );
}

/// The operator removes the item of a hard event 4 s before its target. That is a cancel:
/// the event does not air, and the alarm says it was cancelled, not that it still airs.
#[test]
fn removing_a_locked_hard_event_cancels_it() {
    let main: Vec<_> = (1..=3)
        .map(|i| {
            let path = wav(&format!("rm{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(10, "News", &wav("rm-news", 1.0, Side::Right), 1.0, 1.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    let playlist = add_playlist(&mut engine, "News", vec![news], &news_at("13:00:05"));
    let (p, clock) = offline(engine, |_| {});
    let before = run(&p, &clock, 1.0);
    let _ = p
        .send(Command::PlaylistRemoveItems {
            playlist,
            items: vec![radio_model::ItemId(10)],
        })
        .unwrap();
    let after = run(&p, &clock, 5.5);
    let program = [before, after].concat();

    assert!(first_audible(&program, 1, 0).is_none(), "the event aired");
    let view = p.snapshot();
    let cancelled = view
        .alarms
        .iter()
        .any(|a| matches!(a.alarm, radio_playout::Alarm::HardEventCancelled { .. }));
    let locked = view
        .alarms
        .iter()
        .any(|a| matches!(a.alarm, radio_playout::Alarm::HardEventLocked { .. }));
    assert!(cancelled && !locked);
}

/// One playlist has two hard events, at :05 and :12. Inside the last 10 s before :05,
/// an operator deletes the :05 rule, so the :12 rule takes its place in the rule list.
/// The :05 event still airs (it is locked), and the :12 event airs too: the renumbering
/// never passes for a move of the :05 event.
#[test]
fn deleting_a_hard_rule_near_its_target_keeps_the_other_hard_event() {
    let main: Vec<_> = (1..=6)
        .map(|i| {
            let path = wav(&format!("dr{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(10, "News", &wav("dr-news", 1.0, Side::Right), 1.0, 1.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    let both = format!("{}\n{}", news_at("13:00:05"), news_at("13:00:12"));
    let playlist = add_playlist(&mut engine, "News", vec![news], &both);
    let (p, clock) = offline(engine, |_| {});
    let before = run(&p, &clock, 1.0);
    let _ = p
        .send(Command::PlaylistSetRulesScript {
            playlist,
            script: news_at("13:00:12"),
        })
        .unwrap();
    let after = run(&p, &clock, 13.5);
    let program = [before, after].concat();

    let first = first_audible(&program, 1, 0).expect("the :05 event sounds");
    assert!(
        first.abs_diff(5 * SR) <= SR / 200,
        "the locked event at {first}, target {}",
        5 * SR
    );
    let second = first_audible(&program, 1, first + SR + SR / 10).expect("the :12 event airs");
    assert!(
        second.abs_diff(12 * SR) <= SR / 200,
        "the other event at {second}, target {}",
        12 * SR
    );
    let rows = p.history(t0(), 40).unwrap();
    let news = rows.iter().filter(|r| r.entry.title == "News").count();
    assert_eq!(news, 2, "both hard events air");
}
