//! Shared controls that the theme alone cannot make.

use egui::{Align2, Sense, Stroke, StrokeKind, WidgetInfo, WidgetType};

use crate::icons::{self, Icon};
use crate::theme::{self, Tokens, size, space};

/// An icon button that keeps one exact size in every state.
// Hover, press and focus change only colours. The edge width never changes.
// The accessible name `name` is the tooltip's subject; the icon has no text of its own.
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    selected: bool,
    name: &str,
    hint: &str,
) -> egui::Response {
    icon_button_sized(ui, icon, selected, name, hint, size::ICON_BUTTON)
}

/// An icon button of a given square `side`, a control size from the theme.
pub(crate) fn icon_button_sized(
    ui: &mut egui::Ui,
    icon: Icon,
    selected: bool,
    name: &str,
    hint: &str,
    side: f32,
) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let side = egui::Vec2::splat(side);
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
        let font = icons::font(theme::ICON_MD);
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            icons::glyph(icon),
            font,
            ink,
        );
    }
    resp.on_hover_text(hint)
}

/// The armed state of a guarded button: how much of the time to confirm is left, the
/// accessible name the button takes ("Confirm: ..."), and the full sentence for its tooltip.
/// The sentence is never drawn as text in the layout: it opens as the tooltip at once.
#[derive(Clone, Copy)]
pub(crate) struct Armed<'a> {
    /// The part of the arm time that is left, from 1 down to 0.
    pub left: f32,
    pub name: &'a str,
    pub note: &'a str,
}

/// An icon button for a guarded action. While armed, it shows the danger colour and a ring,
/// and its tooltip opens with the full sentence. Its size never changes.
/// The ring shrinks as the time to press again runs out.
// The `side` is a control size from the theme.
pub(crate) fn guard_button_sized(
    ui: &mut egui::Ui,
    icon: Icon,
    name: &str,
    hint: &str,
    armed: Option<Armed<'_>>,
    side: f32,
) -> egui::Response {
    let Some(Armed { left, name, note }) = armed else {
        return icon_button_sized(ui, icon, false, name, hint, side);
    };
    let t = Tokens::get(ui.ctx());
    let side = egui::Vec2::splat(side);
    let (rect, resp) = ui.allocate_exact_size(side, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, true, name));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let stroke = Stroke::new(size::HAIRLINE, t.danger);
        p.rect(rect, t.radius, t.danger_fill, stroke, StrokeKind::Inside);
        let font = icons::font(theme::ICON_MD);
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            icons::glyph(icon),
            font,
            t.text,
        );
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
    resp.show_tooltip_text(note);
    resp
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

/// How a transport button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    /// The one main action: solid accent.
    Primary,
    /// An action beside it: a field with an edge.
    Secondary,
    /// A toggle that is on: the warning fill.
    On,
}

/// A button of the global transport: an icon and a word, at the hero height. When `compact` is
/// set it is the icon alone in a square, with the name and the tooltip unchanged. While
/// `armed` it turns to the danger colour, says "Confirm" in the room of its own word, shows the
/// countdown bar and opens its tooltip with the full sentence. Its size never changes.
pub(crate) fn transport_button(
    ui: &mut egui::Ui,
    icon: Icon,
    word: &str,
    look: Look,
    compact: bool,
    armed: Option<Armed<'_>>,
    hint: &str,
) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let h = size::CONTROL_HERO;
    let shown = if armed.is_some() { "Confirm" } else { word };
    let font = theme::text(theme::BODY);
    let icon_font = icons::font(theme::ICON_LG);
    let ink_probe = t.text;
    let label_w = if compact {
        0.0
    } else {
        // The widest of the two words, so arming never moves the buttons beside it.
        let width = |s: &str| {
            let galley = ui
                .painter()
                .layout_no_wrap(s.to_owned(), font.clone(), ink_probe);
            galley.size().x
        };
        width(word).max(width("Confirm"))
    };
    let gap = if compact { 0.0 } else { space::S };
    let icon_w = theme::ICON_LG;
    let w = if compact {
        h
    } else {
        icon_w + gap + label_w + space::L * 2.0
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), Sense::click());
    let name = armed.map_or(word, |a| a.name);
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, name));
    if ui.is_rect_visible(rect) {
        let (fill, edge, ink) = if armed.is_some() {
            (t.danger_fill, t.danger, t.text)
        } else {
            match look {
                Look::Primary => {
                    // Hover and press both lighten the fill.
                    let lit = resp.is_pointer_button_down_on() || resp.hovered();
                    let fill = if lit { t.accent_ink } else { t.accent };
                    (fill, t.accent, t.on_accent)
                }
                Look::Secondary | Look::On => {
                    let fill = if resp.is_pointer_button_down_on() {
                        t.pressed
                    } else if resp.hovered() {
                        t.hover
                    } else if look == Look::On {
                        t.warn_fill
                    } else {
                        t.field
                    };
                    let edge = if look == Look::On {
                        t.warn
                    } else {
                        t.field_border
                    };
                    (fill, edge, t.text)
                }
            }
        };
        let edge = if resp.has_focus() { t.accent_ink } else { edge };
        let p = ui.painter();
        p.rect(
            rect,
            t.radius,
            fill,
            Stroke::new(size::HAIRLINE, edge),
            StrokeKind::Inside,
        );
        let ink = if enabled { ink } else { t.text_muted };
        let glyph = icons::glyph(icon);
        if compact {
            p.text(rect.center(), Align2::CENTER_CENTER, glyph, icon_font, ink);
        } else {
            let left = rect.left() + space::L;
            let cy = rect.center().y;
            p.text(
                egui::pos2(left, cy),
                Align2::LEFT_CENTER,
                glyph,
                icon_font,
                ink,
            );
            let at = egui::pos2(left + icon_w + gap, cy);
            p.text(at, Align2::LEFT_CENTER, shown, font, ink);
        }
        if let Some(Armed { left, .. }) = armed {
            // The time left to press again: a bar along the bottom edge that runs out.
            let y = rect.bottom() - size::ON_AIR_EDGE;
            let x1 = rect.left() + rect.width() * left.clamp(0.0, 1.0);
            let bar = [egui::pos2(rect.left(), y), egui::pos2(x1, y)];
            p.line_segment(bar, Stroke::new(size::ON_AIR_EDGE, t.danger));
        }
    }
    if let Some(a) = armed {
        resp.show_tooltip_text(a.note);
        resp
    } else {
        resp.on_hover_text(hint)
    }
}
