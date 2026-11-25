use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
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
            RadioApp::new(
                Box::new(MockSource::new(tz).with_decks(4)),
                Box::new(fixed_clock),
            )
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
    click(&mut harness, "Music rules");
    assert!(shows(&harness, "Music — rules"), "the editor window opens");
    click(&mut harness, "Script");
    assert!(
        shows(&harness, "not run in this build"),
        "the script tab says edits are not applied"
    );
}

/// The rules editor sets how a playlist's items play. The duck depth shows only for items
/// that play over the music.
#[test]
fn the_rules_editor_sets_a_playlist_to_play_over_the_music() {
    let mut harness = harness();
    click(&mut harness, "Music rules");
    assert!(
        !shows(&harness, "Duck by"),
        "a duck depth for items in sequence"
    );
    let plays = harness
        .query_all_by_role(egui::accesskit::Role::ComboBox)
        .find(|n| {
            let node = n.accesskit_node();
            [node.value(), node.label()]
                .into_iter()
                .flatten()
                .any(|t| t == "In sequence")
        })
        .expect("the Plays picker names its setting");
    assert!(
        plays
            .accesskit_node()
            .label()
            .is_some_and(|l| l.starts_with("Plays")),
        "the picker's name starts with its label"
    );
    plays.click();
    harness.run_steps(FRAMES);
    click(&mut harness, "Over the music");
    assert!(
        shows(&harness, "Duck by"),
        "no duck depth for items over the music"
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
    assert!(!shows(&harness, "Program device"));
    click(&mut harness, "Mixer preferences");
    assert!(shows(&harness, "Program device"));
    assert!(shows(&harness, "Monitor device (SND)"));
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
            h.get_by_label("Music rules").rect(),
            h.get_by_label_contains(" tracks · next").rect(),
            h.get_by_label("Mixer preferences").rect(),
            h.get_by_label("Icecast preferences").rect(),
        ]
    };
    let before = rects(&harness);
    for cog in ["Music rules", "Mixer preferences", "Icecast preferences"] {
        harness.get_by_label(cog).hover();
        harness.run_steps(FRAMES);
        assert_eq!(rects(&harness), before, "hovering {cog} moves nothing");
    }
}

#[test]
fn play_next_starts_the_next_item() {
    let mut harness = harness();
    let on_air = "Halvard Pine – Harbours of Kelmora";
    let before = harness.query_all_by_label(on_air).count();
    // Deck A is on air: the first press arms Play next, the second fires it.
    click(&mut harness, "Play next");
    harness.get_by_label_contains("Confirm: play deck").click();
    harness.run_steps(FRAMES);
    let after = harness.query_all_by_label(on_air).count();
    assert!(
        after < before,
        "deck A no longer shows the item that was on air"
    );
}

/// The decks have one main action each and no per-deck next. The one next is in the header.
#[test]
fn deck_cards_have_no_skip_and_the_header_has_play_next() {
    let harness = harness();
    assert!(
        harness.query_all_by_label_contains("Skip").next().is_none(),
        "no control says Skip"
    );
    assert!(
        harness.query_all_by_label_contains("Play next").count() == 1,
        "only the header has Play next"
    );
    assert!(shows(&harness, "Play next"), "the header has Play next");
    assert!(shows(&harness, "Pause deck A"), "the deck on air pauses");
    for deck in ['B', 'C', 'D'] {
        let play = format!("Play deck {deck}");
        assert!(shows(&harness, &play), "deck {deck} has Play");
    }
    assert!(
        shows(&harness, "Fade out"),
        "the header fades the item on air"
    );
}

#[test]
fn the_header_says_preview_and_never_live_with_the_mock() {
    let harness = harness();
    assert!(shows(&harness, "PREVIEW"), "the header names the preview");
    assert!(!shows(&harness, "LIVE"), "the mock never claims to be live");
}

