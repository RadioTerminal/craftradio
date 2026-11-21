//! Loudness normalisation on the decks: each item plays at the gain that brings it to
//! the station target.

use std::time::{Duration, Instant};

use radio_engine::{Command, Engine};
use radio_model::{Clamp, GainSource, ItemId, PEAK_CEILING_DB};
use radio_playout::{Playout, measure_file};

use crate::fixtures::{SR, engine_with, file_item, offline, rms_db, run, wav_at};

/// Stores the real measurement of the file of `item` in the engine, as playout would.
fn seed(engine: &mut Engine, item: u64, path: &str) {
    let m = measure_file(std::path::Path::new(path)).expect("a tone measures");
    engine
        .dispatch(Command::ItemSetLoudness {
            item: ItemId(item),
            hash: m.hash,
            size: m.size,
            lufs: m.loudness.lufs,
            peak_db: m.loudness.peak_db,
        })
        .unwrap();
}

/// A loud tone, then a tone 12 dB quieter, back to back.
fn two_tones(name: &str) -> (Engine, [String; 2]) {
    let loud = wav_at(&format!("{name}-loud"), 3.0, 0.25, None);
    let quiet = wav_at(&format!("{name}-quiet"), 3.0, 0.0625, None);
    let mut engine = engine_with(vec![
        file_item(1, "Copper Field", &loud, 3.0, 3.0, 0.0),
        file_item(2, "Glass Meadow", &quiet, 3.0, 3.0, 0.0),
    ]);
    seed(&mut engine, 1, &loud);
    seed(&mut engine, 2, &quiet);
    (engine, [loud, quiet])
}

/// The level of the first and of the second tone, away from their edges.
fn levels(program: &[f32]) -> (f32, f32) {
    (
        rms_db(program, 0, SR / 2, 5 * SR / 2),
        rms_db(program, 0, 7 * SR / 2, 11 * SR / 2),
    )
}

/// Two tones 12 dB apart come out at about the same level with normalisation on, and
/// still about 12 dB apart with it off.
#[test]
fn normalisation_evens_out_two_tones_12_db_apart() {
    let (engine, _) = two_tones("even");
    let (p, clock) = offline(engine, |_| {});
    let (a, b) = levels(&run(&p, &clock, 6.0));
    assert!((a - b).abs() < 1.5, "on: {a} dB then {b} dB");

    let (mut engine, _) = two_tones("even-off");
    engine
        .dispatch(Command::MixerSetLoudness {
            enabled: Some(false),
            target_lufs: None,
            max_boost_db: None,
        })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let (a, b) = levels(&run(&p, &clock, 6.0));
    assert!((a - b - 12.0).abs() < 1.5, "off: {a} dB then {b} dB");
}

/// The deck shows the gain it applies, where it comes from, and the target.
#[test]
fn the_deck_shows_the_measured_gain_it_applies() {
    let (engine, _) = two_tones("shown");
    let (p, clock) = offline(engine, |_| {});
    run(&p, &clock, 0.5);
    let deck = p
        .snapshot()
        .decks
        .into_iter()
        .find(|d| d.item == Some(ItemId(1)))
        .expect("item one on a deck");
    let gain = deck.gain.expect("a file item has a gain");
    assert_eq!(gain.source, GainSource::Measured);
    assert!(gain.db < 0.0, "the loud tone is cut: {}", gain.db);
    assert_eq!(gain.target_lufs, -18.0);
}

/// A quiet file with high peaks is not boosted past the peak ceiling: its gain stops
/// where its peak reaches −1 dBTP, well short of what its loudness asks for.
#[test]
fn a_peaky_quiet_file_is_not_boosted_past_the_ceiling() {
    let path = wav_at("peaky", 3.0, 0.02, Some((4_800, 0.7)));
    let mut engine = engine_with(vec![file_item(1, "Pin Drop", &path, 3.0, 3.0, 0.0)]);
    seed(&mut engine, 1, &path);
    let (p, clock) = offline(engine, |_| {});
    let on = run(&p, &clock, 2.0);
    let gain = p.snapshot().decks.iter().find_map(|d| d.gain).unwrap();
    let peak = gain.loudness.and_then(|l| l.peak_db).unwrap();
    assert_eq!(gain.clamp, Some(Clamp::Peak));
    assert!(
        gain.db > 0.0,
        "the file still gets the boost up to the ceiling"
    );
    assert!(
        peak + gain.db <= PEAK_CEILING_DB + 0.01,
        "{peak} + {}",
        gain.db
    );
    let wanted = gain.target_lufs - gain.loudness.unwrap().lufs;
    assert!(gain.db < wanted - 6.0, "boost {} of {wanted}", gain.db);

    // The quiet body rises by the clamped gain, not by the loudness gap.
    let mut engine = engine_with(vec![file_item(1, "Pin Drop", &path, 3.0, 3.0, 0.0)]);
    engine
        .dispatch(Command::MixerSetLoudness {
            enabled: Some(false),
            target_lufs: None,
            max_boost_db: None,
        })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let off = run(&p, &clock, 2.0);
    let rise = rms_db(&on, 0, SR / 2, 3 * SR / 2) - rms_db(&off, 0, SR / 2, 3 * SR / 2);
    assert!(
        (rise - gain.db).abs() < 0.5,
        "the body rose {rise} dB for a {} dB gain",
        gain.db
    );
}

/// Renders in 10 ms steps until `done` holds, waiting on the wall clock between steps
/// so the measurement worker can finish.
fn run_until(
    p: &Playout,
    clock: &radio_playout::ManualClock,
    done: impl Fn(&Playout) -> bool,
) -> Vec<f32> {
    let start = Instant::now();
    let mut out = Vec::new();
    while !done(p) {
        assert!(start.elapsed() < Duration::from_secs(30), "no measurement");
        out.extend(run(p, clock, 0.01));
        std::thread::sleep(Duration::from_millis(5));
    }
    out
}

/// Playout measures a file that it has no loudness for, keeps it in the station, and a
/// measurement that arrives on air ramps the gain: no 10 ms block jumps by more than a
/// fraction of the change.
#[test]
fn a_measurement_on_air_is_cached_and_ramps_in() {
    let path = wav_at("on-air", 8.0, 0.25, None);
    let engine = engine_with(vec![file_item(1, "Late Lantern", &path, 8.0, 8.0, 0.0)]);
    let (p, clock) = offline(engine, |cfg| cfg.measure = measure_file);
    let mut out = run_until(&p, &clock, |p| {
        p.snapshot().state.loudness_of(ItemId(1)).is_some()
    });
    assert!(out.len() / 2 < 6 * SR, "measured with room to spare");
    let before = rms_db(&out, 0, out.len() / 2 - SR / 10, out.len() / 2);
    out.extend(run(&p, &clock, 2.0));
    let gain = p.snapshot().decks.iter().find_map(|d| d.gain).unwrap();
    assert_eq!(gain.source, GainSource::Measured);
    let end = out.len() / 2;
    let after = rms_db(&out, 0, end - SR / 10, end);
    assert!(
        (after - before - gain.db).abs() < 0.5,
        "{before} → {after}, gain {}",
        gain.db
    );
    let blocks: Vec<f32> = (10..end / (SR / 100) - 1)
        .map(|k| rms_db(&out, 0, k * SR / 100, (k + 1) * SR / 100))
        .collect();
    let biggest = blocks
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        biggest < gain.db.abs() / 4.0,
        "a {biggest} dB step in one block"
    );
}
