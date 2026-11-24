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

fn update(p: &Playout, id: u64, item: Item, start: DateTime, repeat: Repeat) {
    p.dispatch(Command::ScheduleUpdate {
        id: radio_model::ItemId(id),
        one_off: Box::new(item),
        start,
        fix: FixMode::Hard,
        tolerance: Duration::from_secs(1),
        repeat,
    })
    .unwrap();
}

#[test]
fn a_daily_one_off_that_aired_and_then_became_a_later_once_stays_scheduled() {
    let mut engine = songs("oo5");
    let id = schedule(
        &mut engine,
        promo("oo5-promo", "Daily"),
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
    update(
        &p,
        id,
        promo("oo5-later", "Later"),
        at(14, 0, 0),
        Repeat::Once,
    );
    run(&p, &clock, 2.0);
    let ones = p.snapshot().state.one_offs();
    assert_eq!(
        ones.len(),
        1,
        "the later one-off has not aired, so it stays"
    );
    assert_eq!(ones[0].item.title, "Later");
}

#[test]
fn an_update_just_before_air_keeps_both_hard_one_offs_on_time() {
    let mut engine = songs("oo6");
    let a = schedule(
        &mut engine,
        promo("oo6-a", "First"),
        at(13, 0, 5),
        Repeat::Once,
    );
    schedule(
        &mut engine,
        promo("oo6-b", "Second"),
        at(13, 0, 8),
        Repeat::Once,
    );
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let before = run(&p, &clock, 2.0);
    update(
        &p,
        a,
        promo("oo6-a2", "First, new cut"),
        at(13, 0, 5),
        Repeat::Once,
    );
    let after = run(&p, &clock, 8.0);
    let program = [before, after].concat();
    let first = first_audible(&program, 1, 0).expect("the first one-off sounds");
    assert!(first.abs_diff(5 * SR) <= SR / 200, "first at {first}");
    let second = first_audible(&program, 1, first + SR + SR / 10).expect("the second sounds");
    assert!(second.abs_diff(8 * SR) <= SR / 200, "second at {second}");
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert!(
        !raised.iter().any(|a| matches!(
            a,
            Alarm::HardEventCancelled { .. }
                | Alarm::HardEventMissed { .. }
                | Alarm::OneOffMissed { .. }
        )),
        "{raised:?}"
    );
}

fn soft(item: Item, start: DateTime) -> Command {
    Command::ScheduleAdd {
        one_off: Box::new(item),
        start,
        fix: FixMode::Soft,
        tolerance: Duration::from_secs(1),
        repeat: Repeat::Once,
    }
}

/// Songs that the fitter may neither move nor trim. A person placed the `locked` ones.
fn fixed_songs(tag: &str, locked: &[u64]) -> Engine {
    let main: Vec<_> = (1..=6)
        .map(|i| {
            let path = wav(&format!("{tag}-song{i}"), 4.0, Side::Left);
            let mut item = file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5);
            item.movable = false;
            item.trimmable = false;
            item
        })
        .collect();
    let mut engine = Engine::open_in_memory().unwrap();
    engine
        .dispatch(Command::PlaylistAddItems {
            playlist: radio_model::PlaylistId(1),
            items: main,
            at: None,
            lock: false,
        })
        .unwrap();
    lock_items(&mut engine, locked);
    engine
}

fn lock_items(engine: &mut Engine, ids: &[u64]) {
    for &id in ids {
        engine
            .dispatch(Command::ItemLock {
                item: radio_model::ItemId(id),
            })
            .unwrap();
    }
}

/// The songs `ids` aired one after the other, each at the segue of the one before it.
/// So no soft event moved a locked song.
fn assert_back_to_back(p: &Playout, ids: &[u64]) {
    let rows = p.history(t0(), 40).unwrap();
    let start = |id: u64| {
        let title = format!("Song {id}");
        rows.iter()
            .find(|r| r.entry.title == title)
            .unwrap()
            .entry
            .start
    };
    for w in ids.windows(2) {
        let gap = start(w[1]).duration_since(start(w[0])).as_secs_f64();
        assert!(
            (gap - 3.5).abs() < 0.05,
            "locked Song {} starts at the segue of Song {}: {gap} s",
            w[1],
            w[0]
        );
    }
}

/// When the song with `id` hands over, from its row in the history.
fn handover(p: &Playout, id: u64) -> usize {
    let title = format!("Song {id}");
    let rows = p.history(t0(), 40).unwrap();
    let row = rows.iter().find(|r| r.entry.title == title).unwrap();
    let secs = row.entry.start.duration_since(t0()).as_secs_f64() + 3.5;
    (secs * SR as f64) as usize
}