/// The Mixer ⚙ opens Preferences on the mixer's pages beside the strips. Preferences keeps
/// one size on every page, Done closes it, and the mixer keeps its size all along.
#[test]
fn the_mixer_cog_opens_preferences_beside_the_strips_and_done_closes_it() {
    let mut harness = harness();
    harness.state_mut().set_window(ToolWindow::Mixer, true);
    harness.run_steps(FRAMES);
    let size = |h: &Harness<'_, RadioApp>, w: ToolWindow| {
        h.ctx
            .memory(|m| m.area_rect(w.area_id()))
            .expect("the window floats as an egui window")
            .size()
    };
    let strips = size(&harness, ToolWindow::Mixer);
    click(&mut harness, "Mixer preferences");
    assert!(shows(&harness, "Done"), "Preferences opens");
    assert!(
        shows(&harness, "Program device"),
        "on the mixer's Devices page"
    );
    assert!(
        shows(&harness, "Icecast preferences"),
        "the strips stay in the mixer"
    );
    let devices = size(&harness, ToolWindow::Preferences);
    for tab in ["Processing", "Meters"] {
        click(&mut harness, tab);
        let other = size(&harness, ToolWindow::Preferences);
        assert!(
            (other - devices).abs().max_elem() <= 2.0,
            "Preferences keeps its size on {tab}: {devices:?} vs {other:?}"
        );
    }
    click(&mut harness, "Done");
    assert!(!shows(&harness, "Done"), "Done closes Preferences");
    let again = size(&harness, ToolWindow::Mixer);
    assert!(
        (again - strips).abs().max_elem() <= 2.0,
        "the mixer keeps the size of the strips: {strips:?} vs {again:?}"
    );
}

/// The ⚙ of the Icecast output strip opens Preferences on the Icecast page.
#[test]
fn the_icecast_strip_cog_opens_preferences_on_icecast() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    click(&mut harness, "Icecast preferences");
    assert!(shows(&harness, "Done"), "the page opens");
    assert!(shows(&harness, "Password"), "on the Icecast tab");
    assert!(!shows(&harness, "Program device"), "not on the Devices tab");
}

/// The Icecast state in the status line opens Preferences on the Icecast page. (The on-air
/// chip of the header is the streaming switch; `listen_link` tests it.)
#[test]
fn the_icecast_state_in_the_status_line_opens_its_preferences() {
    let mut harness = harness();
    assert!(!shows(&harness, "Done"));
    let state = harness
        .query_all(egui_kittest::kittest::By::new().predicate(|n| {
            n.value().is_some_and(|v| v.starts_with("Icecast "))
                || n.label().is_some_and(|v| v.starts_with("Icecast "))
        }))
        .next()
        .expect("the Icecast state of the status line");
    state.click();
    harness.run_steps(FRAMES);
    assert!(shows(&harness, "Done"), "Preferences opens");
    assert!(shows(&harness, "Password"), "on the Icecast page");
}

/// Escape over Preferences closes it, as Done does.
#[test]
fn escape_over_preferences_closes_it() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    click(&mut harness, "Mixer preferences");
    harness.get_by_label("Done").hover();
    harness.run_steps(FRAMES);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(FRAMES);
    assert!(!shows(&harness, "Done"), "Escape closes Preferences");
    assert!(
        shows(&harness, "Icecast preferences"),
        "the mixer stays open"
    );
}

/// Every control of a settings form has the one width of the form, whatever its kind.
#[test]
fn the_icecast_form_controls_share_one_width() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    click(&mut harness, "Icecast preferences");
    // The control is the widest node that a row label names; the label itself is narrower.
    let width = |label: &str| {
        harness
            .query_all_by_label(label)
            .map(|n| n.rect().width())
            .fold(0.0_f32, f32::max)
    };
    let host = width("Host");
    assert!(host > 0.0, "the Host field shows");
    for label in ["Port", "Mount", "User", "Password", "Bitrate"] {
        let w = width(label);
        assert!(
            (w - host).abs() <= 1.0,
            "{label} is as wide as Host: {w} vs {host}"
        );
    }
}

