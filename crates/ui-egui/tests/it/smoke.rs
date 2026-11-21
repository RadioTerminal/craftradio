use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{MockSource, RadioApp};

fn fixed_clock() -> Timestamp {
    "2026-11-21T13:58:00Z".parse().expect("valid timestamp")
}

#[test]
fn the_window_shows_upcoming_and_the_next_fixed_news() {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(fixed_clock))
        });
    harness.run_steps(4);

    assert!(
        !harness
            .query_all_by_label_contains("Upcoming")
            .collect::<Vec<_>>()
            .is_empty(),
        "the scheduled pane has an Upcoming tab"
    );
    // 13:58 UTC is 14:58 in Ljubljana, so the next full hour is 15:00:00 local.
    assert!(
        !harness
            .query_all_by_label_contains("15:00:00")
            .collect::<Vec<_>>()
            .is_empty(),
        "the next full hour is on screen"
    );
}
