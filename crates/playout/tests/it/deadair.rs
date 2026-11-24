use crate::fixtures::*;
use radio_playout::Alarm;
use std::time::Duration;

/// A missing file gets an alarm while the item before it still plays. The next item
/// takes its place at the segue. The log never shows the missing item.
#[test]
fn missing_file_is_skipped_before_air() {
    let one = file_item(1, "One", &wav("m-one", 3.0, Side::Left), 3.0, 2.5, 0.5);
    let gone = file_item(2, "Gone", "/nonexistent/gone.wav", 3.0, 2.5, 0.5);
    let three = file_item(3, "Three", &wav("m-three", 3.0, Side::Right), 3.0, 2.5, 0.5);
    let (p, clock) = offline(engine_with(vec![one, gone, three]), |_| {});
    let alarms = p.alarms().unwrap();
    let program = run(&p, &clock, 1.0);
    let skipped = alarms
        .try_iter()
        .find(|a| matches!(&a.alarm, Alarm::ItemSkipped { item, .. } if item.0 == 2));
    assert!(skipped.is_some_and(|a| a.at < t0() + Duration::from_millis(2500)));
    let rest = run(&p, &clock, 3.0);
    let program = [program, rest].concat();

    let start1 = first_audible(&program, 0, 0).expect("item one sounds");
    let start3 = first_audible(&program, 1, 0).expect("item three sounds");
    let segue = start1 + (2.5 * SR as f32) as usize;
    assert!(
        start3.abs_diff(segue) <= SR / 200,
        "item three at {start3}, segue {segue}"
    );
    let rows = p.history(t0(), 10).unwrap();
    let titles: Vec<&str> = rows.iter().map(|r| r.entry.title.as_str()).collect();
    assert_eq!(titles, ["One", "Three"]);
}

/// A silent item makes playout skip. When the next item is silent too, the emergency
/// folder plays and the alarms say so.
#[test]
fn silence_falls_back_to_emergency_playlist() {
    let quiet = file_item(
        1,
        "Quiet",
        &wav("q-one", 10.0, Side::Silent),
        10.0,
        9.5,
        0.0,
    );
    let hush = file_item(2, "Hush", &wav("q-two", 10.0, Side::Silent), 10.0, 9.5, 0.0);
    let backup = wav("backup", 5.0, Side::Both);
    let folder = std::path::Path::new(&backup)
        .parent()
        .unwrap()
        .to_path_buf();
    let (p, clock) = offline(engine_with(vec![quiet, hush]), |cfg| {
        cfg.mixer.silence.after = Duration::from_millis(500);
        cfg.fallback_folder = Some(folder);
    });
    let program = run(&p, &clock, 3.0);

    let heard = first_audible(&program, 0, 0).expect("the fallback sounds");
    assert!(heard < 2 * SR, "fallback after {heard} frames");
    let view = p.snapshot();
    assert!(view.emergency);
    let kinds: Vec<&Alarm> = view.alarms.iter().map(|a| &a.alarm).collect();
    assert!(kinds.contains(&&Alarm::DeadAir));
    assert!(
        kinds
            .iter()
            .any(|a| matches!(a, Alarm::EmergencyFallback { .. }))
    );
    let rows = p.history(t0(), 10).unwrap();
    assert!(rows.iter().any(|r| r.entry.origin == "emergency"));
}

/// The emergency folder gives way to the plan by itself once a planned item is ready.
#[test]
fn emergency_fallback_returns_to_the_plan() {
    let quiet = file_item(
        1,
        "Quiet",
        &wav("r-one", 10.0, Side::Silent),
        10.0,
        9.5,
        0.0,
    );
    let hush = file_item(2, "Hush", &wav("r-two", 10.0, Side::Silent), 10.0, 9.5, 0.0);
    let tune = file_item(3, "Tune", &wav("r-three", 3.0, Side::Right), 3.0, 2.5, 0.0);
    let backup = wav("r-backup", 5.0, Side::Left);
    let folder = std::path::Path::new(&backup)
        .parent()
        .unwrap()
        .to_path_buf();
    let (p, clock) = offline(engine_with(vec![quiet, hush, tune]), |cfg| {
        cfg.mixer.silence.after = Duration::from_millis(500);
        cfg.emergency_retry = Duration::from_secs(1);
        cfg.fallback_folder = Some(folder);
    });
    let program = run(&p, &clock, 4.0);

    let fallback = first_audible(&program, 0, 0).expect("the fallback sounds");
    let planned = first_audible(&program, 1, 0).expect("the plan came back");
    assert!(planned > fallback);
    let view = p.snapshot();
    assert!(!view.emergency);
    assert!(
        view.alarms.iter().all(|a| {
            !matches!(a.alarm, Alarm::EmergencyFallback { .. }) || a.cleared.is_some()
        })
    );
}

