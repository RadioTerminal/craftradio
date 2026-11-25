//! The native menu's Edit keys reach the window that has the focus.

use egui::{Event, Key, Modifiers, RawInput, ViewportId, ViewportInfo};
use radio_ui_egui::key_route;

fn tool() -> ViewportId {
    ViewportId::from_hash_of("a tool window")
}

fn input(focused: ViewportId, events: Vec<Event>) -> RawInput {
    let mut raw = RawInput::default();
    for id in [ViewportId::ROOT, tool()] {
        raw.viewports.insert(
            id,
            ViewportInfo {
                focused: Some(id == focused),
                ..ViewportInfo::default()
            },
        );
    }
    raw.events = events;
    raw
}

fn select_all() -> Event {
    Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }
}

fn pointer() -> Event {
    Event::PointerMoved(egui::pos2(4.0, 4.0))
}

/// What the tool window's input holds after `deliver` in its own pass.
fn delivered(ctx: &egui::Context) -> Vec<Event> {
    let mut seen = Vec::new();
    let mut raw = RawInput {
        viewport_id: tool(),
        ..RawInput::default()
    };
    raw.viewports.insert(tool(), ViewportInfo::default());
    let mut out = ctx.run_ui(raw, |ui| {
        key_route::deliver(ui.ctx());
        seen = ui.input(|i| i.events.clone());
    });
    out.textures_delta.clear();
    seen
}

#[test]
fn a_paste_and_menu_keys_go_to_the_focused_tool_window() {
    let ctx = egui::Context::default();
    let before = vec![pointer(), Event::Paste("key from the clipboard".into())];
    let mut raw = input(
        tool(),
        vec![
            pointer(),
            select_all(),
            Event::Paste("key from the clipboard".into()),
        ],
    );
    assert_eq!(key_route::route(&ctx, &before, &mut raw), Some(tool()));
    // The main window keeps only its own input.
    assert_eq!(raw.events, vec![pointer()]);
    let got = delivered(&ctx);
    assert!(got.contains(&select_all()));
    assert!(got.contains(&Event::Paste("key from the clipboard".into())));
    // Delivered once.
    assert!(delivered(&ctx).is_empty());
}

#[test]
fn the_main_window_keeps_its_keys_while_it_has_the_focus() {
    let ctx = egui::Context::default();
    let before = vec![pointer()];
    let events = vec![pointer(), Event::Copy];
    let mut raw = input(ViewportId::ROOT, events.clone());
    assert_eq!(key_route::route(&ctx, &before, &mut raw), None);
    assert_eq!(raw.events, events);
}

#[test]
fn input_the_menu_took_away_does_not_hide_what_it_added() {
    let ctx = egui::Context::default();
    // An open menu took the first click; then it added ⌘A.
    let click = Event::PointerButton {
        pos: egui::pos2(1.0, 1.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    };
    let before = vec![click, pointer()];
    let mut raw = input(tool(), vec![pointer(), select_all()]);
    key_route::route(&ctx, &before, &mut raw);
    assert_eq!(raw.events, vec![pointer()]);
    assert_eq!(delivered(&ctx), vec![select_all()]);
}
