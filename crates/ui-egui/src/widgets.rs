//! Shared controls that the theme alone cannot make.

use egui::{Align2, Sense, Stroke, StrokeKind, WidgetInfo, WidgetType};

use crate::theme::{self, Tokens, size};

/// An icon button that keeps one exact size in every state.
// Hover, press and focus change only colours. The edge width never changes.
// The accessible name `name` must start with the glyph, which is the visible label.
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    selected: bool,
    name: &str,
    hint: &str,
) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let side = egui::Vec2::splat(size::ICON_BUTTON);
    let (rect, resp) = ui.allocate_exact_size(side, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, name));
    if ui.is_rect_visible(rect) {
        let (fill, edge) = if resp.is_pointer_button_down_on() {
            (t.pressed, t.accent)
        } else if selected {
            (t.selection, t.accent)
        } else if resp.hovered() {
            (t.hover, t.field_border)
        } else {
            (egui::Color32::TRANSPARENT, egui::Color32::TRANSPARENT)
        };
        let edge = if resp.has_focus() { t.accent_ink } else { edge };
        let p = ui.painter();
        let stroke = Stroke::new(size::HAIRLINE, edge);
        p.rect(rect, t.radius, fill, stroke, StrokeKind::Inside);
        let ink = if selected { t.accent_ink } else { t.text };
        let font = theme::text(theme::CARD);
        p.text(rect.center(), Align2::CENTER_CENTER, glyph, font, ink);
    }
    resp.on_hover_text(hint)
}
