use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{MockSource, RadioApp, ToolWindow};

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

fn shows(harness: &Harness<'_, RadioApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

fn click(harness: &mut Harness<'_, RadioApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(FRAMES);
}

#[test]
fn the_window_shows_upcoming_and_the_next_fixed_news() {
    let harness = harness();
    assert!(
        shows(&harness, "Upcoming"),
        "the scheduled pane has an Upcoming tab"
    );
    // 13:58 UTC is 14:58 in Ljubljana, so the next full hour is 15:00:00 local.
    assert!(
        shows(&harness, "15:00:00"),
        "the next full hour is on screen"
    );
}

#[test]
fn the_cog_opens_the_rules_editor_with_a_script_tab() {
    let mut harness = harness();
    assert!(!shows(&harness, "Music — rules"));
    click(&mut harness, "⚙ Music rules");
    assert!(shows(&harness, "Music — rules"), "the editor window opens");
    click(&mut harness, "Script");
    assert!(
        shows(&harness, "not run in this build"),
        "the script tab says edits are not applied"
    );
}

#[test]
fn each_tool_window_opens_from_the_deck_strip() {
    let mut harness = harness();
    let windows = [
        (ToolWindow::Cartwall, "preview — no audio"),
        (ToolWindow::Mixer, "simulated levels"),
        (ToolWindow::Clock, " in "),
        (ToolWindow::History, "AIRED AT"),
        (ToolWindow::Library, "Dock into main window"),
    ];
    for (window, marker) in windows {
        let title = window.title();
        assert!(!shows(&harness, marker), "{title} is closed at start");
        click(&mut harness, title);
        assert!(shows(&harness, marker), "{title} opens");
        // Close it again, so the next toggle is not under a floating window.
        harness.state_mut().set_window(window, false);
        harness.run_steps(FRAMES);
    }
}

#[test]
fn the_mixer_cog_shows_the_device_pickers() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    assert!(!shows(&harness, "Output device (Out)"));
    click(&mut harness, "⚙ Mixer settings");
    assert!(shows(&harness, "Output device (Out)"));
    assert!(shows(&harness, "Cue device (PFL)"));
    assert!(shows(&harness, "Live input device"));
}

/// The number of library files that the count line says are visible.
fn visible_files(harness: &Harness<'_, RadioApp>) -> usize {
    let node = harness.get_by_label_contains(" files · drag");
    let label = node.accesskit_node().value().unwrap_or_default();
    label
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .expect("the count line starts with a number")
}

#[test]
fn the_library_search_narrows_the_rows() {
    let mut harness = harness();
    click(&mut harness, "Library");
    let before = visible_files(&harness);
    let search = harness.get_by_label("Search");
    search.focus();
    search.type_text("Kelmora");
    harness.run_steps(FRAMES);
    let after = visible_files(&harness);
    assert!(after > 0, "some titles match");
    assert!(
        after < before,
        "the search hides rows that do not match ({after} of {before})"
    );
}

#[test]
fn icon_buttons_keep_their_size_on_hover() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    let rects = |h: &Harness<'_, RadioApp>| {
        [
            h.get_by_label("⚙ Music rules").rect(),
            h.get_by_label_contains(" tracks · next").rect(),
            h.get_by_label("⚙ Mixer settings").rect(),
            h.get_by_label("⚙ Icecast settings").rect(),
        ]
    };
    let before = rects(&harness);
    for cog in ["⚙ Music rules", "⚙ Mixer settings", "⚙ Icecast settings"] {
        harness.get_by_label(cog).hover();
        harness.run_steps(FRAMES);
        assert_eq!(rects(&harness), before, "hovering {cog} moves nothing");
    }
}

#[test]
fn skip_on_deck_a_starts_the_next_item() {
    let mut harness = harness();
    let on_air = "Halvard Pine – Harbours of Kelmora";
    let before = harness.query_all_by_label(on_air).count();
    // Deck A is on air: the first press arms Skip, the second fires it.
    click(&mut harness, "⏭ Skip deck A");
    click(&mut harness, "⏭ Skip deck A — press again");
    let after = harness.query_all_by_label(on_air).count();
    assert!(after < before, "deck A no longer shows the skipped item");
}