/// Each deck card names the playlist of its item, and a waiting deck says when it starts.
/// The rows that the decks hold carry the deck letter.
#[test]
fn deck_cards_name_their_playlist_and_start_and_rows_show_their_deck() {
    let harness = harness();
    let in_strip = |label: &str| {
        harness
            .query_all_by_label(label)
            .any(|n| n.rect().bottom() < 250.0)
    };
    assert!(
        in_strip("Music"),
        "a deck card shows the playlist of its item"
    );
    assert!(
        harness
            .query_all_by_label_contains("Starts 15:")
            .any(|n| n.rect().bottom() < 250.0),
        "a NEXT deck says when it starts"
    );
    assert!(
        harness
            .query_all_by_label("Deck C")
            .any(|n| n.rect().top() > 250.0),
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

/// The header has no mode control, and the Out meter spans most of the cluster.
#[test]
fn header_has_no_mode_control_and_a_wide_out_meter() {
    let harness = harness();
    for word in ["Assist", "Manual"] {
        assert!(!shows(&harness, word), "{word} must not be in the UI");
    }
    let meter = harness.get_by_label("Out meter").rect();
    let play = harness.get_by_label("Play next").rect();
    // The cluster runs from the left edge of the transport to the right edge of the meter.
    let cluster_w = meter.right() - play.left();
    assert!(
        meter.width() >= cluster_w / 2.0,
        "the Out meter is {} wide in a cluster {cluster_w} wide",
        meter.width()
    );
}

/// One deck on air and three waiting: the four cards are as tall as each other.
#[test]
fn deck_cards_have_equal_height_whatever_their_state() {
    let harness = harness();
    let rects: Vec<egui::Rect> = (0..4)
        .map(|i| radio_ui_egui::deck_card_rect(&harness.ctx, i).expect("deck card drawn"))
        .collect();
    assert!(shows(&harness, "ON AIR"), "one deck is on air");
    for r in &rects {
        assert!(
            (r.height() - rects[0].height()).abs() < 0.5,
            "deck cards differ in height: {rects:?}"
        );
    }
}

/// Every icon button of a deck card has one size: Play, Pause and the settings cog.
#[test]
fn deck_card_icon_buttons_share_one_size() {
    let harness = harness();
    let play = harness.get_by_label("Play deck B").rect();
    let cog = harness.get_by_label("Deck B preferences").rect();
    let pause = harness.get_by_label("Pause deck A").rect();
    assert!(
        (cog.size() - play.size()).abs().max_elem() < 0.5,
        "cog {cog:?} play {play:?}"
    );
    assert!(
        (pause.size() - play.size()).abs().max_elem() < 0.5,
        "pause {pause:?} play {play:?}"
    );
}

/// Arming a button never changes a card: the cards stay as tall as each other and as tall as
/// before, the header buttons keep their rects, and the sentence is a tooltip, not text.
#[test]
fn arming_a_transport_button_changes_no_layout() {
    let mut harness = harness();
    let card = |h: &Harness<'_, RadioApp>| -> Vec<egui::Rect> {
        (0..4)
            .map(|i| radio_ui_egui::deck_card_rect(&h.ctx, i).expect("deck card drawn"))
            .collect()
    };
    let before = card(&harness);
    let header = |h: &Harness<'_, RadioApp>| {
        ["Play next", "Fade out", "Stop after current"].map(|n| h.get_by_label(n).rect())
    };
    let header_before = header(&harness);
    harness.get_by_label("Play deck C").click();
    harness.run_steps(FRAMES);
    assert!(
        shows(&harness, "Confirm: play deck C now"),
        "the button is armed"
    );
    let armed = card(&harness);
    for (a, b) in armed.iter().zip(&before) {
        assert!(
            (a.size() - b.size()).abs().max_elem() < 0.5,
            "armed card resized"
        );
        assert!((a.min - b.min).abs().max_elem() < 0.5, "armed card moved");
    }
    harness.get_by_label("Play next").click();
    harness.run_steps(FRAMES);
    assert!(shows(&harness, "Confirm: play deck"), "Play next is armed");
    let header_armed = [
        harness.get_by_label_contains("Confirm: play deck").rect(),
        harness.get_by_label("Fade out").rect(),
        harness.get_by_label("Stop after current").rect(),
    ];
    for (a, b) in header_armed.iter().zip(&header_before) {
        assert!(
            (a.size() - b.size()).abs().max_elem() < 0.5,
            "header button resized"
        );
        assert!(
            (a.min - b.min).abs().max_elem() < 0.5,
            "header button moved"
        );
    }
}