/// The soft one-off airs late, after the song in its way, with no song on top of it. The
/// alarm says it was late, and it leaves the schedule.
fn assert_aired_late(p: &Playout, program: &[f32], raised: &[Alarm], id: u64, target: usize) {
    let start = first_audible(program, 1, 0).expect("the soft one-off airs");
    assert!(
        start > target + SR,
        "it starts after its tolerance: {start}"
    );
    assert!(
        peak(program, 0, start + 6 * SR / 10, start + 95 * SR / 100) < 0.01,
        "no song plays on top of it"
    );
    assert!(
        raised
            .iter()
            .any(|a| matches!(a, Alarm::FixedLate { item, .. } if item.0 == id)),
        "a late soft event raises the alarm: {raised:?}"
    );
    assert!(
        !raised
            .iter()
            .any(|a| matches!(a, Alarm::OneOffMissed { .. })),
        "{raised:?}"
    );
    assert!(
        p.history(t0(), 40)
            .unwrap()
            .iter()
            .any(|r| r.entry.title == "Soft promo")
    );
    assert_eq!(one_offs(p), 0, "it aired, so it leaves");
}

fn added_id(p: &Playout, cmd: Command) -> u64 {
    p.dispatch(cmd)
        .unwrap()
        .data
        .and_then(|d| d.as_u64())
        .unwrap()
}

#[test]
fn a_soft_one_off_behind_an_untrimmable_song_airs_late_with_an_alarm() {
    // Song 2 starts at 3.5 s and hands over at 7 s, past 5 s + 1 s. Nothing may move or trim.
    let mut engine = fixed_songs("oo7", &[]);
    let cmd = soft(promo("oo7-promo", "Soft promo"), at(13, 0, 5));
    let id = engine
        .dispatch(cmd)
        .unwrap()
        .data
        .and_then(|d| d.as_u64())
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let program = run(&p, &clock, 14.0);
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert_aired_late(&p, &program, &raised, id, 5 * SR);
}

#[test]
fn a_soft_one_off_among_locked_songs_waits_for_the_last_locked_song_in_its_way() {
    // A person placed songs 1 to 3, so they are locked. The one-off at 5 s falls in song 2,
    // and song 3 follows it. Locked songs never move, so the one-off waits for song 3.
    let mut engine = fixed_songs("oo8", &[1, 2, 3]);
    let cmd = soft(promo("oo8-promo", "Soft promo"), at(13, 0, 5));
    let id = engine
        .dispatch(cmd)
        .unwrap()
        .data
        .and_then(|d| d.as_u64())
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let program = run(&p, &clock, 16.0);
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert_aired_late(&p, &program, &raised, id, 5 * SR);
    assert_back_to_back(&p, &[1, 2, 3]);
    let start = first_audible(&program, 1, 0).unwrap();
    assert!(
        start + SR / 100 >= handover(&p, 3),
        "it starts after the last locked song in its way: {start}"
    );
}

#[test]
fn a_soft_one_off_added_while_the_decks_are_full_still_airs() {
    // At 9 s the three next songs wait on decks. The one-off at 12 s goes before two of them.
    let engine = fixed_songs("oo9", &[]);
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let before = run(&p, &clock, 9.0);
    let id = added_id(&p, soft(promo("oo9-promo", "Soft promo"), at(13, 0, 12)));
    let after = run(&p, &clock, 12.0);
    let program = [before, after].concat();
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert_aired_late(&p, &program, &raised, id, 12 * SR);
}

#[test]
fn a_soft_one_off_in_dead_air_never_starts_before_its_tolerance() {
    let mut engine = Engine::open_in_memory().unwrap();
    engine
        .dispatch(soft(promo("oo10-promo", "Soft in silence"), at(13, 0, 30)))
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let program = run(&p, &clock, 33.0);

    let start = first_audible(&program, 1, 0).expect("the soft one-off airs");
    assert!(
        start + SR >= 30 * SR,
        "it starts no earlier than its time less its tolerance: {start}"
    );
    assert!(start <= 31 * SR, "and no later than its tolerance: {start}");
}

#[test]
fn music_added_while_a_soft_one_off_waits_on_a_deck_plays_before_it() {
    music_plays_before_a_waiting_soft_one_off(None);
}

/// With two decks the waiting one-off does not hold the deck that the next song needs.
#[test]
fn music_plays_before_a_waiting_soft_one_off_with_two_decks() {
    music_plays_before_a_waiting_soft_one_off(Some(2));
}

