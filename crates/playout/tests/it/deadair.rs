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
