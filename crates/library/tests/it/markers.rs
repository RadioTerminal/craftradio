//! Intro and outro markers found by the analysis.

use radio_library::analyse;
use radio_model::MarkerSource;
use std::{path::Path, time::Duration};

/// Writes a mono 48 kHz tone in parts of (seconds, peak level in dBFS).
fn parts(path: &Path, parts: &[(u32, f32)]) {
    let mut wav = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    let mut n = 0u32;
    for &(secs, db) in parts {
        let amp = 10f32.powf(db / 20.0);
        for _ in 0..secs * 48000 {
            let v = amp * (n as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin();
            wav.write_sample((v * i16::MAX as f32) as i16).unwrap();
            n += 1;
        }
    }
    wav.finalize().unwrap();
}

fn near(a: Duration, b: Duration, tol: Duration) -> bool {
    a.abs_diff(b) <= tol
}

#[test]
fn a_thin_intro_and_outro_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.wav");
    parts(&path, &[(4, -22.0), (20, -10.0), (6, -22.0)]);
    let t = analyse(&path).unwrap().timing;
    let intro = t.intro_end.expect("an intro");
    let outro = t.outro_start.expect("an outro");
    let tol = Duration::from_secs(1);
    assert!(near(intro, Duration::from_secs(4), tol), "intro {intro:?}");
    assert!(near(outro, Duration::from_secs(24), tol), "outro {outro:?}");
    assert!(
        t.cue_in < intro && intro < outro && outro < t.segue,
        "{t:?}"
    );
    assert_eq!(t.markers, MarkerSource::Detected);
}

#[test]
fn a_song_loud_from_the_start_has_no_intro() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cold.wav");
    parts(&path, &[(20, -10.0), (6, -22.0)]);
    let t = analyse(&path).unwrap().timing;
    assert_eq!(t.intro_end, None);
    assert!(t.outro_start.is_some());
}

#[test]
fn a_song_that_ends_cold_has_no_outro() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cold_end.wav");
    parts(&path, &[(4, -22.0), (20, -10.0)]);
    let t = analyse(&path).unwrap().timing;
    assert!(t.intro_end.is_some());
    assert_eq!(t.outro_start, None);
}
