//! The tabs of the scheduled pane: one row, and a menu for the playlists that do not fit.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::RadioApp;
use radio_ui_egui::mock::StressSource;

const FRAMES: usize = 6;

/// The narrowest window, with many playlists and long names.
fn harness() -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("valid timestamp");
    let mut h = Harness::builder()
        .with_size(egui::vec2(1024.0, 860.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(StressSource::new(tz)), Box::new(move || now))
        });
    h.run_steps(FRAMES);
    h
}

fn names(h: &Harness<'_, RadioApp>, role: Role) -> Vec<String> {
    h.query_all(By::new().role(role))
        .filter_map(|n| n.accesskit_node().label())
        .collect()
}

#[test]
fn the_tabs_stay_on_one_row_and_the_rest_are_in_a_menu() {
    let mut h = harness();
    let rows: Vec<f32> = h
        .query_all(By::new().role(Role::Tab))
        .map(|n| n.rect().top())
        .collect();
    assert!(
        rows.len() >= 2,
        "Upcoming and at least one playlist are tabs"
    );
    assert!(
        rows.iter().all(|top| (top - rows[0]).abs() < 1.0),
        "every tab is on the first row: {rows:?}"
    );
    let tabs = names(&h, Role::Tab);
    let before = names(&h, Role::Button);
    h.get_by_label("More playlists").click();
    h.run_steps(FRAMES);
    let hidden = names(&h, Role::Button)
        .into_iter()
        .find(|n| !before.contains(n) && !tabs.contains(n))
        .expect("the menu lists a playlist that has no tab");
    h.get(By::new().label(&hidden).role(Role::Button)).click();
    h.run_steps(FRAMES);
    let chosen = h.get(By::new().label(&hidden).role(Role::Tab));
    assert_eq!(
        chosen.accesskit_node().is_selected(),
        Some(true),
        "{hidden} from the menu is now the selected tab"
    );
}
