//! Loudness normalisation in the UI: the deck caption, the Loudness section of the
//! Processing tab, and the gain trim in the metadata editor.

use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 4;
const ROW: &str = "Juniper Vale – Satellites of Ulvenik";

fn harness() -> Harness<'static, RadioApp> {
    let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
    let now: jiff::Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(move || now))
        });
    harness.run_steps(FRAMES);
    harness
}

fn shows(harness: &Harness<'_, RadioApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

fn click(harness: &mut Harness<'_, RadioApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(FRAMES);
}

/// Each deck card says the gain it applies and where it comes from, in words.
#[test]
fn deck_cards_show_the_item_gain_and_its_source() {
    let harness = harness();
    assert!(shows(&harness, "RG −3.2 dB"), "a measured cut");
    assert!(shows(&harness, "tag −2.1 dB"), "a gain from a tag");
}

/// Turning normalisation off on the Processing tab sticks, and the decks then say "off"
/// instead of a gain.
#[test]
fn the_processing_tab_turns_loudness_off() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    click(&mut harness, "Mixer preferences");
    click(&mut harness, "Processing");
    // The switch takes its name from its row label.
    let switch = By::new()
        .label("Loudness normalisation")
        .role(egui::accesskit::Role::CheckBox);
    let on = |h: &Harness<'_, RadioApp>| {
        h.get(switch.clone()).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
    };
    assert!(on(&harness), "normalisation starts on");
    harness.get(switch.clone()).click();
    harness.run_steps(FRAMES);
    click(&mut harness, "Done");
    click(&mut harness, "Mixer preferences");
    click(&mut harness, "Processing");
    assert!(!on(&harness), "the source kept it off");
    assert!(
        !shows(&harness, "RG −3.2 dB"),
        "no gain is shown as applied"
    );
    assert!(shows(&harness, "RG off"));
}

/// The metadata editor of a file has a gain trim field.
#[test]
fn the_metadata_editor_has_a_gain_trim() {
    let mut harness = harness();
    let row = harness
        .query_all_by_label_contains(ROW)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the row");
    row.click_secondary();
    harness.run_steps(FRAMES);
    harness
        .query_all_by_label_contains("Edit metadata")
        .next()
        .expect("menu row")
        .click();
    harness.run_steps(FRAMES);
    assert!(shows(&harness, "Gain trim"));
}
