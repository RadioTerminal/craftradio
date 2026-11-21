//! The top bar: four deck cards, the mode and the station clock.

use egui::{Align, Layout, RichText, Sense, Stroke};

use crate::app::{hms, mmss};
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DeckState, DeckView, StationView};

pub(crate) fn show(ui: &mut egui::Ui, view: &StationView, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let gaps = space::M * view.decks.len() as f32;
        let free = ui.available_width() - size::CLOCK_W - gaps;
        let card_w = (free / view.decks.len() as f32).max(size::DECK_MIN_W);
        for deck in &view.decks {
            deck_card(ui, deck, card_w, t);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            clock(ui, view, t)
        });
    });
}

fn deck_card(ui: &mut egui::Ui, deck: &DeckView, width: f32, t: &Tokens) {
    let playing = deck.state == DeckState::Playing;
    let edge = if playing { t.on_air } else { t.card_border };
    let frame = theme::card_frame(t)
        .fill(if playing { t.on_air_bg } else { t.field })
        .stroke(Stroke::new(size::HAIRLINE, edge));
    let inner = width - frame.total_margin().sum().x - size::HAIRLINE * 2.0;
    frame.show(ui, |ui| {
        ui.set_width(inner);
        ui.vertical(|ui| deck_body(ui, deck, playing, t));
    });
}

fn deck_body(ui: &mut egui::Ui, deck: &DeckView, playing: bool, t: &Tokens) {
    {
        ui.spacing_mut().item_spacing.y = space::XS;
        ui.horizontal(|ui| {
            badge(ui, deck.label, playing, t);
            let (word, colour) = match deck.state {
                DeckState::Playing => ("▶ ON AIR", t.on_air),
                DeckState::Ready => ("⏸ READY", t.text_muted),
                DeckState::Empty => ("EMPTY", t.text_muted),
            };
            ui.label(
                RichText::new(word)
                    .font(theme::text(theme::LABEL))
                    .strong()
                    .color(colour),
            );
            if playing {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let left = deck.len.saturating_sub(deck.elapsed);
                    let text = format!("−{}", mmss(left));
                    ui.label(
                        RichText::new(text)
                            .font(theme::mono(theme::SMALL))
                            .color(t.text),
                    );
                });
            }
        });
        let title = if deck.title.is_empty() {
            "—"
        } else {
            deck.title.as_str()
        };
        ui.add(egui::Label::new(RichText::new(title).font(theme::text(theme::BODY))).truncate());
        progress(ui, deck, t);
        let times = match deck.state {
            DeckState::Playing => format!("{} / {}", mmss(deck.elapsed), mmss(deck.len)),
            DeckState::Ready => format!("ready · {}", mmss(deck.len)),
            DeckState::Empty => "no item".to_owned(),
        };
        ui.label(
            RichText::new(times)
                .font(theme::mono(theme::CODE))
                .color(t.text_muted),
        );
    }
}

fn badge(ui: &mut egui::Ui, label: char, playing: bool, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size::BADGE), Sense::hover());
    let (fill, ink) = if playing {
        (t.on_air, t.canvas)
    } else {
        (t.hover, t.text)
    };
    ui.painter().rect_filled(rect, t.radius, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        theme::mono(theme::CARD),
        ink,
    );
}

fn progress(ui: &mut egui::Ui, deck: &DeckView, t: &Tokens) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, size::PROGRESS_H), Sense::hover());
    ui.painter().rect_filled(rect, t.radius_sm, t.canvas);
    let len = deck.len.as_secs_f32();
    if len > 0.0 {
        let frac = (deck.elapsed.as_secs_f32() / len).clamp(0.0, 1.0);
        let mut done = rect;
        done.set_width(rect.width() * frac);
        let colour = if deck.state == DeckState::Playing {
            t.on_air
        } else {
            t.field_border
        };
        ui.painter().rect_filled(done, t.radius_sm, colour);
    }
}

fn clock(ui: &mut egui::Ui, view: &StationView, t: &Tokens) {
    ui.vertical(|ui| {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(t.selection)
                    .stroke(Stroke::new(size::HAIRLINE, t.accent))
                    .corner_radius(t.radius_lg)
                    .inner_margin(pad::CHIP)
                    .show(ui, |ui| {
                        let mode = RichText::new(&view.mode).font(theme::mono(theme::LABEL));
                        ui.label(mode.strong().color(t.accent_ink));
                    });
                ui.label(
                    RichText::new(hms(&view.now))
                        .font(theme::mono(theme::TITLE))
                        .strong(),
                );
            });
            let date = view.now.strftime("%a %-d %b %Y").to_string();
            ui.label(
                RichText::new(date)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        });
    });
}
