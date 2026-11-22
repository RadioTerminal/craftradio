//! This computer's preferences across a restart: the windows open as they were left.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{AppPrefs, MockSource, RadioApp};
use std::path::Path;

const FRAMES: usize = 6;

fn fixed_clock() -> Timestamp {
    "2026-11-21T13:58:00Z".parse().expect("valid timestamp")
}

/// The app with its preferences in `dir`, as the desktop app keeps them in config.toml.
fn app(dir: &Path) -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let dir = dir.to_path_buf();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(fixed_clock))
                .with_preferences(AppPrefs::open_in(dir))
        });
    harness.run_steps(FRAMES);
    harness
}

fn shows(harness: &Harness<'_, RadioApp>, label: &str) -> bool {
    harness.query_by_label(label).is_some()
}

/// Quits the app as the window closes: what is pending is saved.
fn quit(mut harness: Harness<'_, RadioApp>) {
    eframe::App::on_exit(harness.state_mut(), None);
}

#[test]
fn a_window_left_open_opens_again_after_a_restart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut harness = app(dir.path());
    assert!(
        !shows(&harness, "Clock preferences"),
        "the clock starts closed"
    );
    harness.get_by_label("Clock").click();
    harness.run_steps(FRAMES);
    assert!(shows(&harness, "Clock preferences"), "the clock opens");
    quit(harness);

    let again = app(dir.path());
    assert!(
        shows(&again, "Clock preferences"),
        "the clock is open again after the restart"
    );
}
