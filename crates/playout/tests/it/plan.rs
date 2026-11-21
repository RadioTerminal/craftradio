use crate::fixtures::*;
use radio_engine::{AsPlayed, Command, Engine, LogStatus};
use radio_model::{OnInterrupt, PlaylistId};
use radio_playout::{OutputChoice, Playout, PlayoutConfig};
use std::sync::Arc;
use std::time::Duration;

/// Track 2 starts within 5 ms of the segue of track 1, while track 1 still fades
/// out. Then track 3 follows, not track 1 again, and the log has a row for each start.
#[test]
fn two_tracks_crossfade_at_segue() {
    let one = file_item(1, "One", &wav("one", 2.0, Side::Left), 2.0, 1.5, 0.5);
    let two = file_item(2, "Two", &wav("two", 2.0, Side::Right), 2.0, 1.5, 0.5);
    let three = file_item(3, "Three", &wav("three", 2.0, Side::Both), 2.0, 1.5, 0.5);
    let (p, clock) = offline(engine_with(vec![one, two, three]), |_| {});
    let program = run(&p, &clock, 4.0);

    let start1 = first_audible(&program, 0, 0).expect("track 1 sounds");
    let start2 = first_audible(&program, 1, 0).expect("track 2 sounds");
    let segue = start1 + (1.5 * SR as f32) as usize;
    assert!(
        start2.abs_diff(segue) <= SR / 200,
        "track 2 at {start2}, segue at {segue}"
    );
    assert!(
        first_audible(&program, 0, start2 + SR / 10).is_some_and(|f| f < start2 + SR / 5),
        "track 1 does not overlap track 2"
    );

    let rows = p.history(t0(), 10).unwrap();
    let titles: Vec<&str> = rows.iter().map(|r| r.entry.title.as_str()).collect();
    assert_eq!(titles, ["One", "Two", "Three"]);
    assert_eq!(rows[0].status, Some(LogStatus::Aired));
    assert!(rows[0].end.is_some_and(|end| end > rows[1].entry.start));
}