/// The global transport is the largest control of the header: Play next is at least as tall
/// as the Large control, and Fade out and Stop after current are as tall as Play next.
#[test]
fn play_next_is_a_large_button_in_the_header_with_its_siblings_at_the_same_height() {
    let harness = harness();
    let play = harness.get_by_label("Play next").rect();
    assert!(
        play.height() >= radio_ui_egui::theme::size::CONTROL_LG,
        "Play next is {} high",
        play.height()
    );
    for name in ["Fade out", "Stop after current"] {
        let other = harness.get_by_label(name).rect();
        assert!(
            (other.height() - play.height()).abs() < 0.5,
            "{name} has the height of Play next"
        );
    }
    assert!(play.top() < 100.0, "Play next is in the header strip");
}

/// The line under the Out meter names the deck on air and its item, with the time left.
#[test]
fn the_header_names_what_is_on_air() {
    let harness = harness();
    assert!(
        shows(&harness, "Deck A on air"),
        "the badge of the deck on air"
    );
    let line = harness
        .query_all_by_label("Halvard Pine – Harbours of Kelmora")
        .any(|n| n.rect().bottom() < 250.0 && n.rect().left() > 800.0);
    assert!(line, "the item on air shows in the header cluster");
    assert!(!shows(&harness, "Nothing playing"), "something is on air");
}

/// The arrow keys move between the tabs of the settings page while a tab has the focus.
#[test]
fn the_arrow_keys_move_between_settings_tabs() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    click(&mut harness, "Mixer preferences");
    harness.get_by_label("Devices").focus();
    harness.run_steps(FRAMES);
    for _ in 0..2 {
        harness.key_press(egui::Key::ArrowDown);
        harness.run_steps(FRAMES);
    }
    assert!(shows(&harness, "Peak hold"), "two steps down is Meters");
    assert!(!shows(&harness, "Program device"), "Devices is gone");
    harness.key_press(egui::Key::ArrowUp);
    harness.run_steps(FRAMES);
    assert!(!shows(&harness, "Peak hold"), "one step up leaves Meters");
}

/// The Processing tab sends its change to the view source: after Done and a new visit,
/// the compressor that a person turned on is still on.
#[test]
fn the_processing_tab_turns_the_compressor_on() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    click(&mut harness, "Mixer preferences");
    click(&mut harness, "Processing");
    // The switch takes its name from its row label, "Compressor", like the heading above it.
    let switch = By::new()
        .label("Compressor")
        .role(egui::accesskit::Role::CheckBox);
    let compressor_on = |h: &Harness<'_, RadioApp>| {
        h.get(switch.clone()).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
    };
    assert!(!compressor_on(&harness), "the compressor starts off");
    harness.get(switch.clone()).click();
    harness.run_steps(FRAMES);
    click(&mut harness, "Done");
    click(&mut harness, "Mixer preferences");
    click(&mut harness, "Processing");
    assert!(compressor_on(&harness), "the source kept the compressor on");
}

/// Command+comma opens Preferences, as in every Craft app.
#[test]
fn command_comma_opens_preferences() {
    let mut harness = harness();
    assert!(!shows(&harness, "Done"));
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    harness.run_steps(FRAMES);
    assert!(shows(&harness, "Done"), "Preferences opens");
    assert!(
        shows(&harness, "Deck A fade-out"),
        "on its first page, Decks"
    );
}

/// Each window's ⚙ opens Preferences on that window's page.
#[test]
fn each_settings_button_opens_its_own_page() {
    for (window, button, row) in [
        ("Clock", "Clock preferences", "Show the seconds ring"),
        ("Alarms", "Alarms preferences", "Show acknowledged alarms"),
        ("History", "History preferences", "Show the last"),
    ] {
        let mut harness = harness();
        click(&mut harness, window);
        click(&mut harness, button);
        assert!(shows(&harness, row), "{button} shows {row}");
    }
    let mut harness = harness();
    click(&mut harness, "Deck B preferences");
    assert!(
        shows(&harness, "Deck B fade-out"),
        "a deck's ⚙ opens the Decks page"
    );
}

/// A setting changed in Preferences reaches the view source and is there when Preferences
/// opens again.
#[test]
fn a_setting_changed_in_preferences_stays_changed() {
    let mut harness = harness();
    click(&mut harness, "Clock");
    click(&mut harness, "Clock preferences");
    let h24 = |h: &Harness<'_, RadioApp>| {
        h.get_by_label("24-hour time").accesskit_node().toggled()
            == Some(egui::accesskit::Toggled::True)
    };
    assert!(h24(&harness), "24-hour time starts on");
    click(&mut harness, "24-hour time");
    click(&mut harness, "Done");
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    harness.run_steps(FRAMES);
    assert!(!h24(&harness), "the source kept it off");
}

