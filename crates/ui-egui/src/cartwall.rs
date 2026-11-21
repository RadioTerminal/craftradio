//! The cartwall: a 4×4 grid of carts and a button that stops them all.
//! The preview plays no audio. A pressed cart only flashes, and the caption says so.

use egui::{Align, Layout, RichText, Stroke};

use crate::theme::{self, Tokens, size, space};
use crate::view::StationView;

const NO_AUDIO: &str = "preview — no audio";

/// The flash state of each cart.
pub(crate) struct State {
    /// The egui time at which each cart stops its flash.
    flash_until: Vec<f64>,
    last: Option<usize>,
}

impl State {
    pub fn new(carts: usize) -> Self {
        Self {
            flash_until: vec![f64::NEG_INFINITY; carts],
            last: None,
        }
    }
}

pub(crate) fn show(ui: &mut egui::Ui, view: &StationView, state: &mut State, t: &Tokens) {
    state
        .flash_until
        .resize(view.carts.len(), f64::NEG_INFINITY);
    let now = ui.input(|i| i.time);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Cartwall")
                        .font(theme::text(theme::CARD))
                        .strong(),
                );
                let flashing = state
                    .last
                    .filter(|&i| state.flash_until.get(i).is_some_and(|u| *u > now))
                    .and_then(|i| view.carts.get(i));
                let caption = match flashing {
                    Some(name) => format!("{name} · {NO_AUDIO}"),
                    None => NO_AUDIO.to_owned(),
                };
                ui.label(
                    RichText::new(caption)
                        .font(theme::text(theme::CAPTION))
                        .color(t.text_muted),
                );
            });
            ui.add_space(space::XS);
            egui::Grid::new("cartwall_grid")
                .spacing(egui::vec2(space::S, space::S))
                .show(ui, |ui| {
                    for (i, name) in view.carts.iter().enumerate() {
                        let until = state.flash_until[i];
                        let lit = until > now;
                        let (fill, ink, edge) = if lit {
                            (t.flash, t.flash_ink, t.flash)
                        } else {
                            (t.field, t.text, t.card_border)
                        };
                        let text = RichText::new(name)
                            .font(theme::text(theme::LABEL))
                            .color(ink);
                        let button = egui::Button::new(text)
                            .fill(fill)
                            .stroke(Stroke::new(size::HAIRLINE, edge))
                            .min_size(egui::vec2(size::CART_W, size::CART_H));
                        let resp = ui.add(button).on_hover_text(NO_AUDIO);
                        if resp.clicked() {
                            state.flash_until[i] = now + size::CART_FLASH_S;
                            state.last = Some(i);
                        }
                        if lit {
                            let left = (until - now).max(0.0);
                            ui.ctx().request_repaint_after_secs(left as f32);
                        }
                        if (i + 1) % size::CART_COLS == 0 {
                            ui.end_row();
                        }
                    }
                });
        });
        ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
            let text = RichText::new("■ stop all carts")
                .font(theme::text(theme::BODY))
                .color(t.text);
            let stop = egui::Button::new(text)
                .fill(t.danger_fill)
                .stroke(Stroke::new(size::HAIRLINE, t.danger))
                .min_size(egui::vec2(size::STOP_W, size::CONTROL_LG));
            if ui.add(stop).on_hover_text(NO_AUDIO).clicked() {
                state.flash_until.fill(f64::NEG_INFINITY);
                state.last = None;
            }
        });
    });
}