/// A tone that is left for 2.5 s, then right: the side that sounds shows the position.
fn split_wav(name: &str) -> String {
    let path = std::env::temp_dir().join(format!("radio-split-{}-{name}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for n in 0..6 * SR {
        let x = ((n as f32 * 440.0 * std::f32::consts::TAU / SR as f32).sin() * 8000.0) as i16;
        let (l, r) = if n < 5 * SR / 2 { (x, 0) } else { (0, x) };
        w.write_sample(l).unwrap();
        w.write_sample(r).unwrap();
    }
    w.finalize().unwrap();
    path.to_string_lossy().into_owned()
}

/// Plays 3 s, then "crashes": the old playout stops dead with its rows open. Then it
/// restarts, and returns the sides that sound after the restart and the log rows.
fn crash_and_restart(rule: OnInterrupt) -> (bool, bool, Vec<AsPlayed>) {
    let db = std::env::temp_dir().join(format!("radio-crash-{}-{rule:?}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let song = file_item(1, "Song", &split_wav(&format!("{rule:?}")), 6.0, 5.5, 0.0);
    let mut engine = Engine::open(&db).unwrap();
    for cmd in [
        Command::PlaylistAddItems {
            playlist: PlaylistId(1),
            items: vec![song],
            at: None,
        },
        Command::PlaylistSetOnInterrupt {
            playlist: PlaylistId(1),
            on_interrupt: rule,
        },
    ] {
        engine.dispatch(cmd).unwrap();
    }
    engine.flush().unwrap();
    let (p, clock) = offline(engine, |_| {});
    run(&p, &clock, 3.0);
    // The writer takes jobs in order, so this read waits for the saves before it.
    p.history(t0(), 10).unwrap();
    std::mem::forget(p);
    let clock2 = clock.clone();
    let mut cfg = PlayoutConfig::new(Arc::new(clock2), OutputChoice::Offline);
    cfg.horizon = Duration::from_secs(60);
    let p = Playout::start(Engine::open(&db).unwrap(), cfg).unwrap();
    let after = run(&p, &clock, 0.5);
    let rows = p.history(t0(), 10).unwrap();
    (
        first_audible(&after, 0, 0).is_some(),
        first_audible(&after, 1, 0).is_some(),
        rows,
    )
}

/// After a crash, a `Resume` playlist continues at the position in the same log row. A
/// `Replay` playlist plays from the start, and the cut-off row ends as interrupted.
#[test]
fn restart_resumes_or_replays_by_playlist() {
    let (left, right, rows) = crash_and_restart(OnInterrupt::Resume);
    let titles: Vec<_> = rows.iter().map(|r| (r.status, r.entry.start)).collect();
    assert!(right && !left, "Resume did not continue at the position");
    assert_eq!(rows.len(), 1, "Resume opened a second row: {titles:?}");

    let (left, right, rows) = crash_and_restart(OnInterrupt::Replay);
    assert!(left && !right, "Replay did not start again: {left} {right}");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].status, Some(LogStatus::Interrupted));
    assert!(rows[0].end.is_some_and(|end| end <= rows[1].entry.start));
}

static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// An analysis that blocks its worker until the test releases it.
fn blocking_analysis(_: &std::path::Path) -> Option<radio_model::Timing> {
    while !RELEASE.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

/// Analysis jobs that never finish fill their lane, and the next item still opens:
/// an open never waits behind analysis.
#[test]
fn next_item_opens_while_analysis_is_busy() {
    let one = file_item(1, "One", &wav("busy-one", 2.0, Side::Left), 2.0, 1.5, 0.0);
    let mut engine = engine_with(vec![one]);
    let untimed: Vec<_> = (100..110)
        .map(|id| {
            let mut item = file_item(id, &format!("Untimed {id}"), "untimed.wav", 20.0, 19.0, 0.0);
            item.timing = None;
            item.planned_len = Some(Duration::from_secs(20));
            item
        })
        .collect();
    crate::fixtures::add_playlist(&mut engine, "Library", untimed, "");
    let (p, clock) = offline(engine, |cfg| cfg.analyse = blocking_analysis);
    let program = run(&p, &clock, 0.3);
    RELEASE.store(true, std::sync::atomic::Ordering::Relaxed);

    assert!(
        first_audible(&program, 0, 0).is_some(),
        "item one did not open"
    );
}

/// The item ids before the first fixed event in the plan, the item on air included.
fn window_before_event(p: &radio_playout::Playout) -> Vec<radio_model::ItemId> {
    p.snapshot()
        .timeline
        .entries
        .iter()
        .take_while(|e| e.fixed.is_none())
        .map(|e| e.item)
        .collect()
}

/// A fixed event is 2 minutes away, so its window keeps its order (it is frozen). An
/// operator adds a jingle at the front of the jingle playlist. A fresh plan would put the
/// new jingle in the window, but the edit does not throw the frozen order away.
#[test]
fn station_edit_keeps_the_frozen_window_order() {
    let path = wav("frozen", 3.0, Side::Left);
    let main: Vec<_> = (1..=8)
        .map(|id| file_item(id, &format!("Song {id}"), &path, 22.0, 21.0, 0.5))
        .collect();
    let mut engine = engine_with(main);
    let mut news = file_item(
        50,
        "News",
        &wav("frozen-news", 1.0, Side::Right),
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
        r#"rule({at: {date: "2026-11-21", time: "13:02:00", repeat: "once"},
                place: {fixed: "hard", tolerance: 1}, take: 1, constraints: []});"#,
    );
    let jingle = file_item(60, "Jingle", &path, 4.0, 4.0, 0.0);
    let jingles = add_playlist(
        &mut engine,
        "Jingles",
        vec![jingle],
        r#"rule({every: {tracks: 2}, place: "between", take: 1, constraints: []});"#,
    );
    // Hand-added items are locked, and a locked entry would end the window.
    for id in (1..=8).chain([50, 60]) {
        let item = radio_model::ItemId(id);
        engine.dispatch(Command::ItemUnlock { item }).unwrap();
    }
    let (p, clock) = offline(engine, |cfg| cfg.horizon = Duration::from_secs(600));
    run(&p, &clock, 1.0);
    let before = window_before_event(&p);
    assert!(
        before.contains(&radio_model::ItemId(60)),
        "the window has the jingle: {before:?}"
    );

    let new = file_item(61, "New jingle", &path, 4.0, 4.0, 0.0);
    let _ = p
        .send(Command::PlaylistAddItems {
            playlist: jingles,
            items: vec![new],
            at: Some(0),
        })
        .unwrap();
    let item = radio_model::ItemId(61);
    let _ = p.send(Command::ItemUnlock { item }).unwrap();
    run(&p, &clock, 0.5);
    assert_eq!(window_before_event(&p), before, "the frozen order changed");
}

