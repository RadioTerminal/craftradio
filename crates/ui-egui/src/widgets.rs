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

/// A label on one line. A long text ends with "…", and the tooltip then shows all of it.
pub(crate) fn truncated_label(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.add(egui::Label::new(text).truncate())
}

/// A truncated label at most `max` wide, for a cell or a header part with a fixed limit.
pub(crate) fn truncated_label_max(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    max: f32,
) -> egui::Response {
    let max = max.min(ui.available_width()).max(0.0);
    ui.scope(|ui| {
        ui.set_max_width(max);
        truncated_label(ui, text)
    })
    .inner
}

/// Runs `add` in a part of the row at most `max` wide.
pub(crate) fn with_max_width<R>(
    ui: &mut egui::Ui,
    max: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let max = max.min(ui.available_width()).max(0.0);
    ui.scope(|ui| {
        ui.set_max_width(max);
        add(ui)
    })
    .inner
}

/// The deck mark of a row: a badge with the deck letter in the deck colour, and a thin bar in
/// that colour at the left edge of the row. A row on no deck keeps the room empty, so the
/// columns stay aligned. `hint` is the tooltip.
pub(crate) fn deck_mark(ui: &mut egui::Ui, deck: Option<usize>, hint: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let side = egui::Vec2::splat(size::DECK_MARK);
    let (rect, resp) = ui.allocate_exact_size(side, Sense::hover());
    let Some(d) = deck else {
        return resp;
    };
    let letter = ['A', 'B', 'C', 'D'].get(d).copied().unwrap_or('?');
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, format!("Deck {letter}")));
    paint_deck_mark(ui.painter(), rect, ui.max_rect().left(), d, &t);
    resp.on_hover_text(hint)
}

/// Paints a deck badge in `rect`, and the deck bar at `bar_x`.
pub(crate) fn paint_deck_mark(
    p: &egui::Painter,
    rect: egui::Rect,
    bar_x: f32,
    d: usize,
    t: &Tokens,
) {
    let colour = Tokens::deck(d);
    let bar = egui::Rect::from_x_y_ranges(
        bar_x..=bar_x + size::DECK_BAR,
        rect.center().y - size::ROW / 2.0..=rect.center().y + size::ROW / 2.0,
    );
    p.rect_filled(bar, 0.0, colour);
    p.rect_filled(rect, t.radius_sm, colour);
    let letter = ['A', 'B', 'C', 'D'].get(d).copied().unwrap_or('?');
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        letter,
        theme::mono(theme::CAPTION),
        t.deck_ink(d),
    );
}

/// The tooltip of a deck mark: the deck and when its item starts.
pub(crate) fn deck_hint(deck: usize, start: &jiff::Zoned) -> String {
    let letter = ['A', 'B', 'C', 'D'].get(deck).copied().unwrap_or('?');
    format!(
        "Loaded on deck {letter} · starts {}",
        start.strftime("%H:%M:%S")
    )
}
