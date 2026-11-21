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

/// An icon button for a guarded action. While armed, it shows the danger colour and a ring.
/// The ring shrinks as the time to press again runs out. `armed` is the part of the time left.
pub(crate) fn guard_button(
    ui: &mut egui::Ui,
    glyph: &str,
    name: &str,
    hint: &str,
    armed: Option<f32>,
) -> egui::Response {
    let Some(left) = armed else {
        return icon_button(ui, glyph, false, name, hint);
    };
    let t = Tokens::get(ui.ctx());
    let side = egui::Vec2::splat(size::ICON_BUTTON);
    let (rect, resp) = ui.allocate_exact_size(side, Sense::click());
    let armed_name = format!("{name} — press again");
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, true, &armed_name));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let stroke = Stroke::new(size::HAIRLINE, t.danger);
        p.rect(rect, t.radius, t.danger_fill, stroke, StrokeKind::Inside);
        let font = theme::text(theme::CARD);
        p.text(rect.center(), Align2::CENTER_CENTER, glyph, font, t.text);
        // The ring: a part of a circle around the button, as long as the time left.
        let r = rect.width() / 2.0 + size::HAIRLINE * 2.0;
        let steps = size::RING_STEPS;
        let sweep = std::f32::consts::TAU * left.clamp(0.0, 1.0);
        let points: Vec<egui::Pos2> = (0..=steps)
            .map(|k| {
                let a = -std::f32::consts::FRAC_PI_2 + sweep * k as f32 / steps as f32;
                rect.center() + egui::vec2(a.cos(), a.sin()) * r
            })
            .collect();
        p.add(egui::Shape::line(
            points,
            Stroke::new(size::HAIRLINE * 2.0, t.danger),
        ));
    }
    resp.on_hover_text(format!("{hint} — press again to confirm"))
}