fn music_plays_before_a_waiting_soft_one_off(decks: Option<u64>) {
    // The one-off loads at once on a quiet station. Then a person adds songs and locks the
    // first three, so they stay before the one-off and keep their starts.
    let mut engine = Engine::open_in_memory().unwrap();
    if let Some(n) = decks {
        set_decks(&mut engine, n);
    }
    engine
        .dispatch(soft(promo("oo11-promo", "Soft promo"), at(13, 0, 20)))
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let mut program = run(&p, &clock, 2.0);
    let main: Vec<_> = (1..=6)
        .map(|i| {
            let path = wav(&format!("oo11-song{i}"), 4.0, Side::Left);
            file_item(100 + i, &format!("Song {}", 100 + i), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut cmds = vec![Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: main,
        at: None,
        lock: false,
    }];
    cmds.extend((101..=103).map(|i| Command::ItemLock {
        item: radio_model::ItemId(i),
    }));
    p.dispatch(Command::StationBatch { commands: cmds })
        .unwrap();
    program.extend(run(&p, &clock, 20.0));

    let song = first_audible(&program, 0, 0).expect("the songs play");
    assert!(
        song < 4 * SR,
        "the songs start soon after they are added: {song}"
    );
    assert_back_to_back(&p, &[101, 102, 103]);
    let start = first_audible(&program, 1, 0).expect("the one-off airs");
    assert!(
        start.abs_diff(20 * SR) <= SR,
        "it starts in its tolerance: {start}"
    );
}

#[test]
fn a_soft_one_off_waiting_on_a_deck_waits_for_locked_songs_added_in_its_way() {
    // The one-off at 20 s loads at once on a quiet station. Then a person adds eight songs
    // and locks the first seven. Songs 106 and 107 run from 19.5 s to 26.5 s, past its time.
    let mut engine = Engine::open_in_memory().unwrap();
    let cmd = soft(promo("oo12-promo", "Soft promo"), at(13, 0, 20));
    let id = engine
        .dispatch(cmd)
        .unwrap()
        .data
        .and_then(|d| d.as_u64())
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let mut program = run(&p, &clock, 2.0);
    let main: Vec<_> = (1..=8)
        .map(|i| {
            let path = wav(&format!("oo12-song{i}"), 4.0, Side::Left);
            file_item(100 + i, &format!("Song {}", 100 + i), &path, 4.0, 3.5, 0.5)
        })
        .collect();
    let mut cmds = vec![Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: main,
        at: None,
        lock: false,
    }];
    cmds.extend((101..=107).map(|i| Command::ItemLock {
        item: radio_model::ItemId(i),
    }));
    p.dispatch(Command::StationBatch { commands: cmds })
        .unwrap();
    program.extend(run(&p, &clock, 30.0));

    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert_aired_late(&p, &program, &raised, id, 20 * SR);
    assert_back_to_back(&p, &[101, 102, 103, 104, 105, 106, 107]);
    let start = first_audible(&program, 1, 0).unwrap();
    assert!(
        start + SR / 100 >= handover(&p, 107),
        "it starts after the last locked song in its way: {start}"
    );
}

/// With two decks the soft one-off is the third item in line when its time passes. It still
/// airs after the locked songs in its way, as it does with four decks.
#[test]
fn a_soft_one_off_behind_locked_songs_still_airs_with_two_decks() {
    let mut engine = fixed_songs("oo12", &[1, 2, 3]);
    set_decks(&mut engine, 2);
    let cmd = soft(promo("oo12-promo", "Soft promo"), at(13, 0, 5));
    let id = engine
        .dispatch(cmd)
        .unwrap()
        .data
        .and_then(|d| d.as_u64())
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let program = run(&p, &clock, 16.0);
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();
    assert_aired_late(&p, &program, &raised, id, 5 * SR);
    assert_back_to_back(&p, &[1, 2, 3]);
}

/// The same for a soft event of a rule: it airs late with its alarm, and never just leaves
/// the plan.
#[test]
fn a_soft_rule_event_behind_locked_songs_still_airs_with_two_decks() {
    let mut engine = fixed_songs("oo13", &[1, 2, 3]);
    set_decks(&mut engine, 2);
    let script = r#"rule({at: {date: "2026-11-21", time: "13:00:05", repeat: "once"},
                place: {fixed: "soft", tolerance: 1}, take: 1, constraints: []});"#;
    let promo = promo("oo13-promo", "Soft promo");
    add_playlist(&mut engine, "Promos", vec![promo], script);
    let (p, clock) = offline(engine, |_| {});
    let alarms = p.alarms().unwrap();
    let program = run(&p, &clock, 16.0);
    let raised: Vec<_> = alarms.try_iter().map(|a| a.alarm).collect();

    let start = first_audible(&program, 1, 0).expect("the soft event airs");
    assert!(
        start + SR / 100 >= handover(&p, 3),
        "it starts after the last locked song in its way: {start}"
    );
    assert!(
        raised.iter().any(|a| matches!(a, Alarm::FixedLate { .. })),
        "a late soft event raises the alarm: {raised:?}"
    );
}
