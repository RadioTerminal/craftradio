//! The cartwall: pages of carts in a grid, and a button that stops them all.
//! The preview plays no audio. A pressed cart only flashes, and the caption says so.

use egui::{Align, DragValue, Layout, RichText, Stroke};

use crate::app::Action;
use crate::library::LibraryDrag;
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{DropTarget, StationView};

const NO_AUDIO: &str = "preview — no audio";
const DEFAULT_DUCK_DB: f32 = -12.0;

/// The cartwall state. All of it is local to the window.
pub(crate) struct State {
    page: usize,
    cols: usize,
    rows: usize,
    pages: Vec<String>,
    /// How much each cart lowers the main output while it plays, in dB.
    duck_db: Vec<f32>,
    /// The egui time at which each cart stops its flash.
    flash_until: Vec<f64>,
    last: Option<usize>,
    settings: bool,
}

impl State {
    pub fn new(carts: usize) -> Self {
        let per_page = size::CART_COLS * size::CART_COLS;
        let pages = carts.div_ceil(per_page).max(1);
        Self {
            page: 0,
            cols: size::CART_COLS,
            rows: size::CART_COLS,
            pages: (1..=pages).map(|n| format!("Page {n}")).collect(),
            duck_db: vec![DEFAULT_DUCK_DB; carts],
            flash_until: vec![f64::NEG_INFINITY; carts],
            last: None,
            settings: false,
        }
    }

    fn per_page(&self) -> usize {
        (self.cols * self.rows).max(1)
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    state
        .flash_until
        .resize(view.carts.len(), f64::NEG_INFINITY);
    state.duck_db.resize(view.carts.len(), DEFAULT_DUCK_DB);
    if state.pages.is_empty() {
        state.pages.push("Page 1".to_owned());
    }
    state.page = state.page.min(state.pages.len() - 1);
    let now = ui.input(|i| i.time);

    let mut open = state.settings;
    let flashing = state
        .last
        .filter(|&i| state.flash_until.get(i).is_some_and(|u| *u > now))
        .and_then(|i| view.carts.get(i));
    let caption = match flashing {
        Some(name) => format!("{name} · {NO_AUDIO}"),
        None => NO_AUDIO.to_owned(),
    };
    settings::header(ui, "Cartwall", &mut open, |ui| {
        for page in (0..state.pages.len()).rev() {
            if ui
                .selectable_label(state.page == page, &state.pages[page])
                .clicked()
            {
                state.page = page;
            }
        }
        ui.label(
            RichText::new(caption)
                .font(theme::text(theme::CAPTION))
                .color(t.text_muted),
        );
    });
    state.settings = open;
    if state.settings {
        settings::section(ui, "Cartwall", t, |ui| cart_settings(ui, view, state, t));
    }
    ui.add_space(space::S);

    let first = state.page * state.per_page();
    egui::Grid::new("cartwall_grid")
        .spacing(egui::vec2(space::S, space::S))
        .show(ui, |ui| {
            for n in 0..state.per_page() {
                let i = first + n;
                match view.carts.get(i) {
                    Some(name) => cart(ui, i, name, state, now, actions, t),
                    None => {
                        let empty = egui::Button::new(RichText::new("empty").color(t.text_muted))
                            .min_size(egui::vec2(size::CART_W, size::CART_H));
                        ui.add_enabled(false, empty);
                    }
                }
                if (n + 1) % state.cols == 0 {
                    ui.end_row();
                }
            }
        });
    ui.add_space(space::S);
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        let text = RichText::new("■ stop all carts")
            .font(theme::text(theme::BODY))
            .color(t.text);
        let stop = egui::Button::new(text)
            .fill(t.danger_fill)
            .stroke(Stroke::new(size::HAIRLINE, t.danger))
            .min_size(egui::vec2(size::STOP_W, size::CONTROL_MD));
        if ui.add(stop).on_hover_text(NO_AUDIO).clicked() {
            state.flash_until.fill(f64::NEG_INFINITY);
            state.last = None;
        }
    });
}

fn cart(
    ui: &mut egui::Ui,
    i: usize,
    name: &str,
    state: &mut State,
    now: f64,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
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
    let resp = ui.add(button);
    if let Some(drag) = resp.dnd_release_payload::<LibraryDrag>() {
        actions.push(Action::Drop {
            item: drag.0,
            target: DropTarget::Cart(i),
        });
    }
    if resp.dnd_hover_payload::<LibraryDrag>().is_some() {
        ui.painter().rect_stroke(
            resp.rect,
            t.radius,
            Stroke::new(size::HAIRLINE * 2.0, t.accent),
            egui::StrokeKind::Outside,
        );
    }
    if resp.on_hover_text(NO_AUDIO).clicked() {
        state.flash_until[i] = now + size::CART_FLASH_S;
        state.last = Some(i);
    }
    if lit {
        ui.ctx()
            .request_repaint_after_secs((until - now).max(0.0) as f32);
    }
}

fn cart_settings(ui: &mut egui::Ui, view: &StationView, state: &mut State, t: &Tokens) {
    egui::Grid::new("cart_settings")
        .spacing(egui::vec2(space::M, space::S))
        .show(ui, |ui| {
            settings::label(ui, "Grid", t);
            ui.horizontal(|ui| {
                ui.add(
                    DragValue::new(&mut state.cols)
                        .range(1..=8)
                        .suffix(" columns"),
                );
                ui.add(DragValue::new(&mut state.rows).range(1..=8).suffix(" rows"));
            });
            ui.end_row();
            settings::label(ui, "Pages", t);
            ui.vertical(|ui| {
                let mut remove = None;
                for (i, name) in state.pages.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(name).desired_width(size::COMBO_W));
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove.filter(|_| state.pages.len() > 1) {
                    state.pages.remove(i);
                }
                if ui.small_button("+ Add page").clicked() {
                    let n = state.pages.len() + 1;
                    state.pages.push(format!("Page {n}"));
                }
            });
            ui.end_row();
        });
    ui.add_space(space::S);
    settings::label(
        ui,
        "Ducking per cart on this page (dB under the main output)",
        t,
    );
    let first = state.page * state.per_page();
    egui::Grid::new("cart_ducking")
        .spacing(egui::vec2(space::M, space::XS))
        .show(ui, |ui| {
            let carts = view
                .carts
                .iter()
                .enumerate()
                .skip(first)
                .take(state.per_page());
            for (n, (i, name)) in carts.enumerate() {
                ui.label(RichText::new(name).font(theme::text(theme::LABEL)));
                ui.add(
                    DragValue::new(&mut state.duck_db[i])
                        .range(-40.0..=0.0)
                        .speed(0.5)
                        .suffix(" dB"),
                );
                if (n + 1) % 2 == 0 {
                    ui.end_row();
                }
            }
        });
}
