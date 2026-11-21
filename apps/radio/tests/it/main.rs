use std::process::Command;

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
    let run = || {
        radio()
            .args([
                "--headless",
                "--null-output",
                "--run-for",
                "300ms",
                "--data-dir",
            ])
            .arg(&data)
            .output()
            .unwrap()
    };
    let first = run();
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
    let second = run();
    assert!(second.status.success());
    let text = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(
        !text.contains("first run"),
        "the second run does not seed: {text}"
    );
}
