//! The key map: cart keys, arrow keys, and no shortcut while a text field types.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 4;
/// What a fired cart reports while the preview has no audio.
const FIRED: &str = "· preview — no audio";
/// A main row that no deck holds, with an ad break after it.
const ROW: &str = "Marlo Fenwick – Echoes of Miravel";
const NEXT_ROW: &str = "Ad 1 · Kesten Bakery";

fn harness() -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(move || now))
        });
    harness.run_steps(FRAMES);
    harness
}

fn shows(h: &Harness<'_, RadioApp>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

#[test]
fn f1_fires_the_first_cart() {
    let mut h = harness();
    assert!(!shows(&h, FIRED), "no cart has fired yet");
    h.key_press(egui::Key::F1);
    h.run_steps(FRAMES);
    assert!(shows(&h, FIRED), "F1 fires cart 1");
}

#[test]
fn f1_does_nothing_while_a_text_field_has_focus() {
    let mut h = harness();
    h.get_by_label("Library").click();
    h.run_steps(FRAMES);
    h.get_by_label("Search").focus();
    h.run_steps(FRAMES);
    h.key_press(egui::Key::F1);
    h.run_steps(FRAMES);
    assert!(!shows(&h, FIRED), "the search field keeps the key");
}

#[test]
fn the_down_arrow_moves_the_selection_to_the_next_row() {
    let mut h = harness();
    let row = h.query_all_by_label(ROW).count();
    let next = h.query_all_by_label_contains(NEXT_ROW).count();
    h.query_all_by_label(ROW)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the row is on screen")
        .click();
    h.run_steps(FRAMES);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(FRAMES);
    // Delete acts on the selection, so it shows where the arrow moved it.
    h.key_press(egui::Key::Delete);
    h.run_steps(FRAMES);
    assert_eq!(
        h.query_all_by_label(ROW).count(),
        row,
        "the clicked row stays"
    );
    assert!(
        h.query_all_by_label_contains(NEXT_ROW).count() < next,
        "the row below it is removed"
    );
}

fn click_row(h: &mut Harness<'_, RadioApp>, label: &str, modifiers: egui::Modifiers) {
    h.query_all_by_label(label)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the row is on screen")
        .click_modifiers(modifiers);
    h.run_steps(FRAMES);
}

#[test]
fn command_a_selects_every_row_but_not_inside_a_text_field() {
    let mut h = harness();
    click_row(&mut h, ROW, egui::Modifiers::NONE);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(FRAMES);
    assert!(
        shows(&h, " selected ·"),
        "the list says how many rows are selected"
    );

    let mut h = harness();
    h.get_by_label("Library").click();
    h.run_steps(FRAMES);
    h.get_by_label("Search").focus();
    h.run_steps(FRAMES);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(FRAMES);
    assert!(!shows(&h, " selected ·"), "the text field keeps Command+A");
}

#[test]
fn backspace_removes_the_selected_rows_and_undo_brings_them_back() {
    let mut h = harness();
    let first = "Marlo Fenwick – Echoes of Miravel";
    let second = "Silver Tram – Tides of Quenby";
    let before = (
        h.query_all_by_label(first).count(),
        h.query_all_by_label(second).count(),
    );
    click_row(&mut h, first, egui::Modifiers::NONE);
    click_row(&mut h, second, egui::Modifiers::COMMAND);
    h.key_press(egui::Key::Backspace);
    h.run_steps(FRAMES);
    assert!(
        h.query_all_by_label(first).count() < before.0,
        "the first row is gone"
    );
    assert!(
        h.query_all_by_label(second).count() < before.1,
        "the second row is gone"
    );
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(FRAMES);
    assert_eq!(
        (
            h.query_all_by_label(first).count(),
            h.query_all_by_label(second).count()
        ),
        before,
        "undo brings both back"
    );
}
