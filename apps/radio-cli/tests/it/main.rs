use std::path::PathBuf;
use std::process::{Command, Output};

const NOW: &str = "2026-11-21T13:00:00Z";

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/station.toml")
}

fn plan(station: &std::path::Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radio-cli"))
        .arg("plan")
        .arg(station)
        .args(["--now", NOW])
        .args(extra)
        .output()
        .expect("radio-cli runs")
}

fn station_file(toml: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.toml");
    std::fs::write(&path, toml).unwrap();
    (dir, path)
}

/// One main playlist holding one item; `item_fields` completes the item.
fn one_item_station(item_fields: &str) -> String {
    format!(
        r#"
tz = "UTC"

[[playlists]]
id = 1
name = "Music"
color = [1, 2, 3]
is_main = true
is_fill_pool = false
priority = 0
pick = "InOrder"
rules = []
constraints = []

[[playlists.items]]
id = 7
locked = false
{item_fields}
"#
    )
}

#[test]
fn plan_example_prints_news_at_1500() {
    let out = plan(&example(), &["--hours", "4"]);
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let news = lines
        .iter()
        .position(|l| l.contains("15:00:00") && l.contains("News"))
        .unwrap_or_else(|| panic!("no news line in:\n{text}"));
    let fit = lines.get(news + 1).copied().unwrap_or_default();
    assert!(
        fit.contains("fit:") && fit.contains('✓'),
        "fit line: {fit:?}"
    );
}

#[test]
fn invalid_station_exits_1_with_item_name() {
    let (_dir, path) = station_file(&one_item_station(
        "title = \"Mystery Reel\"\nsource = { File = { path = \"reel.wav\" } }",
    ));
    let out = plan(&path, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("Mystery Reel"), "stderr: {err}");
}

#[test]
fn dead_air_exits_2() {
    let (_dir, path) = station_file(
        r#"
tz = "UTC"

[[playlists]]
id = 1
name = "Music"
color = [1, 2, 3]
is_main = true
is_fill_pool = false
priority = 0
pick = "InOrder"
items = []
rules = []
constraints = []
"#,
    );
    let out = plan(&path, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!out.stderr.is_empty());
}
