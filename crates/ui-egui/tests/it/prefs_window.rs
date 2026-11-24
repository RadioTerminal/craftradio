//! The Preferences window: its search narrows the rail, Restore page defaults resets a page,
//! and a section folds.

use egui::accesskit::Toggled;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use radio_ui_egui::{MockSource, RadioApp};

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
