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
