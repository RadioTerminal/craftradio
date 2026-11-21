//! The Alarms window: the status-bar chip opens it, and an acknowledgement lowers the count.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 4;

fn fixed_clock() -> Timestamp {
    "2026-11-21T13:58:00Z".parse().expect("valid timestamp")
}

fn harness() -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(fixed_clock))
        });
    harness.run_steps(FRAMES);
    harness
}

/// The label of the status-bar chip and its count. Its label ends with "warning(s)".
fn chip(h: &Harness<'_, RadioApp>) -> (String, usize) {
    h.query_all(By::new().role(Role::Button))
        .filter_map(|n| n.accesskit_node().label())
        .find(|l| l.ends_with(" warnings") || l.ends_with(" warning"))
        .map(|l| {
            let n = l
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .expect("the chip shows a count");
            (l, n)
        })
        .expect("the status bar has the warnings chip")
}

#[test]
fn the_chip_opens_the_alarms_window_and_an_ack_lowers_its_count() {
    let mut h = harness();
    let (label, before) = chip(&h);
    assert!(before > 0, "the preview has new alarms");
    assert!(
        h.query_all_by_label_contains("CONCERNS").next().is_none(),
        "the Alarms window is closed at start"
    );
    h.get_by_label(&label).click();
    h.run_steps(FRAMES);
    assert!(
        h.query_all_by_label_contains("CONCERNS").next().is_some(),
        "the chip opens the Alarms window"
    );
    let acks = h.query_all_by_label("Acknowledge").count();

    assert!(acks > 0, "each new alarm has an Acknowledge button");
    h.query_all_by_label("Acknowledge")
        .next()
        .expect("an Acknowledge button")
        .click();
    h.run_steps(FRAMES);
    let (_, after) = chip(&h);
    assert_eq!(after + 1, before, "the chip counts one alarm less");
    assert_eq!(
        h.query_all_by_label("Acknowledge").count() + 1,
        acks,
        "the acknowledged row has no button any more"
    );
    assert!(
        h.query_all_by_label_contains("Acknowledged")
            .next()
            .is_some(),
        "the row stays, marked as acknowledged"
    );
}
