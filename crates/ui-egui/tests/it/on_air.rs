//! The On air switch: the button above the output strips, the Playout menu item and the
//! shortcut. Each needs a second press within the arm time, both ways.

use std::sync::{Arc, Mutex};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::theme::size;
use radio_ui_egui::view::{Edit, MeterFrame, Row, StationView, Status, ViewSource};
use radio_ui_egui::{MockSource, RadioApp};

/// One frame of the harness, in seconds.
const STEP_S: f32 = 0.25;
const FRAMES: usize = 4;

/// The mock, keeping each edit it gets. With `going`, the station has just gone on air and
/// Icecast has not connected yet.
struct Recording {
    inner: MockSource,
    edits: Arc<Mutex<Vec<Edit>>>,
    going: bool,
}

impl ViewSource for Recording {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let mut view = self.inner.snapshot(now);
        if self.going {
            view.preview = false;
            view.going_on_air = true;
            view.output.icecast = Status::Failed("connection refused".to_owned());
        }
        view
    }
    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        if let Ok(mut edits) = self.edits.lock() {
            edits.push(edit.clone());
        }
        self.inner.edit(edit)
    }
    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
    fn meters(&self) -> Option<MeterFrame> {
        self.inner.meters()
    }
}

fn setting(part: &str, key: &str, value: bool) -> Edit {
    Edit::SetPartSetting {
        part: part.to_owned(),
        key: key.to_owned(),
        value: value.into(),
    }
}

/// The app on the mock. `setup` edits run on the mock first and are not recorded.
fn harness(
    edits: &Arc<Mutex<Vec<Edit>>>,
    setup: Vec<Edit>,
    going: bool,
) -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let edits = Arc::clone(edits);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_step_dt(STEP_S)
        .build_eframe(move |_cc| {
            let mut inner = MockSource::new(tz);
            for e in setup {
                inner.edit(e).expect("the mock takes the setting");
            }
            let source = Recording {
                inner,
                edits,
                going,
            };
            RadioApp::new(Box::new(source), Box::new(move || now))
        });
    h.run_steps(FRAMES);
    h
}

/// The air switches that reached the source: the values of `air.on_air` it was set to.
fn switches(edits: &Arc<Mutex<Vec<Edit>>>) -> Vec<bool> {
    edits
        .lock()
        .expect("edits")
        .iter()
        .filter_map(|e| match e {
            Edit::SetPartSetting { part, key, value }
                if part == radio_model::AIR_PART && key == radio_model::ON_AIR_KEY =>
            {
                value.as_bool()
            }
            _ => None,
        })
        .collect()
}

fn shows(h: &Harness<'_, RadioApp>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

/// Frames past the arm time, so an armed press has run out.
fn wait_out_the_arm_time(h: &mut Harness<'_, RadioApp>) {
    let frames = (size::ARM_S / f64::from(STEP_S)).ceil() as usize + FRAMES;
    h.run_steps(frames);
}

fn shortcut(h: &mut Harness<'_, RadioApp>) {
    h.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::O,
    );
    h.run_steps(FRAMES);
}

#[test]
fn one_press_does_not_switch() {
    let edits = Arc::default();
    let mut h = harness(&edits, Vec::new(), false);
    shortcut(&mut h);
    assert!(switches(&edits).is_empty(), "one press only arms");
    assert!(
        shows(&h, "Press again to go off air"),
        "the station says what the second press does"
    );
}

#[test]
fn two_presses_within_the_arm_time_switch() {
    let edits = Arc::default();
    let mut h = harness(&edits, Vec::new(), false);
    shortcut(&mut h);
    shortcut(&mut h);
    assert_eq!(switches(&edits), vec![false], "the station goes off air");
    assert!(shows(&h, "Off air"), "the status bar says Off air in words");
    // And back: on air takes two presses too.
    shortcut(&mut h);
    assert_eq!(switches(&edits), vec![false], "one press only arms");
    assert!(shows(&h, "Press again to go on air"));
    shortcut(&mut h);
    assert_eq!(switches(&edits), vec![false, true]);
}

#[test]
fn two_presses_apart_do_not_switch() {
    let edits = Arc::default();
    let mut h = harness(&edits, Vec::new(), false);
    shortcut(&mut h);
    wait_out_the_arm_time(&mut h);
    shortcut(&mut h);
    assert!(
        switches(&edits).is_empty(),
        "the late press only arms again"
    );
}

/// A key pressed and held: the first press, then a press while it is still down, which egui
/// takes as a repeat. A repeat is not a second press.
fn hold(h: &mut Harness<'_, RadioApp>, key: egui::Key, modifiers: egui::Modifiers) {
    for _ in 0..2 {
        h.key_down_modifiers(modifiers, key);
        h.run_steps(FRAMES);
    }
    h.key_up_modifiers(modifiers, key);
    h.run_steps(FRAMES);
}

#[test]
fn a_held_key_is_not_a_second_press() {
    let edits = Arc::default();
    let mut h = harness(&edits, Vec::new(), false);
    let chord = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    hold(&mut h, egui::Key::O, chord);
    assert!(switches(&edits).is_empty(), "a held shortcut only arms");
}

/// The shortcut's key press as the native macOS menu sends it: AppKit takes the key
/// equivalent, on each autorepeat too, and craft-menu hands it on as a press that never says
/// repeat. Only the key-up comes from the keyboard itself.
fn native_press(h: &mut Harness<'_, RadioApp>) {
    let chord = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    h.event(egui::Event::Key {
        key: egui::Key::O,
        physical_key: Some(egui::Key::O),
        pressed: true,
        repeat: false,
        modifiers: chord,
    });
    h.run_steps(FRAMES);
}

