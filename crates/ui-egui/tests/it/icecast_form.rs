//! The Icecast form of the mixer settings takes typed text: a field keeps the focus while
//! a person types into it.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 4;

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

fn click(harness: &mut Harness<'_, RadioApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(FRAMES);
}

#[test]
fn typing_into_the_icecast_host_field_keeps_the_text() {
    let mut h = harness();
    click(&mut h, "Mixer");
    click(&mut h, "Icecast preferences");
    let host = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("the Host field");
    host.click();
    h.run_steps(FRAMES);
    for c in "radio.example".chars() {
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .expect("field")
            .type_text(&c.to_string());
        h.run_steps(2);
    }
    h.run_steps(FRAMES);
    let host = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.accesskit_node().label().as_deref() == Some("Host"))
        .and_then(|n| n.accesskit_node().value())
        .unwrap_or_default();
    assert!(
        host.contains("radio.example"),
        "the Host field holds what was typed, got {host:?}"
    );
}
