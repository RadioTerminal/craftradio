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
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("dead air"), "stderr: {err}");
}

#[test]
fn version_prints_without_a_subcommand() {
    let out = Command::new(env!("CARGO_BIN_EXE_radio-cli"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
    assert!(text.starts_with("radio-cli "));
}

fn cmd(data_dir: &std::path::Path, json: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radio-cli"))
        .arg("cmd")
        .arg("--data-dir")
        .arg(data_dir)
        .arg(json)
        .output()
        .expect("radio-cli runs")
}

#[test]
fn cmd_schedules_a_one_off_in_a_station_folder() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("station.db");
    radio_engine::Engine::open(&db).unwrap().flush().unwrap();
    let add = r#"{"id": "schedule.add", "args": {
        "one_off": {"id": 0, "title": "Station ID", "locked": false,
                    "source": {"File": {"path": "/music/id.wav"}},
                    "timing": null, "planned_len": {"secs": 20, "nanos": 0},
                    "valid_from": null, "valid_to": null, "max_per_day": null},
        "start": "2026-11-22T09:00:00", "fix": "Soft",
        "tolerance": {"secs": 2, "nanos": 0}}}"#;
    let out = cmd(dir.path(), add);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = printed["data"].as_u64().expect("the new id");
    let engine = radio_engine::Engine::open(&db).unwrap();
    let ones = engine.state().one_offs();
    assert_eq!(ones.len(), 1);
    assert_eq!(ones[0].item.id.0, id);
    assert_eq!(ones[0].fix, radio_model::FixMode::Soft);
}

#[test]
fn cmd_reports_a_refused_command_and_a_missing_station() {
    let dir = tempfile::tempdir().unwrap();
    let out = cmd(dir.path(), r#"{"id": "station.get"}"#);
    assert!(!out.status.success(), "no station.db, no new station");
    assert!(!dir.path().join("station.db").exists());
    radio_engine::Engine::open(dir.path().join("station.db"))
        .unwrap()
        .flush()
        .unwrap();
    let out = cmd(
        dir.path(),
        r#"{"id": "schedule.remove", "args": {"id": 42}}"#,
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown item"));
    let out = cmd(dir.path(), "not json");
    assert!(!out.status.success());
}