/// Fallback files that do not open are tried once each, and then an alarm says that the
/// fallback has nothing to play.
#[test]
fn unreadable_fallback_files_raise_an_alarm() {
    let quiet = file_item(
        1,
        "Quiet",
        &wav("u-one", 10.0, Side::Silent),
        10.0,
        9.5,
        0.0,
    );
    let first = wav("u-bad-one", 0.1, Side::Both);
    let folder = std::path::Path::new(&first).parent().unwrap().to_path_buf();
    std::fs::write(&first, b"not audio").unwrap();
    std::fs::write(folder.join("u-bad-two.wav"), b"not audio either").unwrap();
    let (p, clock) = offline(engine_with(vec![quiet]), |cfg| {
        cfg.mixer.silence.after = Duration::from_millis(500);
        cfg.fallback_folder = Some(folder);
    });
    run(&p, &clock, 3.0);

    let view = p.snapshot();
    assert!(view.emergency);
    let reasons: Vec<&str> = view
        .alarms
        .iter()
        .filter_map(|a| match &a.alarm {
            Alarm::EmergencyFallback { reason } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains("did not open")),
        "no alarm for the unreadable fallback files: {reasons:?}"
    );
}

/// Silence that began while the station was quiet on purpose counts as dead air once the
/// station is in `Auto` again and still has nothing to play.
#[test]
fn silence_from_manual_mode_is_dead_air_after_the_switch_to_auto() {
    let gone = file_item(1, "Gone", "/nonexistent/gone.wav", 3.0, 2.5, 0.5);
    let backup = wav("s-backup", 5.0, Side::Both);
    let folder = std::path::Path::new(&backup)
        .parent()
        .unwrap()
        .to_path_buf();
    let mut engine = engine_with(vec![gone]);
    engine
        .dispatch(radio_engine::Command::ModeSet {
            mode: radio_engine::Mode::Manual,
        })
        .unwrap();
    let (p, clock) = offline(engine, |cfg| {
        cfg.mixer.silence.after = Duration::from_millis(500);
        cfg.fallback_folder = Some(folder);
    });
    run(&p, &clock, 1.5);
    assert!(!p.snapshot().emergency, "Manual mode is quiet on purpose");
    let _ = p
        .send(radio_engine::Command::ModeSet {
            mode: radio_engine::Mode::Auto,
        })
        .unwrap();
    let program = run(&p, &clock, 3.0);

    assert!(
        p.snapshot()
            .alarms
            .iter()
            .any(|a| a.alarm == Alarm::DeadAir),
        "no dead air alarm after the switch to Auto"
    );
    assert!(
        first_audible(&program, 0, 0).is_some(),
        "the fallback never played"
    );
}

/// An operator start after a long quiet time in `Assist` mode is not dead air: the item
/// plays on, and no alarm comes.
#[test]
fn an_operator_start_after_a_quiet_time_is_not_dead_air() {
    let one = file_item(1, "One", &wav("a-one", 4.0, Side::Left), 4.0, 3.5, 0.5);
    let backup = wav("a-backup", 5.0, Side::Right);
    let folder = std::path::Path::new(&backup)
        .parent()
        .unwrap()
        .to_path_buf();
    let mut engine = engine_with(vec![one]);
    engine
        .dispatch(radio_engine::Command::ModeSet {
            mode: radio_engine::Mode::Assist,
        })
        .unwrap();
    let (p, clock) = offline(engine, |cfg| {
        cfg.mixer.silence.after = Duration::from_millis(500);
        cfg.fallback_folder = Some(folder);
    });
    run(&p, &clock, 1.5);
    let deck = p
        .snapshot()
        .decks
        .iter()
        .position(|d| d.item == Some(radio_model::ItemId(1)))
        .expect("item one waits on a deck");
    let _ = p
        .send(radio_engine::Command::DeckPlay {
            deck: deck as u8,
            expect_item: Some(radio_model::ItemId(1)),
            fade_in: Duration::ZERO,
        })
        .unwrap();
    let program = run(&p, &clock, 2.0);

    assert!(
        peak(&program, 0, SR, 2 * SR) > 0.1,
        "the item the operator started did not stay on air"
    );
    let view = p.snapshot();
    assert!(!view.emergency);
    assert!(view.alarms.iter().all(|a| a.alarm != Alarm::DeadAir));
}
