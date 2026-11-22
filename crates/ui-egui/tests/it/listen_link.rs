use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
fn the_status_line_copies_the_listen_link_only_while_it_works() {
    let without = harness(None);
    assert!(
        without
            .query_all_by_label_contains("Copy listen link")
            .next()
            .is_none(),
        "a link that no tracker carries must not show"
    );

    let mut with = harness(Some(LINK));
    with.get_by_label_contains("Copy listen link").click();
    with.step();
    let copied = with
        .output()
        .platform_output
        .commands
        .iter()
        .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == LINK));
    assert!(copied, "the click copies the link");
}