/// A hard event started 3 s late, and playout crashes 5 s into it, after its target plus
/// its length. On restart it resumes the event at its position from the real start.
#[test]
fn restart_resumes_a_late_hard_event_from_its_real_start() {
    let db = std::env::temp_dir().join(format!("radio-crash-{}-late.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let mut engine = Engine::open(&db).unwrap();
    let carts: Vec<_> = (20..24)
        .map(|i| {
            let path = wav(&format!("crash{i}"), 9.0, Side::Silent);
            file_item(i, &format!("Cart {i}"), &path, 9.0, 9.0, 0.0)
        })
        .collect();
    add_playlist(&mut engine, "Carts", carts, "");
    // Left for 2.5 s, then right: the side shows the position.
    let mut news = file_item(10, "News", &split_wav("late-news"), 6.0, 6.0, 0.0);
    news.movable = false;
    news.trimmable = false;
    let script = r#"rule({at: {date: "2026-11-21", time: "13:00:03", repeat: "once"},
                place: {fixed: "hard", tolerance: 5}, take: 1, constraints: []});"#;
    let playlist = add_playlist(&mut engine, "News", vec![news], script);
    engine
        .dispatch(Command::PlaylistSetOnInterrupt {
            playlist,
            on_interrupt: OnInterrupt::Resume,
        })
        .unwrap();
    engine.flush().unwrap();
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
    run(&p, &clock, 0.2);
    for (deck, item) in decks {
        let expect_item = Some(radio_model::ItemId(item));
        let _ = p.send(Command::DeckPause { deck, expect_item }).unwrap();
    }
    run(&p, &clock, 5.8);
    for (deck, item) in decks {
        let expect_item = Some(radio_model::ItemId(item));
        let _ = p.send(Command::DeckSkip { deck, expect_item }).unwrap();
    }
    // The event starts when the decks free up, 3 s after its target.
    let before = run(&p, &clock, 5.0);
    let start = first_audible(&before, 0, 0).expect("the late event sounds");
    assert!(
        start < SR / 10,
        "the event started {start} frames after the decks freed"
    );
    p.history(t0(), 10).unwrap();
    std::mem::forget(p);

    let mut cfg = PlayoutConfig::new(Arc::new(clock.clone()), OutputChoice::Offline);
    cfg.horizon = Duration::from_secs(60);
    let p = Playout::start(Engine::open(&db).unwrap(), cfg).unwrap();
    let after = run(&p, &clock, 0.5);
    assert!(
        first_audible(&after, 1, 0).is_some() && first_audible(&after, 0, 0).is_none(),
        "the event did not resume at its position"
    );
}

/// A mute that the station stored before a restart still mutes Out after it.
#[test]
fn a_stored_mute_holds_after_a_restart() {
    let path = wav("muted", 2.0, Side::Both);
    let mut engine = engine_with(vec![file_item(1, "Song", &path, 2.0, 2.0, 0.0)]);
    engine
        .dispatch(Command::MixerSetStrip {
            strip: "Out".into(),
            gain_db: None,
            mute: Some(true),
            pfl: None,
            route: None,
        })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let program = run(&p, &clock, 1.0);
    assert!(
        peak(&program, 0, SR / 10, SR) < 0.001,
        "Out stays muted after the start"
    );
}

/// A fader drag sends many commands. They change no station data, so they cause no plan.
#[test]
fn a_fader_drag_causes_no_plans() {
    let path = wav("drag", 4.0, Side::Both);
    let engine = engine_with(vec![file_item(1, "Song", &path, 4.0, 4.0, 0.0)]);
    let (p, clock) = offline(engine, |_| {});
    let _ = run(&p, &clock, 0.1);
    let before = p.snapshot().plans;
    let steps = 40;
    for k in 0..steps {
        let _ = p
            .send(Command::MixerSetStrip {
                strip: "Out".into(),
                gain_db: Some(-(k as f32) / 4.0),
                mute: None,
                pfl: None,
                route: None,
            })
            .unwrap();
        let _ = run(&p, &clock, 0.01);
    }
    let after = p.snapshot().plans;
    eprintln!(
        "plans during a {steps}-step drag over 0.4 s: {}",
        after - before
    );
    assert!(
        after - before <= 1,
        "{} plans for {steps} fader steps",
        after - before
    );
}
