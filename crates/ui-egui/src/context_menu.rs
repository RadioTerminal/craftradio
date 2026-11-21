//! Right-click menus: one pattern for every pane and window.
//!
//! Each row of a menu is a `menu_item`. A row shows its keyboard shortcut on the right.
//! The accessible name of each row starts with its label.

use egui::{Event, Key, KeyboardShortcut, Modifiers, RichText};

use crate::theme::Tokens;

/// The message for an action that needs the audio engine, which the preview does not have.
pub(crate) const NEEDS_ENGINE: &str = "preview — needs the audio engine";

pub(crate) const CUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::X);
pub(crate) const COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::C);
pub(crate) const PASTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::V);
pub(crate) const DUPLICATE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::D);
pub(crate) const DELETE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Delete);

/// A row of a menu. It returns true on a click, and then closes the menu.
pub(crate) fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: Option<KeyboardShortcut>,
) -> bool {
    menu_item_if(ui, true, label, shortcut)
}

/// A row of a menu. When `enabled` is false, the row shows but does nothing.
pub(crate) fn menu_item_if(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    shortcut: Option<KeyboardShortcut>,
) -> bool {
    let mut button = egui::Button::new(label);
    if let Some(s) = shortcut {
        button = button.shortcut_text(ui.ctx().format_shortcut(&s));
    }
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// A row that deletes something. It shows in the danger colour.
pub(crate) fn danger_item(ui: &mut egui::Ui, label: &str, t: &Tokens) -> bool {
    let clicked = ui.button(RichText::new(label).color(t.danger)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// A row with a check mark after the label when `on` is true.
pub(crate) fn check_item(ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    let text = if on {
        format!("{label} ✔")
    } else {
        label.to_owned()
    };
    menu_item(ui, &text, None)
}

/// The clipboard and edit shortcuts that a person pressed in this frame.
#[derive(Default, Clone, Copy)]
pub(crate) struct Pressed {
    pub cut: bool,
    pub copy: bool,
    pub paste: bool,
    pub duplicate: bool,
    pub delete: bool,
}

/// Read the shortcuts of this frame. A focused text field keeps them for itself.
pub(crate) fn pressed(ctx: &egui::Context) -> Pressed {
    if ctx.egui_wants_keyboard_input() {
        return Pressed::default();
    }
    ctx.input_mut(|i| {
        let mut p = Pressed::default();
        for e in &i.events {
            match e {
                Event::Cut => p.cut = true,
                Event::Copy => p.copy = true,
                Event::Paste(_) => p.paste = true,
                _ => {}
            }
        }
        p.duplicate = i.consume_shortcut(&DUPLICATE);
        p.delete = i.consume_shortcut(&DELETE) || i.consume_key(Modifiers::NONE, Key::Backspace);
        p
    })
}
