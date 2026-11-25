//! The status dots of the WebRTC and Server pages on the Preferences rail: each state has its
//! own look and says what it is in words.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::view::{
    Health, MeterFrame, OutputStatus, PageStatus, PairState, PairingView, Row, StationView, Status,
    ViewSource,
};
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 6;
const ADDRESS: &str = "https://radio.example.org";

fn mock() -> MockSource {
    MockSource::new(TimeZone::get("Europe/Ljubljana").expect("tz"))
}

fn now() -> Timestamp {
    "2026-11-21T13:58:00Z".parse().expect("ts")
}

/// The mock's outputs with WebRTC in `status` and its switch as `streaming` says.
fn outputs(status: Status, streaming: bool) -> OutputStatus {
    let mut out = mock().snapshot(now()).output;
    out.webrtc = status;
    out.streaming = Some(streaming);
    out.webrtc_listeners = None;
    out.webrtc_reported = None;
    out
}

fn webrtc(status: Status) -> PageStatus {
    outputs(status, true).webrtc_page_status()
}

#[test]
fn the_webrtc_dot_follows_the_stream() {
    let mut live = outputs(Status::Connected, true);
    live.webrtc_listeners = Some(2);
    let live = live.webrtc_page_status();
    assert_eq!(live.health, Health::Good);
    assert!(live.words.contains("2 listeners"), "{}", live.words);

    let starting = webrtc(Status::Reconnecting { attempt: 0 });
    assert_eq!(starting.health, Health::Busy);
    assert!(
        starting.words.contains("reconnecting"),
        "{}",
        starting.words
    );

    let down = webrtc(Status::Failed("port in use".into()));
    assert_eq!(down.health, Health::Fault);
    assert!(down.words.contains("port in use"), "{}", down.words);

    let off_air = webrtc(Status::OffAir);
    assert_eq!(off_air.health, Health::Idle);
    assert!(off_air.words.contains("off air"), "{}", off_air.words);
}

#[test]
fn a_webrtc_switched_off_is_off_not_a_fault() {
    for status in [Status::Off, Status::NotConnected] {
        let off = outputs(status, false).webrtc_page_status();
        assert_eq!(off.health, Health::Off);
        assert!(off.words.ends_with("off"), "{}", off.words);
    }
}

fn pairing(state: PairState, paired: bool) -> PairingView {
    PairingView {
        address: ADDRESS.to_owned(),
        paired,
        state,
        ..PairingView::default()
    }
}

#[test]
fn the_server_dot_follows_the_pairing() {
    assert_eq!(
        pairing(PairState::NotPaired, false).page_status(),
        None,
        "no dot before a pairing is tried"
    );
    for (state, health) in [
        (PairState::Pairing, Health::Busy),
        (PairState::Paired, Health::Good),
        (PairState::Unreachable("timed out".into()), Health::Fault),
        (PairState::KeyRefused, Health::Fault),
        (PairState::Refused("newer document".into()), Health::Fault),
    ] {
        let view = pairing(state, false);
        let status = view.page_status().expect("a dot");
        assert_eq!(status.health, health, "{:?}", view.state);
        assert_eq!(status.words, view.label(), "the words of the page");
    }
}

/// The mock as a station whose WebRTC and pairing are what the test says.
struct Station {
    inner: MockSource,
    webrtc: Status,
    pairing: PairingView,
}

impl ViewSource for Station {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let mut view = self.inner.snapshot(now);
        view.preview = false;
        view.output.webrtc = self.webrtc.clone();
        view.output.streaming = Some(true);
        view.pairing = self.pairing.clone();
        view
    }
    fn edit(&mut self, edit: radio_ui_egui::view::Edit) -> Result<usize, String> {
        self.inner.edit(edit)
    }
    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
    fn meters(&self) -> Option<MeterFrame> {
        self.inner.meters()
    }
}

#[test]
fn the_rail_names_the_webrtc_and_server_pages_with_their_state() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            let source = Station {
                inner: mock(),
                webrtc: Status::Failed("port in use".into()),
                pairing: pairing(PairState::Paired, true),
            };
            RadioApp::new(Box::new(source), Box::new(now))
        });
    h.run_steps(FRAMES);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    h.run_steps(FRAMES);
    assert!(
        h.query_by_label("WebRTC — WebRTC failed: port in use")
            .is_some(),
        "the WebRTC tab says its fault"
    );
    assert!(
        h.query_by_label(&format!("Server — Paired with {ADDRESS}"))
            .is_some(),
        "the Server tab says it is paired"
    );
}
