//! The number of decks in use: the deck strip and the Decks page of Preferences follow it.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 6;

fn app(decks: Option<usize>) -> Harness<'static, RadioApp> {
    let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
    let now: jiff::Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            let source = MockSource::new(tz);
            let source = match decks {
                Some(n) => source.with_decks(n),
                None => source,
            };
            RadioApp::new(Box::new(source), Box::new(move || now))
        });
    h.run_steps(FRAMES);
    h
}

fn shows(h: &Harness<'_, RadioApp>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

#[test]
fn a_station_that_never_chose_shows_two_decks() {
    let h = app(None);
    assert!(shows(&h, "Pause deck A") && shows(&h, "Play deck B"));
    assert!(!shows(&h, "Play deck C") && !shows(&h, "Play deck D"));
}

#[test]
fn the_strip_shows_as_many_decks_as_the_setting_says() {
    let h = app(Some(3));
    assert!(shows(&h, "Play deck C"), "the third deck is in use");
    assert!(!shows(&h, "Play deck D"), "the fourth is not");
}

#[test]
fn the_decks_page_has_the_fades_of_the_decks_in_use_and_restores_two() {
    let mut h = app(Some(4));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    h.run_steps(FRAMES);
    assert!(shows(&h, "Number of decks"));
    assert!(shows(&h, "Deck D fade-out"));
    h.get_by_label("Restore page defaults").click();
    h.run_steps(FRAMES);
    assert!(shows(&h, "Deck B fade-out"));
    assert!(
        !shows(&h, "Deck C fade-out"),
        "the page follows the number of decks"
    );
    assert!(!shows(&h, "Play deck C"), "and so does the deck strip");
}

#[test]
fn the_mixer_has_a_strip_for_each_deck_in_use_only() {
    let mut h = app(None);
    h.get_by_label("Mixer").click();
    h.run_steps(FRAMES);
    assert!(shows(&h, "SND B"), "deck B has its strip");
    assert!(shows(&h, "SND Program"), "Program keeps its strip");
    assert!(
        !shows(&h, "SND C"),
        "a deck that is not in use has no strip"
    );
}

const ON_AIR: &str = "Halvard Pine – Harbours of Kelmora";

/// The position bar of the deck on air is a slider: a click on it moves the deck there. A
/// deck that the preview cannot move has no slider.
#[test]
fn a_click_on_the_position_bar_moves_the_deck_on_air() {
    let mut h = app(Some(4));
    assert!(
        !shows(&h, "Deck B position"),
        "a deck that cannot seek shows a slider"
    );
    h.get_by_label("Deck A position").click();
    h.run_steps(FRAMES);
    assert!(
        h.query_by_label_contains("deck A is at").is_some(),
        "the click did not move deck A"
    );
}

/// Left and Right on the focused position bar move the deck, and the bar keeps the focus.
#[test]
fn the_arrow_keys_on_the_position_bar_move_the_deck_and_keep_the_focus() {
    let mut h = app(Some(4));
    h.get_by_label("Deck A position").focus();
    h.run_steps(FRAMES);
    for key in [egui::Key::ArrowLeft, egui::Key::ArrowRight] {
        h.key_press(key);
        h.run_steps(FRAMES);
        assert!(
            h.get_by_label("Deck A position").is_focused(),
            "{key:?} moved the focus off the bar"
        );
    }
    assert!(
        h.query_by_label_contains("deck A is at").is_some(),
        "the keys did not move deck A"
    );
}

/// Cue to start in the deck menu puts the deck on air at the start of its item.
#[test]
fn cue_to_start_moves_the_deck_to_the_start() {
    let mut h = app(Some(4));
    h.query_all_by_label(ON_AIR)
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the deck card names the item on air")
        .click_secondary();
    h.run_steps(FRAMES);
    h.get_by_label("Cue to start").click();
    h.run_steps(FRAMES);
    assert!(
        h.query_by_label_contains("deck A is at 0:00").is_some(),
        "Cue to start did not move deck A to its start"
    );
}
