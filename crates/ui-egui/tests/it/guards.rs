//! On-air safety: the second press on the deck on air, protected rows and Undo.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 4;
const ON_AIR: &str = "Halvard Pine – Harbours of Kelmora";
/// A main row far enough from the row on air that no deck holds it.
const FAR_ROW: &str = "Marlo Fenwick – Echoes of Miravel";

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

fn count(h: &Harness<'_, RadioApp>, label: &str) -> usize {
    h.query_all_by_label(label).count()
}

fn right_click_row(h: &mut Harness<'_, RadioApp>, label: &str) {
    let node = h
        .query_all_by_label(label)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the row is on screen");
    node.click_secondary();
    h.run_steps(FRAMES);
}

#[test]
fn play_next_cuts_the_item_on_air_only_on_the_second_press() {
    let mut h = harness();
    let before = count(&h, ON_AIR);
    h.get_by_label("⏭ Play next").click();
    h.run_steps(FRAMES);
    assert_eq!(
        count(&h, ON_AIR),
        before,
        "the first press only arms Play next"
    );
    assert!(
        h.query_all_by_label_contains("press again")
            .next()
            .is_some(),
        "the armed button asks for a second press"
    );
    h.get_by_label("⏭ Play next — press again").click();
    h.run_steps(FRAMES);
    assert!(
        count(&h, ON_AIR) < before,
        "the second press plays the next deck"
    );
}

#[test]
fn play_on_a_waiting_deck_cuts_the_item_on_air_only_on_the_second_press() {
    let mut h = harness();
    let before = count(&h, ON_AIR);
    h.get_by_label("▶ Play deck C").click();
    h.run_steps(FRAMES);
    assert_eq!(count(&h, ON_AIR), before, "the first press only arms Play");
    h.get_by_label("▶ Play deck C — press again").click();
    h.run_steps(FRAMES);
    assert!(count(&h, ON_AIR) < before, "the second press plays deck C");
}

#[test]
fn the_row_on_air_cannot_be_removed() {
    let mut h = harness();
    right_click_row(&mut h, ON_AIR);
    let remove = h.get_by_label_contains("Remove (");
    assert!(
        remove.accesskit_node().is_disabled(),
        "Remove is disabled for the row on air"
    );
}

#[test]
fn undo_brings_back_a_removed_row() {
    let mut h = harness();
    let before = count(&h, FAR_ROW);
    right_click_row(&mut h, FAR_ROW);
    h.get_by_label("Remove").click();
    h.run_steps(FRAMES);
    assert!(count(&h, FAR_ROW) < before, "the row is gone");
    h.get_by_label_contains("Undo (").click();
    h.run_steps(FRAMES);
    assert_eq!(count(&h, FAR_ROW), before, "Undo puts the row back");
}
