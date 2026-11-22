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

/// Scrolling a playlist tab ends: its items list does not grow while the pane scrolls.
#[test]
fn a_playlist_tab_scrolls_a_bounded_distance() {
    let mut h = harness();
    h.get_by_label("    Jingles").click();
    h.run_steps(FRAMES);
    let caption = |h: &Harness<'_, RadioApp>| h.get_by_label("ITEMS (3)").rect().top();
    let before = caption(&h);
    let over = h.get_by_label("ITEMS (3)").rect().center();
    h.hover_at(over);
    for _ in 0..60 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -200.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(2);
    }
    h.run_steps(FRAMES * 4);
    let moved = before - caption(&h);
    // 60 wheel steps of 200 points: unbounded content would scroll thousands of points.
    // The pane is a little taller than its window, so it scrolls only that little.
    assert!(moved < 200.0, "the pane scrolled {moved} points");
}
