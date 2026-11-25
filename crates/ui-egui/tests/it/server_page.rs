//! The Server page of Preferences: the address and the key go to the `Pair` edit, the key
//! never shows and is forgotten after Connect, and each pairing state says what it is.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::theme::Tokens;
use radio_ui_egui::view::{
    Edit, MeterFrame, OUTPUTS_READ_ONLY, OutputFormatView, OutputRow, OutputsState, PairState,
    PairingView, Row, StationView, ViewSource,
};
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
                ..PairingView::default()
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
        ..PairingView::default()
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
        ..PairingView::default()
    });
    p.type_into("Server address", ADDRESS);
    p.type_into("Station key", KEY);
    p.click("Connect");
    assert!(p.pairs().is_empty(), "no second try while one is asked");
}

fn row(mount: &str, format: OutputFormatView, kbps: u32) -> OutputRow {
    OutputRow {
        mount: mount.to_owned(),
        format,
        kbps,
    }
}

/// A paired station whose server keeps outputs the app may change.
fn paired_with(outputs: Vec<OutputRow>, state: OutputsState) -> PairingView {
    PairingView {
        address: ADDRESS.to_owned(),
        paired: true,
        state: PairState::Paired,
        outputs,
        outputs_editable: true,
        outputs_state: state,
    }
}

fn two_outputs() -> Vec<OutputRow> {
    vec![
        row("low.opus", OutputFormatView::Opus, 32),
        row("high.mp3", OutputFormatView::Mp3, 128),
    ]
}

impl Page {
    fn has(&self, label: &str) -> bool {
        self.h.query_by_label(label).is_some()
    }

    /// True when some text on the page contains `words`.
    fn says(&self, words: &str) -> bool {
        self.h.query_all_by_label_contains(words).any(|_| true)
    }

    fn enabled(&self, label: &str) -> bool {
        !self.h.get_by_label(label).accesskit_node().is_disabled()
    }

