//! The Preferences window: its search narrows the rail, Restore page defaults resets a page,
//! and a section folds.

use egui::accesskit::Toggled;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use radio_ui_egui::view::{Edit, StationView, ViewSource};
use radio_ui_egui::{MockSource, RadioApp, ToolWindow};

const FRAMES: usize = 6;

/// The app with Preferences open on its first page.
fn preferences() -> Harness<'static, RadioApp> {
    let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
    let now: jiff::Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(move || now))
        });
    h.run_steps(FRAMES);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    h.run_steps(FRAMES);
    h
}

fn shows(h: &Harness<'_, RadioApp>, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

/// Clicks the control named `label` in Preferences. The main window may have one of the same
/// name (the Clock window's toggle, a deck's badge): Preferences draws after it, so its own
/// is the last.
fn click(h: &mut Harness<'_, RadioApp>, label: &str) {
    h.query_all_by_label(label)
        .last()
        .unwrap_or_else(|| panic!("no {label}"))
        .click();
    h.run_steps(FRAMES);
}

fn count(h: &Harness<'_, RadioApp>, label: &str) -> usize {
    h.query_all_by_label(label).count()
}

fn search(h: &mut Harness<'_, RadioApp>, text: &str) {
    h.get_by_label("Search preferences").click();
    h.run_steps(2);
    h.get_by_label("Search preferences").type_text(text);
    h.run_steps(FRAMES);
}

fn checked(h: &Harness<'_, RadioApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(Toggled::True)
}

#[test]
fn a_search_keeps_only_the_pages_that_match_and_shows_the_first() {
    let mut h = preferences();
    let clocks = count(&h, "Clock");
    assert!(shows(&h, "Decks") && clocks > 0);
    search(&mut h, "bitrate");
    assert!(!shows(&h, "Decks"), "Decks has no bitrate");
    assert_eq!(count(&h, "Clock"), clocks - 1, "Clock has no bitrate");
    assert!(shows(&h, "Host"), "the Icecast page shows its form");
    search(&mut h, " nothing-matches-this");
    assert!(
        h.query_all_by_label_contains("No settings match")
            .next()
            .is_some(),
        "an empty search result says so"
    );
}

#[test]
fn restore_page_defaults_sets_the_clock_page_back() {
    let mut h = preferences();
    click(&mut h, "Clock");
    click(&mut h, "24-hour time");
    assert!(!checked(&h, "24-hour time"), "the click turns it off");
    click(&mut h, "Restore page defaults");
    assert!(checked(&h, "24-hour time"), "Restore turns it back on");
}

#[test]
fn restore_page_defaults_is_disabled_where_a_page_has_none() {
    let mut h = preferences();
    let restore = |h: &Harness<'_, RadioApp>| {
        h.get_by_label("Restore page defaults")
            .accesskit_node()
            .is_disabled()
    };
    assert!(!restore(&h), "Decks has defaults");
    click(&mut h, "Cartwall");
    assert!(restore(&h), "the cart pages have no defaults");
}

#[test]
fn a_section_folds_its_rows_away_and_back() {
    let mut h = preferences();
    assert!(shows(&h, "Deck A fade-out"));
    click(&mut h, "Deck A");
    assert!(
        !shows(&h, "Deck A fade-out"),
        "the folded section hides its rows"
    );
    assert!(shows(&h, "Deck B fade-out"), "the other sections stay open");
    click(&mut h, "Deck A");
    assert!(
        shows(&h, "Deck A fade-out"),
        "a second click opens it again"
    );
}

#[test]
fn a_search_finds_a_page_by_the_label_of_a_row() {
    let mut h = preferences();
    search(&mut h, "threshold");
    assert!(
        h.query_all_by_label("Threshold").next().is_some(),
        "Processing, which has a Threshold row, is shown"
    );
    assert!(!shows(&h, "Decks"), "the other pages are not");
}

#[test]
fn command_f_puts_the_keys_into_the_search_field() {
    let mut h = preferences();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    h.run_steps(FRAMES);
    assert!(
        h.get_by_label("Search preferences").is_focused(),
        "the search field has the focus"
    );
}

/// A ⚙ opens its own page even when the last visit ended inside a search.
#[test]
fn a_settings_button_after_a_search_opens_its_own_page() {
    let mut h = preferences();
    search(&mut h, "bitrate");
    click(&mut h, "Done");
    click(&mut h, "Clock");
    click(&mut h, "Clock preferences");
    assert!(
        shows(&h, "Show the seconds ring"),
        "the Clock page shows, not the search's Icecast"
    );
}

