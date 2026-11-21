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
        (ToolWindow::Clock, "ends in"),
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

#[test]
fn the_header_says_preview_and_never_live_with_the_mock() {
    let harness = harness();
    assert!(shows(&harness, "PREVIEW"), "the header names the preview");
    assert!(!shows(&harness, "LIVE"), "the mock never claims to be live");
}

/// The floating Mixer grows when its ⚙ section opens and shrinks back when it closes.
#[test]
fn the_mixer_window_follows_the_size_of_its_content() {
    let mut harness = harness();
    harness.state_mut().set_window(ToolWindow::Mixer, true);
    harness.run_steps(FRAMES);
    let id = ToolWindow::Mixer.area_id();
    let height = |h: &Harness<'_, RadioApp>| {
        h.ctx
            .memory(|m| m.area_rect(id))
            .expect("the mixer floats as an egui window")
            .height()
    };
    let closed = height(&harness);
    click(&mut harness, "⚙ Mixer settings");
    let opened = height(&harness);
    assert!(
        opened > closed + 2.0,
        "the window grows: {closed} → {opened}"
    );
    click(&mut harness, "⚙ Mixer settings");
    let again = height(&harness);
    assert!(
        again < opened - 2.0,
        "the window shrinks back: {opened} → {again}"
    );
    assert!(
        (again - closed).abs() <= 2.0,
        "back to its size: {closed} vs {again}"
    );
}

/// Each deck card names the playlist of its item, and a waiting deck says when it starts.
/// The rows that the decks hold carry the deck letter.
#[test]
fn deck_cards_name_their_playlist_and_start_and_rows_show_their_deck() {
    let harness = harness();
    let in_strip = |label: &str| {
        harness
            .query_all_by_label(label)
            .any(|n| n.rect().bottom() < 200.0)
    };
    assert!(
        in_strip("Music"),
        "a deck card shows the playlist of its item"
    );
    assert!(
        harness
            .query_all_by_label_contains("Starts 15:")
            .any(|n| n.rect().bottom() < 200.0),
        "a NEXT deck says when it starts"
    );
    assert!(
        harness
            .query_all_by_label("Deck C")
            .any(|n| n.rect().top() > 200.0),
        "the row on deck C shows the C badge"
    );
}

#[test]
fn a_library_column_changes_width_after_a_drag() {
    let mut harness = harness();
    harness.state_mut().set_window(ToolWindow::Library, true);
    harness.run_steps(FRAMES);
    let artist = harness.get_by_label("ARTIST").rect();
    let title = harness
        .query_all_by_label("TITLE")
        .map(|node| node.rect())
        .find(|rect| {
            (rect.center().y - artist.center().y).abs() < 1.0 && rect.left() < artist.left()
        })
        .expect("Library title header beside the artist header");
    let before = artist.left() - title.left();
    let start = egui::pos2(artist.left() - 4.0, title.center().y);
    let end = start + egui::vec2(48.0, 0.0);
    harness.hover_at(start);
    harness.run_steps(1);
    harness.drag_at(start);
    harness.run_steps(1);
    harness.hover_at(end);
    harness.run_steps(2);
    harness.drop_at(end);
    harness.run_steps(FRAMES);
    let after = harness.get_by_label("ARTIST").rect().left() - title.left();
    assert!(
        after > before,
        "the title column must grow after dragging: {before} -> {after}"
    );
}

#[test]
fn header_shows_full_mode_names_at_1440() {
    let harness = harness();
    assert!(harness.query_all_by_label("Assist").next().is_some());
}
