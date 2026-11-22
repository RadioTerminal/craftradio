//! Controls keep one size in every state: hovering a tab does not move its neighbours.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 4;

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

fn rect(h: &Harness<'_, RadioApp>, label: &str) -> egui::Rect {
    h.get_by_label(label).rect()
}

#[test]
fn hovering_a_playlist_tab_keeps_its_size_and_its_neighbours_in_place() {
    // "Upcoming" is selected at start; "    Jingles" is a tab at rest.
    for hovered in ["    Jingles", "Upcoming"] {
        let mut h = harness();
        let before = [rect(&h, "Upcoming"), rect(&h, "    Jingles")];
        h.get_by_label(hovered).hover();
        h.run_steps(FRAMES);
        let after = [rect(&h, "Upcoming"), rect(&h, "    Jingles")];
        assert_eq!(before, after, "hovering {hovered:?} changed the tab bar");
    }
}

/// A playlist tab has one scroll, its items list: wheeling over the list never moves the
/// header of the tab.
#[test]
fn a_playlist_tab_scrolls_only_its_items_list() {
    let mut h = harness();
    h.get_by_label("    Jingles").click();
    h.run_steps(FRAMES);
    let header = |h: &Harness<'_, RadioApp>| h.get_by_label_contains("3 items").rect().top();
    let before = header(&h);
    let over = h.get_by_label("Station ID 1").rect().center();
    h.hover_at(over);
    for _ in 0..60 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -200.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        });
        h.run_steps(2);
    }
    h.run_steps(FRAMES * 4);
    assert_eq!(header(&h), before, "the header of the tab stays put");
}
