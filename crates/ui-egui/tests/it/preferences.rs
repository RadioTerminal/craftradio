//! This computer's preferences across a restart: the windows open as they were left.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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

/// A station whose output device was changed in Preferences, while the playout still plays to
/// the device it opened at the start.
struct OutputChanged(MockSource);

impl radio_ui_egui::ViewSource for OutputChanged {
    fn snapshot(&mut self, now: Timestamp) -> radio_ui_egui::view::StationView {
        let mut v = self.0.snapshot(now);
        v.preview = false;
        v.device_roles.out = "Built-in Output".to_owned();
        v.settings
            .entry("audio".to_owned())
            .or_default()
            .insert("out".to_owned(), serde_json::json!("Studio Interface"));
        v
    }
    fn edit(&mut self, edit: radio_ui_egui::view::Edit) -> Result<usize, String> {
        self.0.edit(edit)
    }
    fn stream_title(&self, row: &radio_ui_egui::view::Row) -> String {
        self.0.stream_title(row)
    }
}

#[test]
fn a_new_output_device_shows_as_picked_and_says_it_waits_for_the_next_start() {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(
                Box::new(OutputChanged(MockSource::new(tz))),
                Box::new(fixed_clock),
            )
        });
    harness.run_steps(FRAMES);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    harness.run_steps(FRAMES);
    harness.get_by_label("Devices").click();
    harness.run_steps(FRAMES);
    let shown: Vec<String> = harness
        .query_all_by_label("Output device (Out)")
        .filter_map(|n| n.accesskit_node().value())
        .collect();
    assert!(
        shown.iter().any(|v| v.contains("Studio Interface")),
        "the row shows the device that was picked: {shown:?}"
    );
    assert!(
        harness
            .query_by_label_contains(
                "Opens at the next start. Until then the sound goes to Built-in Output."
            )
            .is_some(),
        "and says where the sound goes until then"
    );
}

/// A station whose Icecast password is set.
struct PasswordSet(MockSource);

impl radio_ui_egui::ViewSource for PasswordSet {
    fn snapshot(&mut self, now: Timestamp) -> radio_ui_egui::view::StationView {
        let mut v = self.0.snapshot(now);
        v.icecast.password_set = true;
        v
    }
    fn edit(&mut self, edit: radio_ui_egui::view::Edit) -> Result<usize, String> {
        self.0.edit(edit)
    }
    fn stream_title(&self, row: &radio_ui_egui::view::Row) -> String {
        self.0.stream_title(row)
    }
}

/// The hint of the empty password field: the placeholder of the field named "Password".
fn password_hint(harness: &Harness<'_, RadioApp>) -> String {
    harness
        .query_all_by_label("Password")
        .filter_map(|n| n.accesskit_node().placeholder().map(str::to_owned))
        .next()
        .unwrap_or_default()
}

#[test]
fn the_password_hint_asks_again_once_the_server_changes() {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(
                Box::new(PasswordSet(MockSource::new(tz))),
                Box::new(fixed_clock),
            )
        });
    harness.run_steps(FRAMES);
    harness.get_by_label("Mixer").click();
    harness.run_steps(FRAMES);
    harness.get_by_label("Icecast preferences").click();
    harness.run_steps(FRAMES);
    assert_eq!(password_hint(&harness), "set · leave empty to keep it");
    let host = harness
        .query_all_by_label("Host")
        .find(|n| n.accesskit_node().role() == egui::accesskit::Role::TextInput)
        .expect("the Host field");
    host.focus();
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness
        .query_all_by_label("Host")
        .find(|n| n.accesskit_node().role() == egui::accesskit::Role::TextInput)
        .expect("the Host field")
        .type_text("relay.example.net");
    harness.run_steps(FRAMES);
    assert_eq!(
        password_hint(&harness),
        "type it again for this server",
        "the kept password belongs to the old server"
    );
}
