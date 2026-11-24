//! The Server page of Preferences: the address and the key go to the `Pair` edit, the key
//! never shows and is forgotten after Connect, and each pairing state says what it is.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::theme::Tokens;
use radio_ui_egui::view::{Edit, MeterFrame, PairState, PairingView, Row, StationView, ViewSource};
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 6;
const ADDRESS: &str = "https://radio.example.org";
const KEY: &str = "invented-station-key-42";

/// The mock as a station: its pairing is what the test says, and the edits it gets are
/// kept for the test to read.
struct Station {
    inner: MockSource,
    pairing: PairingView,
    preview: bool,
    edits: Arc<Mutex<Vec<Edit>>>,
}

impl ViewSource for Station {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let mut view = self.inner.snapshot(now);
        view.pairing = self.pairing.clone();
        view.preview = self.preview;
        view
    }
    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        self.edits.lock().expect("edits").push(edit.clone());
        match edit {
            Edit::Pair { .. } | Edit::Unpair => Ok(1),
            other => self.inner.edit(other),
        }
    }
    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
    fn meters(&self) -> Option<MeterFrame> {
        self.inner.meters()
    }
}

struct Page {
    h: Harness<'static, RadioApp>,
    edits: Arc<Mutex<Vec<Edit>>>,
}

fn now() -> Timestamp {
    "2026-11-21T13:58:00Z".parse().expect("ts")
}

/// The app with Preferences open on the Server page.
fn page(pairing: PairingView) -> Page {
    let edits = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&edits);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            let source = Station {
                inner: MockSource::new(TimeZone::get("Europe/Ljubljana").expect("tz")),
                pairing,
                preview: false,
                edits: kept,
            };
            RadioApp::new(Box::new(source), Box::new(now))
        });
    h.run_steps(FRAMES);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    h.run_steps(FRAMES);
    // The rail draws the page's name first; the page heading, once open, after it.
    h.query_all_by_label("Server")
        .next()
        .expect("the Server page in the rail")
        .click();
    h.run_steps(FRAMES);
    Page { h, edits }
}

impl Page {
    fn type_into(&mut self, label: &str, text: &str) {
        self.h.get_by_label(label).click();
        self.h.run_steps(2);
        self.h.get_by_label(label).type_text(text);
        self.h.run_steps(FRAMES);
    }

    fn click(&mut self, label: &str) {
        self.h.get_by_label(label).click();
        self.h.run_steps(FRAMES);
    }

    fn value(&self, label: &str) -> String {
        self.h
            .get_by_label(label)
            .accesskit_node()
            .value()
            .unwrap_or_default()
    }

    fn pairs(&self) -> Vec<(String, String)> {
        self.edits
            .lock()
            .expect("edits")
            .iter()
            .filter_map(|e| match e {
                Edit::Pair { address, key } => Some((address.clone(), key.expose().to_owned())),
                _ => None,
            })
            .collect()
    }

    fn unpairs(&self) -> usize {
        let edits = self.edits.lock().expect("edits");
        edits.iter().filter(|e| matches!(e, Edit::Unpair)).count()
    }
}

#[test]
fn connect_sends_pair_with_the_typed_address_and_key() {
    let mut p = page(PairingView::default());
    p.type_into("Server address", ADDRESS);
    p.type_into("Station key", KEY);
    assert!(p.pairs().is_empty(), "typing alone sends nothing");
    p.click("Connect");
    assert_eq!(p.pairs(), vec![(ADDRESS.to_owned(), KEY.to_owned())]);
}

