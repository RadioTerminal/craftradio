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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_craftradio"));
    // The station's control channel keeps its token files here, not in the user's own folder.
    let control = std::env::temp_dir().join(format!("craftradio-control-{}", std::process::id()));
    cmd.env("CRAFTRADIO_CONTROL_DIR", control);
    cmd
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

/// A first run whose station file is broken fails, and the next start tries the import again.
#[test]
fn a_failed_first_import_is_tried_again() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let file = dir.path().join("station.toml");
    std::fs::write(&file, "this is not a station").unwrap();
    let run = || {
        radio()
            .args([
                "--headless",
                "--null-output",
                "--run-for",
                "100ms",
                "--station",
            ])
            .arg(&file)
            .arg("--data-dir")
            .arg(&data)
            .output()
            .unwrap()
    };
    assert!(!run().status.success(), "a broken station file fails");
    tone(&dir.path().join("song.wav"));
    std::fs::write(
        &file,
        r#"tz = "UTC"

[[playlists]]
id = 1
name = "Music"
color = [60, 120, 200]
is_main = true
is_fill_pool = false
priority = 0
pick = "InOrder"
rules = []
constraints = []

[[playlists.items]]
id = 1
title = "Song"
source = { File = { path = "song.wav" } }
locked = false
planned_len = { secs = 5, nanos = 0 }
"#,
    )
    .unwrap();
    let second = run();
    let text = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(text.contains("first run: imported"), "{text}");
}

/// A watched folder that does not read at start keeps its items, marked missing. When the
/// folder is back with the file under another name, the item relinks by its content.
#[test]
fn a_missing_folder_keeps_its_items_and_a_moved_file_relinks() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let music = dir.path().join("Music");
    std::fs::create_dir_all(&music).unwrap();
    tone(&music.join("song.wav"));
    let file = dir.path().join("station.toml");
    std::fs::write(
        &file,
        format!(
            r#"tz = "UTC"

[[playlists]]
id = 1
name = "Music"
color = [60, 120, 200]
is_main = true
is_fill_pool = false
priority = 0
pick = "InOrder"
rules = []
constraints = []
source = {{ path = "{}", recursive = false, watch = true }}

[[playlists.items]]
id = 1
title = "Song"
source = {{ File = {{ path = "Music/song.wav" }} }}
locked = false
planned_len = {{ secs = 5, nanos = 0 }}
"#,
            music.display()
        ),
    )
    .unwrap();
    let run = || {
        let out = radio()
            .args([
                "--headless",
                "--null-output",
                "--run-for",
                "100ms",
                "--station",
            ])
            .arg(&file)
            .arg("--data-dir")
            .arg(&data)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    run();
    let away = dir.path().join("Music-away");
    std::fs::rename(&music, &away).unwrap();
    let text = run();
    assert!(text.contains("Music: 1 missing"), "{text}");
    assert!(
        !text.contains("gone"),
        "a folder that does not read removes nothing: {text}"
    );
    std::fs::rename(&away, &music).unwrap();
    std::fs::rename(music.join("song.wav"), music.join("moved.wav")).unwrap();
    let text = run();
    assert!(text.contains("1 relinked"), "{text}");
}

/// A file added after headless startup is imported without a restart.
#[test]
fn headless_watches_files_added_after_startup() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let dir = tempfile::tempdir().unwrap();
    let mut child = radio()
        .args([
            "--headless",
            "--null-output",
            "--run-for",
            "4s",
            "--data-dir",
        ])
        .arg(dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "headless stopped before watches started"
        );
        if line.contains("folder watches started") {
            break;
        }
    }
    tone(&dir.path().join("Music").join("after-start.wav"));
    assert!(child.wait().unwrap().success());
    let engine = radio_engine::Engine::open(dir.path().join("station.db")).unwrap();
    assert!(engine.state().station.playlists.iter().flat_map(|p| &p.items)
        .any(|i| matches!(&i.source, radio_model::Source::File { path } if path.ends_with("after-start.wav"))));
}

/// The headless status line says whether the station is on air, and says off air when the
/// saved setting is off.
#[test]
fn the_headless_status_line_says_off_air() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let run = || {
        let out = radio()
            .args(["--headless", "--null-output", "--run-for", "300ms"])
            .arg("--data-dir")
            .arg(&data)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let on = run();
    assert!(on.contains("radio: on air"), "{on}");
    assert!(!on.contains("off air"), "{on}");

    let mut engine = radio_engine::Engine::open(data.join("station.db")).unwrap();
    engine
        .dispatch(radio_engine::Command::SettingsSet {
            part: radio_model::AIR_PART.into(),
            key: radio_model::ON_AIR_KEY.into(),
            value: false.into(),
        })
        .unwrap();
    drop(engine);
    let off = run();
    assert!(off.contains("radio: off air"), "{off}");
}

/// The headless app prints the pairing with a station server at start, in the words of the
/// Server page. A fresh station is not paired, and that is said, not left out.
#[test]
fn headless_prints_the_pairing_state() {
    let dir = tempfile::tempdir().unwrap();
    let out = radio()
        .args(["--headless", "--null-output", "--run-for", "300ms"])
        .arg("--data-dir")
        .arg(dir.path().join("data"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("radio: Not paired"), "{text}");
}
