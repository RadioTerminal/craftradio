//! Right-click menus: one pattern for every pane and window.
//!
//! Each row of a menu is a `menu_item`. A row shows its keyboard shortcut on the right.
//! The accessible name of each row starts with its label.

use egui::RichText;

use crate::theme::Tokens;

/// The message for an action that needs the audio engine, which the preview does not have.
pub(crate) const NEEDS_ENGINE: &str = "preview — needs the audio engine";

/// The message for an action that this source cannot do yet.
pub(crate) fn not_here(view: &crate::view::StationView) -> String {
    view.say(NEEDS_ENGINE, "Not in this build yet").to_owned()
}

pub(crate) use crate::shortcuts::{COPY, CUT, DELETE, DUPLICATE, PASTE};

/// A row of a menu. It returns true on a click, and then closes the menu.
pub(crate) fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: Option<&str>) -> bool {
    menu_item_if(ui, true, label, shortcut)
}

/// A row of a menu. When `enabled` is false, the row shows but does nothing.
pub(crate) fn menu_item_if(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    shortcut: Option<&str>,
) -> bool {
    let mut button = egui::Button::new(label);
    if let Some(text) = shortcut
        .map(|id| crate::shortcuts::label(ui.ctx(), id))
        .filter(|t| !t.is_empty())
    {
        button = button.shortcut_text(text);
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