fn native_release(h: &mut Harness<'_, RadioApp>) {
    h.event(egui::Event::Key {
        key: egui::Key::O,
        physical_key: Some(egui::Key::O),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    });
    h.run_steps(FRAMES);
}

#[test]
fn a_held_shortcut_from_the_native_menu_does_not_switch() {
    let edits = Arc::default();
    let mut h = harness(&edits, Vec::new(), false);
    native_press(&mut h);
    native_press(&mut h);
    assert!(
        switches(&edits).is_empty(),
        "a second press with no key-up between is the key held down"
    );
}

#[test]
fn the_shortcut_switches_after_its_key_comes_up() {
    let edits = Arc::default();
    let mut h = harness(&edits, Vec::new(), false);
    native_press(&mut h);
    native_release(&mut h);
    native_press(&mut h);
    assert_eq!(switches(&edits), vec![false], "two real presses switch");
}

/// The app with the Library's search field holding the keys. The native menu then sends the
/// shortcut as a click of the On Air item, not as a key press.
fn typing(edits: &Arc<Mutex<Vec<Edit>>>) -> Harness<'static, RadioApp> {
    let mut h = harness(edits, Vec::new(), false);
    h.get_by_label("Library").click();
    h.run_steps(FRAMES);
    h.get_by_label("Search").focus();
    h.run_steps(FRAMES);
    h
}

/// A click of the On Air menu item as the native menu reports it.
fn menu_click(h: &mut Harness<'_, RadioApp>) {
    h.state_mut().menu_click("air.toggle");
    h.run_steps(FRAMES);
}

fn modifiers(h: &mut Harness<'_, RadioApp>, m: egui::Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    h.run_steps(FRAMES);
}

#[test]
fn a_held_shortcut_while_typing_does_not_switch() {
    let edits = Arc::default();
    let mut h = typing(&edits);
    modifiers(&mut h, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    menu_click(&mut h);
    menu_click(&mut h);
    assert!(
        switches(&edits).is_empty(),
        "an autorepeat of the held shortcut is not a second press"
    );
    native_release(&mut h);
    menu_click(&mut h);
    assert_eq!(
        switches(&edits),
        vec![false],
        "after the key came up, the next press switches"
    );
}

#[test]
fn a_pointer_click_of_the_menu_item_still_counts_while_typing() {
    let edits = Arc::default();
    let mut h = typing(&edits);
    menu_click(&mut h);
    assert!(switches(&edits).is_empty(), "the first click arms");
    assert!(shows(&h, "Press again to go off air"));
    menu_click(&mut h);
    assert_eq!(switches(&edits), vec![false], "the second click switches");
}

#[test]
fn the_menu_item_and_shortcut_arm_too() {
    let edits = Arc::default();
    // The Mixer stays closed: the menu and the key work without the button on screen.
    let mut h = harness(&edits, Vec::new(), false);
    shortcut(&mut h);
    assert!(switches(&edits).is_empty(), "one key press only arms");
    assert!(
        shows(&h, "Press again to go off air"),
        "the station says what a second press does"
    );
    shortcut(&mut h);
    assert_eq!(
        switches(&edits),
        vec![false],
        "the second key press switches"
    );

    h.get(By::new().label("Playout").role(Role::MenuItem))
        .click();
    h.run_steps(FRAMES);
    let item = h
        .query_all_by_label("On Air")
        .find(|n| {
            matches!(
                n.accesskit_node().role(),
                Role::MenuItemCheckBox | Role::MenuItem
            )
        })
        .expect("On Air in the Playout menu");
    item.click();
    h.run_steps(FRAMES);
    assert_eq!(switches(&edits), vec![false], "the menu item only arms");
    assert!(shows(&h, "Press again to go on air"));
    shortcut(&mut h);
    assert_eq!(
        switches(&edits),
        vec![false, true],
        "a key press after the menu's arm switches"
    );
}

#[test]
fn off_air_shows_in_words_in_the_status_bar() {
    let edits = Arc::default();
    let on = harness(&edits, Vec::new(), false);
    assert!(
        !shows(&on, "Off air"),
        "on air, the status bar does not say off air"
    );
    let off = harness(
        &edits,
        vec![setting(
            radio_model::AIR_PART,
            radio_model::ON_AIR_KEY,
            false,
        )],
        false,
    );
    assert!(shows(&off, "Off air"), "off air, the status bar says so");
}

#[test]
fn going_on_air_shows_while_reconnecting() {
    let edits = Arc::default();
    let mut h = harness(&edits, Vec::new(), true);
    assert!(
        !shows(&h, "failed: connection refused"),
        "the status bar shows no failure while going on air"
    );
    // The status bar's Icecast says so, and opens the Icecast page, which does not call it
    // down either.
    h.query_all_by_label_contains("going on air…")
        .find(|n| {
            let node = n.accesskit_node();
            let text = node.label().or_else(|| node.value());
            text.is_some_and(|l| l.starts_with("Icecast "))
        })
        .expect("the status bar's Icecast says going on air")
        .click();
    h.run_steps(FRAMES);
    assert!(shows(&h, "Icecast going on air…"), "the Icecast page opens");
    assert!(
        !shows(&h, "Icecast down"),
        "the Icecast page shows no failure"
    );
}

#[test]
fn the_deck_confirm_preference_does_not_skip_the_second_press() {
    let edits = Arc::default();
    let no_confirm = vec![setting(radio_model::DECKS_PART, "confirm_on_air", false)];
    let mut h = harness(&edits, no_confirm, false);
    shortcut(&mut h);
    assert!(
        switches(&edits).is_empty(),
        "going off air is never one press"
    );
}