#[test]
fn the_key_field_is_masked_and_cleared_after_connect() {
    let mut p = page(PairingView::default());
    p.type_into("Server address", ADDRESS);
    p.type_into("Station key", KEY);
    let typed = p.value("Station key");
    assert!(!typed.contains(KEY), "the key never shows, got {typed:?}");
    assert!(
        !p.h.query_all_by_label_contains(KEY).any(|_| true),
        "no node names the key"
    );
    p.click("Connect");
    assert!(
        p.value("Station key").is_empty(),
        "the key field is empty after Connect"
    );
    // Undo in the field does not bring the key back.
    p.h.get_by_label("Station key").click();
    p.h.run_steps(2);
    p.h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    p.h.run_steps(FRAMES);
    assert!(
        p.value("Station key").is_empty(),
        "undo leaves the key field empty"
    );
    assert_eq!(p.pairs().len(), 1, "one Connect, one Pair");
}

#[test]
fn each_state_shows_in_words_and_only_paired_looks_ok() {
    let t = Tokens::DARK;
    let states = [
        PairState::NotPaired,
        PairState::Pairing,
        PairState::Paired,
        PairState::Unreachable("connection refused".to_owned()),
        PairState::KeyRefused,
        PairState::Refused("a newer pairing document".to_owned()),
    ];
    for state in states {
        for paired in [false, true] {
            let pairing = PairingView {
                address: ADDRESS.to_owned(),
                paired,
                state: state.clone(),
            };
            let words = pairing.label();
            let p = page(pairing.clone());
            assert!(
                p.h.query_all_by_label_contains(&words).any(|_| true),
                "the page says {words:?}"
            );
            let mut view = MockSource::new(TimeZone::get("UTC").expect("tz")).snapshot(now());
            view.pairing = pairing;
            view.preview = false;
            let ok = radio_ui_egui::state_ink(&view, &t) == t.ok;
            assert_eq!(
                ok,
                state == PairState::Paired,
                "{words:?} looks like success only when the server took the key"
            );
            view.preview = true;
            assert_ne!(
                radio_ui_egui::state_ink(&view, &t),
                t.ok,
                "the preview never looks paired"
            );
        }
    }
}

#[test]
fn disconnect_sends_unpair() {
    let mut p = page(PairingView {
        address: ADDRESS.to_owned(),
        paired: true,
        state: PairState::Paired,
    });
    p.click("Disconnect");
    assert_eq!(p.unpairs(), 1);
    assert!(p.pairs().is_empty(), "Disconnect does not pair");
}

#[test]
fn disconnect_is_off_while_not_paired() {
    let mut p = page(PairingView::default());
    p.click("Disconnect");
    assert_eq!(p.unpairs(), 0, "nothing to unpair");
}

#[test]
fn the_page_is_reachable_by_keyboard_and_names_start_with_labels() {
    let mut p = page(PairingView::default());
    for name in ["Server address", "Station key", "Connect", "Disconnect"] {
        assert!(p.h.query_by_label(name).is_some(), "a control named {name}");
    }
    assert!(
        p.h.get_by_label("Server address").is_focused(),
        "the address field has the focus when the page opens"
    );
    p.h.get_by_label("Server address").type_text(ADDRESS);
    p.h.run_steps(FRAMES);
    p.h.key_press(egui::Key::Tab);
    p.h.run_steps(FRAMES);
    assert!(
        p.h.get_by_label("Station key").is_focused(),
        "Tab moves from the address to the key"
    );
    p.h.get_by_label("Station key").type_text(KEY);
    p.h.run_steps(FRAMES);
    p.h.key_press(egui::Key::Enter);
    p.h.run_steps(FRAMES);
    assert_eq!(
        p.pairs(),
        vec![(ADDRESS.to_owned(), KEY.to_owned())],
        "Enter in the key field connects"
    );
}

/// While the server is being asked, Connect is off: a second click would send the key once
/// more toward the server's lockout.
#[test]
fn connect_is_off_while_a_pairing_is_asked() {
    let mut p = page(PairingView {
        address: ADDRESS.to_owned(),
        paired: false,
        state: PairState::Pairing,
    });
    p.type_into("Server address", ADDRESS);
    p.type_into("Station key", KEY);
    p.click("Connect");
    assert!(p.pairs().is_empty(), "no second try while one is asked");
}
