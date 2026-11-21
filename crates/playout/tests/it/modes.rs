use crate::fixtures::*;
use radio_engine::{Command, Mode};
use radio_playout::DeckPhase;
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
                fade_in: Duration::ZERO,
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
                fade_in: Duration::ZERO,
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
            lock: true,
        })
        .unwrap();
    program.extend(run(&p, &clock, 0.2));
    let view = p.snapshot();
    let late = view
        .timeline
        .entries
        .iter()
        .find(|e| e.item == radio_model::ItemId(10))
        .expect("late event row");
    let next = view
        .timeline
        .entries
        .iter()
        .find(|e| e.item == radio_model::ItemId(30))
        .expect("following row");
    assert!(late.start >= t0() + Duration::from_secs(6));
    assert!(
        next.start >= late.start + late.len.saturating_sub(late.trim),
        "timeline overlaps late event"
    );
    program.extend(run(&p, &clock, 5.3));

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

/// An item that an operator sends to play next airs right after the item on air, not a round
/// of the playlist later.
#[test]
fn a_play_next_item_airs_right_after_the_item_on_air() {
    let main: Vec<_> = (1..=3)
        .map(|i| {
            let path = wav(&format!("pn{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 4.0, 0.0)
        })
        .collect();
    let engine = engine_with(main);
    let (p, clock) = offline(engine, |_| {});
    let before = run(&p, &clock, 1.0);
    let next = file_item(20, "Next", &wav("pn-next", 1.0, Side::Right), 1.0, 1.0, 0.0);
    p.dispatch(Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: vec![next],
        at: None,
        lock: true,
    })
    .unwrap();
    p.dispatch(Command::PlayoutPlayNext {
        item: radio_model::ItemId(20),
    })
    .unwrap();
    let after = run(&p, &clock, 5.5);
    let program = [before, after].concat();
    let start = first_audible(&program, 1, 0).expect("the play-next item airs");
    assert!(
        start.abs_diff(4 * SR) <= SR / 10,
        "it starts when song 1 ends: at {start}, song 1 ends at {}",
        4 * SR
    );
    assert!(
        peak(&program, 0, 4 * SR + SR / 10, 5 * SR - SR / 10) < 0.01,
        "song 2 waits until the play-next item ends"
    );
    let view = p.snapshot();
    assert!(
        view.state.queue.is_empty(),
        "the item left the queue when it started"
    );
}

/// The operator deletes the first of two hard rules inside the 10 s window, then moves the
/// rule that is left from :20 to :22. The second edit moves the other event, not the locked
/// one: both events air, at :05 and at :22.
#[test]
fn deleting_then_editing_a_hard_rule_keeps_the_other_hard_event() {
    let main: Vec<_> = (1..=6)
        .map(|i| {
            let path = wav(&format!("de{i}"), 4.0, Side::Left);
            file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(10, "News", &wav("de-news", 1.0, Side::Right), 1.0, 1.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    let both = format!("{}\n{}", news_at("13:00:05"), news_at("13:00:20"));
    let playlist = add_playlist(&mut engine, "News", vec![news], &both);
    let (p, clock) = offline(engine, |_| {});
    let first = run(&p, &clock, 1.0);
    let _ = p
        .send(Command::PlaylistSetRulesScript {
            playlist,
            script: news_at("13:00:20"),
        })
        .unwrap();
    let second = run(&p, &clock, 1.0);
    let edited = news_at("13:00:22");
    let _ = p
        .send(Command::PlaylistSetRulesScript {
            playlist,
            script: edited,
        })
        .unwrap();
    let third = run(&p, &clock, 22.5);
    let program = [first, second, third].concat();

    let at5 = first_audible(&program, 1, 0).expect("the :05 event airs");
    assert!(
        at5.abs_diff(5 * SR) <= SR / 200,
        "the locked event at {at5}"
    );
    let at14 = first_audible(&program, 1, at5 + SR + SR / 10).expect("the moved event airs");
    assert!(
        at14.abs_diff(22 * SR) <= SR / 200,
        "the other event at {at14}"
    );
}

/// Play on the NEXT 2 deck: it fades in, and the item on air fades out then. The NEXT 1
/// item never airs here, and no deck holds an item twice.
#[test]
fn playing_next_2_crossfades_and_skips_next_1() {
    let sides = [Side::Left, Side::Both, Side::Right, Side::Silent];
    let main: Vec<_> = sides
        .iter()
        .enumerate()
        .map(|(k, side)| {
            let i = k as u64 + 1;
            let path = wav(&format!("x{i}"), 10.0, *side);
            file_item(i, &format!("Song {i}"), &path, 10.0, 10.0, 0.0)
        })
        .collect();
    let (p, clock) = offline(engine_with(main), |_| {});
    let before = run(&p, &clock, 1.0);
    let view = p.snapshot();
    let deck_of = |id: u64| {
        view.decks
            .iter()
            .position(|d| d.item == Some(radio_model::ItemId(id)))
            .expect("the song waits on a deck")
    };
    let third = deck_of(3);
    let _ = deck_of(2);
    p.dispatch(Command::DeckPlay {
        deck: third as u8,
        expect_item: Some(radio_model::ItemId(3)),
        fade_in: Duration::from_secs(1),
    })
    .unwrap();
    let after = run(&p, &clock, 3.0);
    let at = |s: f32| (s * SR as f32) as usize;
    // Song 3 sounds on the right only: its level grows over the fade-in.
    let early = peak(&after, 1, 0, at(0.1));
    let full = peak(&after, 1, at(1.2), at(1.5));
    assert!(full > 0.2, "song 3 plays: {full}");
    assert!(early < full * 0.3, "song 3 fades in: {early} then {full}");
    // Song 1 sounds on the left only: it fades out over the same second.
    let on_air = peak(&before, 0, at(0.5), at(0.9));
    let mid = peak(&after, 0, at(0.45), at(0.55));
    assert!(
        mid < on_air * 0.8 && mid > 0.0,
        "song 1 fades: {on_air} then {mid}"
    );
    assert!(
        peak(&after, 0, at(1.2), at(3.0)) < 0.001,
        "song 1 has ended"
    );
    let rows = p.history(t0(), 10).unwrap();
    let titles: Vec<&str> = rows.iter().map(|r| r.entry.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Song 1", "Song 3"],
        "song 2 waits for a later turn"
    );
    let held: Vec<_> = p.snapshot().decks.iter().filter_map(|d| d.item).collect();
    for (k, a) in held.iter().enumerate() {
        assert!(
            !held[k + 1..].contains(a),
            "{a:?} is on two decks: {held:?}"
        );
    }
}

/// Play on a waiting deck while the operator holds the item on air. The held item leaves
/// the air. The station plays on after the new item by itself.
#[test]
fn playing_a_deck_ends_a_paused_item_on_air() {
    let main: Vec<_> = (1..=4)
        .map(|i| {
            let side = if i == 1 { Side::Left } else { Side::Right };
            let path = wav(&format!("pz{i}"), 2.0, side);
            file_item(i, &format!("Song {i}"), &path, 2.0, 2.0, 0.0)
        })
        .collect();
    let (p, clock) = offline(engine_with(main), |_| {});
    run(&p, &clock, 0.5);
    let one = Some(radio_model::ItemId(1));
    let air = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.item == one)
        .unwrap();
    p.dispatch(Command::DeckPause {
        deck: air as u8,
        expect_item: one,
    })
    .unwrap();
    run(&p, &clock, 0.3);
    let two = Some(radio_model::ItemId(2));
    let next = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.item == two)
        .unwrap();
    p.dispatch(Command::DeckPlay {
        deck: next as u8,
        expect_item: two,
        fade_in: Duration::ZERO,
    })
    .unwrap();
    let after = run(&p, &clock, 0.5);
    // Song 1 may wait on a deck again for its next turn, but it is not paused on air.
    let paused = p
        .snapshot()
        .decks
        .iter()
        .any(|d| d.phase == DeckPhase::Paused);
    assert!(!paused, "no item stays paused on air");
    assert!(
        peak(&after, 0, SR / 10, after.len() / 2) < 0.001,
        "song 1 is off air"
    );
    run(&p, &clock, 4.0);
    let rows = p.history(t0(), 10).unwrap();
    let titles: Vec<&str> = rows.iter().map(|r| r.entry.title.as_str()).collect();
    assert!(
        titles.starts_with(&["Song 1", "Song 2", "Song 3"]),
        "the station plays on after song 2: {titles:?}"
    );
}

#[test]
fn transport_waits_for_hard_target_and_manual_play_covers_it() {
    for manual in [false, true] {
        let mut engine = engine_with(vec![]);
        engine
            .dispatch(Command::ModeSet { mode: Mode::Assist })
            .unwrap();
        let item = file_item(
            90,
            "Signal",
            &wav("fixed-transport", 0.5, Side::Right),
            0.5,
            0.5,
            0.0,
        );
        add_playlist(&mut engine, "Signals", vec![item], &news_at("13:00:05"));
        let (p, clock) = offline(engine, |_| {});
        run(&p, &clock, 1.0);
        let deck = p
            .snapshot()
            .decks
            .iter()
            .position(|d| d.item == Some(radio_model::ItemId(90)))
            .unwrap() as u8;
        if manual {
            let _ = p
                .send(Command::DeckLoad {
                    deck,
                    item: radio_model::ItemId(90),
                })
                .unwrap();
        }
        let _ = p
            .send(Command::DeckPlay {
                deck,
                expect_item: Some(radio_model::ItemId(90)),
                fade_in: Duration::ZERO,
            })
            .unwrap();
        let _ = p.send(Command::PlayoutNext).unwrap();
        let early = run(&p, &clock, 2.0);
        assert_eq!(first_audible(&early, 1, 0).is_some(), manual);
        let target = run(&p, &clock, 4.0);
        assert_eq!(first_audible(&target, 1, 0).is_some(), !manual);
        let rows = p.history(t0(), 20).unwrap();
        assert_eq!(rows.iter().filter(|r| r.entry.title == "Signal").count(), 1);
    }
}

#[test]
fn cue_only_removes_strip_from_program_and_gain_edits_keep_it_off_air() {
    let song = file_item(
        91,
        "Routing test",
        &wav("cue-route", 10.0, Side::Left),
        10.0,
        10.0,
        0.0,
    );
    let (p, clock) = offline(engine_with(vec![song]), |_| {});
    assert!(first_audible(&run(&p, &clock, 0.5), 0, 0).is_some());
    let deck = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.item == Some(radio_model::ItemId(91)))
        .unwrap();
    let strip = ["A", "B", "C", "D"][deck].to_owned();
    for (route, gain_db) in [
        (Some(radio_engine::Route::CueOnly), None),
        (None, Some(3.0)),
    ] {
        let _ = p
            .send(Command::MixerSetStrip {
                strip: strip.clone(),
                gain_db,
                mute: None,
                pfl: None,
                route,
            })
            .unwrap();
        let audio = run(&p, &clock, 0.2);
        assert!(peak(&audio, 0, SR / 20, audio.len() / 2) < 0.01);
    }
    let _ = p
        .send(Command::MixerSetStrip {
            strip,
            gain_db: None,
            mute: None,
            pfl: None,
            route: Some(radio_engine::Route::Out),
        })
        .unwrap();
    assert!(first_audible(&run(&p, &clock, 0.2), 0, 0).is_some());
}

#[test]
fn slow_rules_script_does_not_block_scheduled_play() {
    let mut engine = engine_with(vec![]);
    let signal = file_item(
        92,
        "Worker signal",
        &wav("worker-signal", 1.0, Side::Right),
        1.0,
        1.0,
        0.0,
    );
    add_playlist(&mut engine, "Signals", vec![signal], &news_at("13:00:05"));
    let (p, clock) = offline(engine, |_| {});
    run(&p, &clock, 4.7);
    // The script consumes its 200 ms execution budget. Play must still be sent at
    // the 300 ms lead, on a control tick while this worker is busy.
    let reply = p
        .send(Command::PlaylistSetRulesScript {
            playlist: radio_model::PlaylistId(1),
            script: "while (true) {}".into(),
        })
        .unwrap();
    let before = std::time::Instant::now();
    let mut program = p.render(STEP).unwrap();
    assert!(
        before.elapsed() < Duration::from_millis(150),
        "script stalled the control thread at the command lead"
    );
    clock.advance(Duration::from_millis(10));
    program.extend(run(&p, &clock, 0.69));
    let start = first_audible(&program, 1, 0).unwrap();
    assert!(start.abs_diff(SR * 30 / 100) < SR / 100);
    std::thread::sleep(Duration::from_millis(220));
    run(&p, &clock, 0.01);
    assert!(reply.recv_timeout(Duration::from_secs(1)).unwrap().is_err());
}

#[test]
fn operator_fade_uses_requested_length() {
    let item = file_item(
        1,
        "Long tone",
        &wav("fade-length", 8.0, Side::Left),
        8.0,
        8.0,
        0.1,
    );
    let (p, clock) = offline(engine_with(vec![item]), |_| {});
    run(&p, &clock, 1.0);
    let deck = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.phase == DeckPhase::Playing)
        .unwrap();
    let _answer = p
        .send(Command::DeckFade {
            deck: deck as u8,
            expect_item: Some(radio_model::ItemId(1)),
            over: Duration::from_secs(2),
        })
        .unwrap();
    let sound = run(&p, &clock, 2.5);
    assert!(
        peak(&sound, 0, SR / 2, SR) > 0.02,
        "fade ended before its requested length"
    );
    assert!(
        peak(&sound, 0, SR * 22 / 10, SR * 24 / 10) < 0.01,
        "fade did not end"
    );
}

#[test]
fn pick_only_rule_edit_does_not_raise_hard_event_locked() {
    let main = file_item(
        1,
        "Main tone",
        &wav("pick-main", 10.0, Side::Left),
        10.0,
        10.0,
        0.0,
    );
    let mut engine = engine_with(vec![main]);
    let event = file_item(
        10,
        "Fixed tone",
        &wav("pick-event", 1.0, Side::Right),
        1.0,
        1.0,
        0.0,
    );
    let playlist = add_playlist(&mut engine, "Fixed", vec![event], &news_at("13:00:05"));
    let (p, clock) = offline(engine, |_| {});
    run(&p, &clock, 1.0);
    let script = format!(
        "pick('shuffle', {{seed: 7}}); {}",
        news_at("13:00:05").replace("constraints: []", "constraints: [maxPerHour(20)]")
    );
    let reply = p
        .send(Command::PlaylistSetRulesScript { playlist, script })
        .unwrap();
    run(&p, &clock, 0.3);
    reply.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    assert!(
        !p.snapshot()
            .alarms
            .iter()
            .any(|a| matches!(a.alarm, radio_playout::Alarm::HardEventLocked { .. }))
    );
}