/// The audio runs at one fixed rate, so the Devices page states it and offers no choice
/// that would change nothing.
#[test]
fn the_sample_rate_is_stated_not_offered() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    click(&mut harness, "Mixer preferences");
    assert!(
        harness.query_by_label_contains("48 kHz, fixed").is_some(),
        "the fixed rate shows"
    );
    assert!(
        harness
            .query_all_by_role(egui::accesskit::Role::ComboBox)
            .all(|n| n.accesskit_node().label().as_deref() != Some("Sample rate")),
        "no sample rate picker"
    );
}

/// Without a monitor output, SND cannot be heard, so the preview (which plays nothing)
/// offers it disabled instead of a button that seems to work.
#[test]
fn snd_is_disabled_without_a_monitor_device() {
    let mut harness = harness();
    click(&mut harness, "Mixer");
    let snd = harness.get_by_label("SND A");
    assert!(snd.accesskit_node().is_disabled(), "SND A is disabled");
}

/// Cart ducking plays now, so its settings no longer say it is not applied; the cart
/// modes still are not, and keep saying so.
#[test]
fn cart_ducking_no_longer_says_it_is_not_played() {
    let mut harness = harness();
    click(&mut harness, "Cartwall");
    click(&mut harness, "Cartwall preferences");
    assert!(
        !shows(&harness, "Saved, but the playout does not apply this yet"),
        "the ducking rows still say they are not played"
    );
}

/// Where there is no native menu bar, the window draws the menus, and an item runs its
/// command: Window ▸ Mixer opens the Mixer.
#[test]
fn the_window_menu_opens_a_tool_window() {
    use egui::accesskit::Role;
    let mut harness = harness();
    assert!(
        !shows(&harness, "Mixer preferences"),
        "the mixer starts closed"
    );
    harness
        .get(
            egui_kittest::kittest::By::new()
                .label("Window")
                .role(Role::MenuItem),
        )
        .click();
    harness.run_steps(FRAMES);
    let item = harness
        .query_all_by_label("Mixer")
        .find(|n| {
            matches!(
                n.accesskit_node().role(),
                Role::MenuItemCheckBox | Role::MenuItem | Role::MenuItemRadio
            )
        })
        .expect("Mixer in the Window menu");
    item.click();
    harness.run_steps(FRAMES);
    assert!(shows(&harness, "Mixer preferences"), "the mixer opens");
}

/// Preferences › Shortcuts opens the Keyboard Shortcuts dialog, where every command's keys can
/// be changed.
#[test]
fn the_shortcuts_page_opens_the_keyboard_shortcuts_dialog() {
    let mut harness = harness();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    harness.run_steps(FRAMES);
    click(&mut harness, "Shortcuts");
    let count = |h: &Harness<'_, RadioApp>| h.query_all_by_label_contains("Play Next").count();
    let before = count(&harness);
    click(&mut harness, "Keyboard Shortcuts…");
    assert!(
        count(&harness) > before,
        "the dialog lists the commands, Play Next among them"
    );
}

/// The sample log holds only today, so another day is empty; the day steps lead back.
#[test]
fn the_history_range_picker_shows_another_day_and_steps_back_to_today() {
    let mut harness = harness();
    click(&mut harness, "History");
    assert!(shows(&harness, " on air"), "the totals count the airings");
    assert!(
        !shows(&harness, "Previous day"),
        "the last hours have no day"
    );

    let picker = harness
        .query_all_by_role(egui::accesskit::Role::ComboBox)
        .find(|n| {
            let node = n.accesskit_node();
            let text = [node.value(), node.label()];
            text.into_iter().flatten().any(|t| t == "Last 3 hours")
        })
        .expect("the range picker names the range on show");
    picker.click();
    harness.run_steps(FRAMES);
    click(&mut harness, "Yesterday");
    assert!(shows(&harness, "No airings match."), "yesterday is empty");
    click(&mut harness, "Next day");
    assert!(shows(&harness, " on air"), "today has the airings again");
    assert!(!shows(&harness, "No airings match."));
}