#[test]
fn restore_page_defaults_turns_the_compressor_back_off() {
    let mut h = preferences();
    click(&mut h, "Processing");
    let compressor = |h: &Harness<'_, RadioApp>| {
        h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Compressor")
            .accesskit_node()
            .toggled()
            == Some(Toggled::True)
    };
    assert!(!compressor(&h), "the compressor starts off");
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Compressor")
        .click();
    h.run_steps(FRAMES);
    assert!(compressor(&h), "the click turns it on");
    click(&mut h, "Restore page defaults");
    assert!(!compressor(&h), "Restore turns it off again");
}

#[test]
fn the_webrtc_page_keeps_its_port_and_restores_it() {
    let mut h = preferences();
    click(&mut h, "WebRTC");
    assert!(shows(&h, "Port"), "the page has a Port row");
    let restore = h.get_by_label("Restore page defaults");
    assert!(
        !restore.accesskit_node().is_disabled(),
        "WebRTC has defaults"
    );
}

/// True when a control called `label` has the keyboard focus.
fn focused(h: &Harness<'_, RadioApp>, label: &str) -> bool {
    h.query_all_by_label(label)
        .any(|n| n.accesskit_node().is_focused())
}

#[test]
fn opening_branding_focuses_the_name_but_a_search_that_selects_it_does_not() {
    let mut h = preferences();
    click(&mut h, "Branding");
    assert!(
        focused(&h, "Station name"),
        "a click on the rail focuses it"
    );
    let mut h = preferences();
    search(&mut h, "logo");
    assert!(shows(&h, "Station name"), "the search selected Branding");
    assert!(!focused(&h, "Station name"), "the name did not take focus");
    assert!(
        focused(&h, "Search preferences"),
        "the search keeps the focus while it is typed in"
    );
}

#[test]
fn the_branding_page_says_that_no_logo_means_the_app_icon() {
    let mut h = preferences();
    click(&mut h, "Branding");
    assert!(shows(&h, "Station name") && shows(&h, "Logo"));
    assert!(shows(&h, "No logo: listeners see the app icon."));
    assert!(
        h.get_by_label("Clear").accesskit_node().is_disabled(),
        "there is no logo to clear"
    );
}

/// What the Station name field holds.
fn name_field(h: &Harness<'_, RadioApp>) -> String {
    h.query_all_by_label("Station name")
        .filter_map(|n| n.accesskit_node().value())
        .next_back()
        .unwrap_or_default()
}

#[test]
fn a_name_typed_in_branding_is_saved_when_preferences_closes_with_the_field_still_focused() {
    let mut h = preferences();
    click(&mut h, "Branding");
    h.get_by_label("Station name").type_text("Radio Lipa");
    h.run_steps(FRAMES);
    assert!(focused(&h, "Station name"), "the person is still typing");
    h.state_mut().set_window(ToolWindow::Preferences, false);
    h.run_steps(FRAMES);
    assert!(!shows(&h, "Station name"), "Preferences closed");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    h.run_steps(FRAMES);
    click(&mut h, "Branding");
    assert_eq!(name_field(&h), "Radio Lipa", "the name was saved");
}

/// A station that is not the preview, paired with a server or not. It keeps each edit it
/// is sent, so a test can see what a button asked for.
struct Station {
    mock: MockSource,
    paired: bool,
    edits: std::sync::Arc<std::sync::Mutex<Vec<Edit>>>,
}

impl ViewSource for Station {
    fn snapshot(&mut self, now: jiff::Timestamp) -> StationView {
        let mut v = self.mock.snapshot(now);
        v.preview = false;
        v.pairing.paired = self.paired;
        v
    }
    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        if let Ok(mut edits) = self.edits.lock() {
            edits.push(edit.clone());
        }
        self.mock.edit(edit)
    }
    fn stream_title(&self, row: &radio_ui_egui::view::Row) -> String {
        self.mock.stream_title(row)
    }
}

/// Preferences of a [`Station`], and the edits it was sent.
fn station_preferences(
    paired: bool,
) -> (
    Harness<'static, RadioApp>,
    std::sync::Arc<std::sync::Mutex<Vec<Edit>>>,
) {
    let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
    let now: jiff::Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let edits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = std::sync::Arc::clone(&edits);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            let station = Station {
                mock: MockSource::new(tz),
                paired,
                edits: kept,
            };
            RadioApp::new(Box::new(station), Box::new(move || now))
        });
    h.run_steps(FRAMES);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    h.run_steps(FRAMES);
    (h, edits)
}