    fn saves(&self) -> Vec<Vec<OutputRow>> {
        let edits = self.edits.lock().expect("edits");
        edits
            .iter()
            .filter_map(|e| match e {
                Edit::SaveServerOutputs(rows) => Some(rows.clone()),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn outputs_show_only_when_paired() {
    let others = [
        (false, PairState::NotPaired),
        (false, PairState::Pairing),
        (true, PairState::Pairing),
        (true, PairState::KeyRefused),
        (
            true,
            PairState::Unreachable("connection refused".to_owned()),
        ),
        (
            false,
            PairState::Refused("a newer pairing document".to_owned()),
        ),
    ];
    for (paired, state) in others {
        let mut view = paired_with(two_outputs(), OutputsState::Idle);
        view.paired = paired;
        view.state = state.clone();
        let p = page(view);
        assert!(
            !p.has("Add output") && !p.has("Save") && !p.has("Mount of output 1"),
            "no outputs while {state:?} (paired: {paired})"
        );
    }
    let p = page(paired_with(two_outputs(), OutputsState::Idle));
    for name in [
        "Add output",
        "Save",
        "Mount of output 1",
        "Mount of output 2",
    ] {
        assert!(p.has(name), "the paired page has {name}");
    }
    assert_eq!(p.value("Mount of output 1"), "low.opus");
}

#[test]
fn add_edit_remove_then_save_sends_the_rows() {
    let mut p = page(paired_with(two_outputs(), OutputsState::Idle));
    assert!(!p.enabled("Save"), "nothing changed yet, nothing to save");
    p.click("Add output");
    assert!(p.has("Mount of output 3"), "Add output adds a row");
    p.type_into("Mount of output 3", "mid.opus");
    p.click("Remove output 1");
    assert!(!p.has("Mount of output 3"), "Remove takes a row away");
    assert_eq!(
        p.value("Mount of output 1"),
        "high.mp3",
        "the rest moves up"
    );
    assert!(p.enabled("Save"), "the rows changed");
    assert!(p.saves().is_empty(), "editing alone sends nothing");
    p.click("Save");
    assert_eq!(
        p.saves(),
        vec![vec![
            row("high.mp3", OutputFormatView::Mp3, 128),
            row("mid.opus", OutputFormatView::Opus, 64),
        ]],
        "Save sends every row, once"
    );
}

#[test]
fn a_format_is_chosen_from_a_list() {
    let mut p = page(paired_with(two_outputs(), OutputsState::Idle));
    p.click("Format of output 1");
    p.click("MP3");
    p.click("Save");
    assert_eq!(
        p.saves(),
        vec![vec![
            row("low.opus", OutputFormatView::Mp3, 32),
            row("high.mp3", OutputFormatView::Mp3, 128),
        ]]
    );
}

#[test]
fn removing_every_row_saves_an_empty_list() {
    let mut p = page(paired_with(two_outputs(), OutputsState::Idle));
    p.click("Remove output 1");
    p.click("Remove output 1");
    p.click("Save");
    assert_eq!(p.saves(), vec![Vec::<OutputRow>::new()]);
}

#[test]
fn save_is_off_while_saving() {
    let mut p = page(paired_with(two_outputs(), OutputsState::Saving));
    p.click("Add output");
    p.type_into("Mount of output 3", "mid.opus");
    assert!(p.says("Saving…"), "the page says it is saving");
    assert!(!p.enabled("Save"), "no second save while one is on its way");
    p.click("Save");
    assert!(p.saves().is_empty());
}

#[test]
fn a_failed_save_shows_the_reason_and_keeps_the_rows() {
    // After a failure the view holds the rows that were sent, so nothing typed is lost.
    let sent = vec![
        row("low.opus", OutputFormatView::Opus, 5),
        row("low.opus", OutputFormatView::Mp3, 128),
    ];
    let reason = "the mount low.opus is used twice";
    let mut p = page(paired_with(
        sent.clone(),
        OutputsState::Failed(reason.to_owned()),
    ));
    assert!(p.says(reason), "the page gives the server's reason");
    assert_eq!(p.value("Mount of output 2"), "low.opus");
    assert!(p.enabled("Save"), "a failed save can be tried again");
    p.click("Save");
    assert_eq!(p.saves(), vec![sent], "the same rows go again");
}

#[test]
fn a_saved_state_says_saved() {
    let p = page(paired_with(two_outputs(), OutputsState::Saved));
    assert!(p.says("Saved"));
    assert!(!p.enabled("Save"), "what the server holds is what shows");
}

#[test]
fn not_editable_shows_read_only_with_the_hint() {
    let mut view = paired_with(two_outputs(), OutputsState::Idle);
    view.outputs_editable = false;
    let p = page(view);
    assert!(p.says(OUTPUTS_READ_ONLY), "the hint says why");
    for name in ["Add output", "Save", "Remove output 1", "Mount of output 1"] {
        assert!(!p.has(name), "{name} is not offered");
    }
    for words in ["low.opus", "high.mp3"] {
        assert!(
            p.h.query_all_by_label_contains(words).any(|_| true),
            "the row {words} is listed"
        );
    }
}

#[test]
fn typing_in_a_row_edits_its_mount() {
    let mut p = page(paired_with(two_outputs(), OutputsState::Idle));
    p.type_into("Mount of output 1", "2");
    assert_eq!(p.value("Mount of output 1"), "low.opus2");
}

#[test]
fn the_output_controls_are_named_by_their_labels_and_reached_by_tab() {
    let mut p = page(paired_with(two_outputs(), OutputsState::Idle));
    for name in [
        "Format of output 1",
        "Bitrate of output 1",
        "Mount of output 1",
        "Remove output 1",
        "Format of output 2",
        "Add output",
        "Save",
    ] {
        assert!(p.has(name), "a control named {name}");
    }
    p.h.get_by_label("Mount of output 1").focus();
    p.h.run_steps(FRAMES);
    p.h.key_press(egui::Key::Tab);
    p.h.run_steps(FRAMES);
    assert!(
        p.h.get_by_label("Remove output 1").is_focused(),
        "Tab moves from the mount to Remove"
    );
    p.h.key_press(egui::Key::Tab);
    p.h.run_steps(FRAMES);
    assert!(
        p.h.get_by_label("Format of output 2").is_focused(),
        "Tab moves on to the next row"
    );
    p.h.key_press(egui::Key::Tab);
    p.h.run_steps(FRAMES);
    assert!(
        p.h.get_by_label("Bitrate of output 2").is_focused(),
        "Tab moves from the format to the bitrate"
    );
}

const MP3_STEPS: [u32; 14] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];

/// Types `text` into the bitrate field of an Opus row and presses Enter.
fn set_opus_kbps(p: &mut Page, text: &str) {
    p.h.get_by_label("Bitrate of output 1").click();
    p.h.run_steps(2);
    p.h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    p.h.get_by_label("Bitrate of output 1").type_text(text);
    p.h.key_press(egui::Key::Enter);
    p.h.run_steps(FRAMES);
}

#[test]
fn an_opus_row_takes_6_to_510() {
    for (typed, want) in [("6", 6), ("510", 510), ("2", 6), ("9999", 510)] {
        let mut p = page(paired_with(
            vec![row("a.opus", OutputFormatView::Opus, 64)],
            OutputsState::Idle,
        ));
        set_opus_kbps(&mut p, typed);
        p.click("Save");
        let saved = p.saves();
        assert_eq!(
            saved.first().and_then(|r| r.first()).map(|r| r.kbps),
            Some(want),
            "typed {typed}"
        );
    }
}

#[test]
fn an_mp3_row_offers_exactly_the_fourteen_bitrates() {
    let mut p = page(paired_with(
        vec![row("a.mp3", OutputFormatView::Mp3, 128)],
        OutputsState::Idle,
    ));
    p.click("Bitrate of output 1");
    let offered: Vec<u32> = MP3_STEPS
        .iter()
        .copied()
        .filter(|b| p.h.query_by_label(&format!("{b} kbit/s")).is_some())
        .collect();
    assert_eq!(offered, MP3_STEPS, "every step is offered");
    let all = p.h.query_all_by_label_contains(" kbit/s").count();
    assert_eq!(all, MP3_STEPS.len(), "nothing but the steps is offered");
    // The last steps may sit below the popup's fold.
    p.h.get_by_label("320 kbit/s").scroll_to_me();
    p.h.run_steps(FRAMES);
    p.click("320 kbit/s");
    p.click("Save");
    assert_eq!(
        p.saves(),
        vec![vec![row("a.mp3", OutputFormatView::Mp3, 320)]]
    );
}

#[test]
fn an_mp3_row_with_another_bitrate_keeps_it_until_one_is_picked() {
    let sent = vec![row("a.mp3", OutputFormatView::Mp3, 100)];
    let mut p = page(paired_with(
        sent.clone(),
        OutputsState::Failed("100 kbps can't be made".to_owned()),
    ));
    assert_eq!(
        p.value("Bitrate of output 1"),
        "100 kbit/s",
        "the refused value still shows"
    );
    p.click("Save");
    assert_eq!(p.saves(), vec![sent], "sent unchanged");
}
