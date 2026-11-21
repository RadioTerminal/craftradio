//! Settings live with the part that they configure. There is no general settings window.
//!
//! Each window and pane has a ⚙ in its header row. The ⚙ opens a settings section inside that
//! same window. In the preview, settings are local UI state, and the section says so.

use egui::{Align, Layout, RichText, Stroke};

use crate::theme::{self, Tokens, pad, size, space};
use crate::widgets;

pub(crate) const NOT_SAVED: &str = "preview — not saved";

/// The ⚙ toggle for the settings of `part`. Its accessible name starts with its visible label.
pub(crate) fn settings_button(ui: &mut egui::Ui, part: &str, open: &mut bool) {
    let name = format!("⚙ {part} settings");
    let resp = widgets::icon_button(ui, "⚙", *open, &name, &format!("{part} settings"));
    if resp.clicked() {
        *open = !*open;
    }
}

/// A header row: the title on the left, then `extra` and the ⚙ on the right.
pub(crate) fn header(
    ui: &mut egui::Ui,
    title: &str,
    open: &mut bool,
    extra: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(theme::text(theme::SECTION))
                .strong(),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            settings_button(ui, title, open);
            extra(ui);
        });
    });
}

/// The settings section for `part`, in a card, with the "not saved" note.
pub(crate) fn section(ui: &mut egui::Ui, part: &str, t: &Tokens, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(size::HAIRLINE, t.accent))
        .corner_radius(t.radius_lg)
        .inner_margin(pad::CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{part} settings"))
                        .font(theme::text(theme::CARD))
                        .strong(),
                );
                let note = RichText::new(NOT_SAVED).font(theme::text(theme::CAPTION));
                ui.label(note.color(t.text_muted));
            });
            ui.add_space(space::S);
            add(ui);
        });
    ui.add_space(space::S);
}

/// A label in the left column of a settings grid.
pub(crate) fn label(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.label(
        RichText::new(text)
            .font(theme::text(theme::LABEL))
            .color(t.text_muted),
    );
}