/// The Icecast page has its own switch, as the WebRTC page does: it starts on, and turning it
/// off stores the setting and says Icecast is off, in words.
#[test]
fn the_icecast_page_switches_icecast_off_and_says_so() {
    let (mut h, edits) = station_preferences(false);
    search(&mut h, "Icecast");
    // The row names the switch, as on the WebRTC page.
    assert!(checked(&h, "Streaming"), "Icecast starts on");
    click(&mut h, "Streaming");
    assert!(!checked(&h, "Streaming"), "the click turns it off");
    let sent = edits.lock().expect("edits").clone();
    assert!(
        sent.iter()
            .any(|e| matches!(e, Edit::SetPartSetting { part, key, value }
            if part == "icecast" && key == "enabled" && *value == serde_json::json!(false))),
        "the switch stores icecast.enabled: {sent:?}"
    );
    assert!(
        h.query_all_by_label_contains("Icecast off")
            .next()
            .is_some(),
        "the page says Icecast is off"
    );
    assert!(
        h.query_all_by_label_contains("Icecast off air")
            .next()
            .is_none(),
        "on air, a switched-off Icecast is not off air"
    );
}

/// Restore page defaults: whether it can, and why not in words.
fn restore_reason(h: &Harness<'_, RadioApp>) -> Option<String> {
    let node = h.get_by_label("Restore page defaults");
    let node = node.accesskit_node();
    node.is_disabled()
        .then(|| node.description().unwrap_or_default().to_owned())
}

fn sent(edits: &std::sync::Mutex<Vec<Edit>>) -> Vec<Edit> {
    edits.lock().expect("edits").clone()
}

/// On the Server page, Restore page defaults is Disconnect: it unpairs, and only a paired
/// station can.
#[test]
fn restore_on_the_server_page_unpairs_and_only_while_paired() {
    let (mut h, edits) = station_preferences(false);
    click(&mut h, "Server");
    assert_eq!(
        restore_reason(&h).as_deref(),
        Some("The station is not paired with a server.")
    );
    click(&mut h, "Restore page defaults");
    assert!(!sent(&edits).contains(&Edit::Unpair), "nothing to unpair");

    let (mut h, edits) = station_preferences(true);
    click(&mut h, "Server");
    assert_eq!(restore_reason(&h), None, "a paired station can unpair");
    click(&mut h, "Restore page defaults");
    assert_eq!(
        sent(&edits).iter().filter(|e| **e == Edit::Unpair).count(),
        1,
        "Restore unpairs once"
    );
}

/// On the Icecast page, Restore page defaults sets Icecast back, switched on again; a paired
/// station keeps the settings its pairing writes, and says so.
#[test]
fn restore_on_the_icecast_page_sets_it_back_unless_paired() {
    let (mut h, edits) = station_preferences(false);
    search(&mut h, "Icecast");
    click(&mut h, "Streaming");
    assert!(!checked(&h, "Streaming"));
    assert_eq!(restore_reason(&h), None, "Icecast has defaults");
    click(&mut h, "Restore page defaults");
    assert!(sent(&edits).contains(&Edit::RestoreIcecast));
    assert!(checked(&h, "Streaming"), "Restore switches Icecast back on");

    let (mut h, edits) = station_preferences(true);
    search(&mut h, "Icecast");
    assert_eq!(
        restore_reason(&h).as_deref(),
        Some(radio_ui_egui::view::ICECAST_PAIRED)
    );
    click(&mut h, "Restore page defaults");
    assert!(!sent(&edits).contains(&Edit::RestoreIcecast));
}

/// Restore page defaults on the WebRTC page turns streaming to listeners back off.
#[test]
fn restore_on_the_webrtc_page_turns_streaming_off() {
    let (mut h, edits) = station_preferences(false);
    click(&mut h, "WebRTC");
    click(&mut h, "Streaming");
    assert!(checked(&h, "Streaming"), "streaming is on");
    click(&mut h, "Restore page defaults");
    assert!(sent(&edits).contains(&Edit::SetStreaming(false)));
    assert!(!checked(&h, "Streaming"), "Restore turns streaming off");
}
