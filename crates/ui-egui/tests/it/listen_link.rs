use egui::accesskit::Toggled;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::view::{Edit, MeterFrame, Row, StationView, ViewSource};
use radio_ui_egui::{MockSource, RadioApp};

const LINK: &str = "https://radioterminal.github.io/craftradio/#station-id";

/// The mock, with the listen link of a station whose tracker is connected or not.
struct Linked {
    inner: MockSource,
    link: Option<String>,
}

impl ViewSource for Linked {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let mut view = self.inner.snapshot(now);
        view.output.listen_link = self.link.clone();
        view
    }
    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        self.inner.edit(edit)
    }
    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
    fn meters(&self) -> Option<MeterFrame> {
        self.inner.meters()
    }
}

fn harness(link: Option<&str>) -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let link = link.map(str::to_owned);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            let source = Linked {
                inner: MockSource::new(tz),
                link,
            };
            RadioApp::new(Box::new(source), Box::new(move || now))
        });
    harness.run_steps(4);
    harness
}

#[test]
fn share_copies_the_listen_link_and_hides_while_there_is_nothing_to_share() {
    let without = harness(None);
    assert!(
        without.query_by_label("Share").is_none(),
        "nothing to share while streaming is off"
    );

    let mut with = harness(Some(LINK));
    with.get_by_label("Share").click();
    with.run_steps(2);
    with.get_by_label("Copy listen link").click();
    with.step();
    let copied = with
        .output()
        .platform_output
        .commands
        .iter()
        .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == LINK));
    assert!(copied, "the menu copies the link");
}

#[test]
fn the_on_air_chip_turns_streaming_on_and_the_status_says_off_until_then() {
    let mut h = harness(None);
    let texts: Vec<String> = h
        .root()
        .query_all(egui_kittest::kittest::By::new().predicate(|n| {
            n.value().is_some_and(|v| v.contains("WebRTC"))
                || n.label().is_some_and(|v| v.contains("WebRTC"))
        }))
        .filter_map(|n| n.accesskit_node().value().or(n.accesskit_node().label()))
        .collect();
    assert!(
        texts.iter().any(|t| t.ends_with(" off")),
        "the status says off while streaming is off: {texts:?}"
    );
    // The preview sends no audio, so its chip says PREVIEW; it still switches streaming.
    let chip = h.get_by_label("PREVIEW");
    assert_eq!(chip.accesskit_node().toggled(), Some(Toggled::False));
    chip.click();
    h.run_steps(4);
    let chip = h.get_by_label("PREVIEW");
    assert_eq!(chip.accesskit_node().toggled(), Some(Toggled::True));
}
