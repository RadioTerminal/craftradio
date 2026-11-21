use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::RadioApp;
use radio_ui_egui::mock::StressSource;

const FRAMES: usize = 6;
/// Two rects overlap when they share more than this many points of width and of height.
const SLACK: f32 = 1.0;
/// The top of the window: the deck strip, the header cluster and the pane headers. Rows of
/// a scroll area below it can sit under a clip, which accessibility rects do not show.
const HEAD: f32 = 240.0;

fn fixed_clock() -> Timestamp {
    "2026-11-21T13:58:00Z".parse().expect("valid timestamp")
}

/// At the narrowest window and with long text in many scripts, the labels and buttons of the
/// main window stay inside the window and never cover each other.
#[test]
fn labels_and_buttons_never_overlap_at_the_narrowest_width_with_long_text() {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let width = 1024.0;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(width, 860.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(StressSource::new(tz)), Box::new(fixed_clock))
        });
    harness.run_steps(FRAMES);
    let mut rects = Vec::new();
    for role in [Role::Label, Role::Button] {
        for node in harness.query_all(By::new().role(role)) {
            let r = node.rect();
            if r.width() <= 0.0 || r.height() <= 0.0 {
                continue;
            }
            let n = node.accesskit_node();
            let name = n
                .label()
                .or_else(|| n.value())
                .unwrap_or_default()
                .to_owned();
            rects.push((format!("{role:?} {name}"), r));
        }
    }
    assert!(rects.len() > 50, "the main window has many controls");
    for (name, r) in &rects {
        assert!(
            r.right() <= width + SLACK,
            "{name:?} runs out of the window: {r:?}"
        );
    }
    let head: Vec<_> = rects.iter().filter(|(_, r)| r.bottom() < HEAD).collect();
    assert!(head.len() > 10, "the head of the window has many controls");
    for (i, (a, ra)) in head.iter().enumerate() {
        for (b, rb) in head.iter().skip(i + 1) {
            let both = ra.intersect(*rb);
            let nested = ra.contains_rect(*rb) || rb.contains_rect(*ra);
            if both.width() > SLACK && both.height() > SLACK && !nested {
                panic!("{a:?} {ra:?} covers {b:?} {rb:?}");
            }
        }
    }
}
