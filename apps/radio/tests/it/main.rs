use std::process::Command;

/// A short stereo WAV with a quiet tone.
fn tone(path: &std::path::Path) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..48_000 * 5 {
        let x = ((n as f32 * 0.0576).sin() * 8000.0) as i16;
        w.write_sample(x).unwrap();
        w.write_sample(x).unwrap();
    }
    w.finalize().unwrap();
}

fn radio() -> Command {
    Command::new(env!("CARGO_BIN_EXE_radio"))
}

/// The preview runs without a window and stops by itself.
#[test]
fn headless_demo_exits_cleanly() {
    let out = radio()
        .args(["--headless", "--demo", "--run-for", "1s"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A first run with no station seeds the example and keeps only the files that exist. A
/// second run opens the same station without seeding it again.
#[test]
fn a_first_run_seeds_the_example_station_once() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let run = |time: &str| {
        radio()
            .args([
                "--headless",
                "--null-output",
                "--run-for",
                time,
                "--data-dir",
            ])
            .arg(&data)
            .output()
            .unwrap()
    };
    let first = run("300ms");
    let text = String::from_utf8_lossy(&first.stdout).into_owned();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        text.contains("first run: the example station, 0 items"),
        "{text}"
    );
    assert!(
        data.join("Music").is_dir(),
        "the Music folder waits for files"
    );
    // A file that a person drops into Music while the app is closed joins at the next start.
    tone(&data.join("Music").join("dropped.wav"));
    // The playout plans within a second or two.
    let second = run("2s");
    assert!(second.status.success());
    let text = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(
        !text.contains("first run"),
        "the second run does not seed: {text}"
    );
    assert!(text.contains("Music: 1 new files"), "{text}");
    assert!(
        !text.contains(" 0 planned entries"),
        "the file is in the plan: {text}"
    );
}
