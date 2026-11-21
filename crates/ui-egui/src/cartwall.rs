//! The cartwall: pages of carts in a grid, and a button that stops them all.
//! The preview plays no audio. A pressed cart only flashes, and the caption says so.
//! Pages, carts, ducking, mode and colour live in the view source.

use egui::{Align, DragValue, Layout, RichText, Stroke};

use crate::app::{Action, PasteTo};
use crate::context_menu::{self, PASTE, check_item, menu_item, menu_item_if};
use crate::library::LibraryDrag;
use crate::parts;
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{CartChange, CartMode, CartSlot, DropTarget, Edit, StationView};
use crate::windows::ToolWindow;

/// The ducking choices in the menu, in dB. Zero means off.
const DUCKS: [f32; 4] = [0.0, -6.0, -12.0, -18.0];
/// The deepest ducking that the settings allow, in dB.
const MAX_DUCK_DB: f32 = -40.0;
/// Ducking rows in the settings: carts per row.
const DUCK_COLS: usize = 2;
/// The largest grid side in the settings.
const MAX_GRID: u32 = 8;

const NO_AUDIO: &str = "preview — no audio";

/// The view state of the window: the page on show, flashes and draft page names.
#[derive(Default)]
pub(crate) struct State {
    page: usize,
    /// The egui time at which each cart stops its flash.
    flash_until: Vec<f64>,
    last: Option<usize>,
    settings: bool,
    /// Page names while a person edits them.
    names: Vec<String>,
    /// True when the clipboard holds items, for this frame.
    has_clip: bool,
}

fn set_cart(cart: usize, change: CartChange) -> Action {
    Action::EditQuiet(Edit::SetCart { cart, change })
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    has_clip: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let wall = &view.cartwall;
    state.has_clip = has_clip;
    state
        .flash_until
        .resize(view.carts.len(), f64::NEG_INFINITY);
    state.page = state.page.min(wall.pages.len().saturating_sub(1));
    let cols = parts::u32(view, parts::CARTWALL, "cols", size::CART_COLS as u32).max(1) as usize;
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
        for page in (0..wall.pages.len()).rev() {
            if ui
                .selectable_label(state.page == page, &wall.pages[page])
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
    if open && !state.settings {
        state.names.clone_from(&wall.pages);
    }
    state.settings = open;
    if state.settings {
        settings::section(ui, "Cartwall", t, |ui| {
            cart_settings(ui, view, state, actions, t)
        });
    }
    ui.add_space(space::S);

    let first = state.page * wall.per_page;
    egui::Grid::new("cartwall_grid")
        .spacing(egui::vec2(space::S, space::S))
        .show(ui, |ui| {
            for n in 0..wall.per_page {
                let i = first + n;
                if let Some(name) = view.carts.get(i) {
                    let slot = wall.slots.get(i).copied().unwrap_or(DEFAULT_SLOT);
                    cart(ui, i, name, slot, state, actions, t);
                }
                if (n + 1) % cols == 0 {
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

/// The settings of a cart that the source does not know.
const DEFAULT_SLOT: CartSlot = CartSlot {
    duck_db: 0.0,
    mode: CartMode::OneShot,
    colour: None,
};

fn cart(
    ui: &mut egui::Ui,
    i: usize,
    name: &str,
    slot: CartSlot,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let now = ui.input(|i| i.time);
    let empty = name.is_empty();
    let name = if empty { "empty" } else { name };
    let until = state.flash_until[i];
    let lit = until > now;
    let (fill, ink, edge) = if lit {
        (t.flash, t.flash_ink, t.flash)
    } else {
        (
            t.field,
            if empty { t.text_muted } else { t.text },
            t.card_border,
        )
    };
    let text = RichText::new(name)
        .font(theme::text(theme::LABEL))
        .color(ink);
    let button = egui::Button::new(text)
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, edge))
        .min_size(egui::vec2(size::CART_W, size::CART_H));
    let resp = ui.add(button);
    let colour = slot.colour.and_then(|c| theme::CART_COLOURS.get(c));
    if let Some(c) = colour {
        let mut bar = resp.rect;
        bar.set_width(size::CART_BAR);
        ui.painter()
            .rect_filled(bar, t.radius_sm, Tokens::playlist(c.1));
    }
    let has_clip = state.has_clip;
    resp.context_menu(|ui| cart_menu(ui, i, slot, has_clip, actions));
    if let Some(drag) = resp.dnd_release_payload::<LibraryDrag>() {
        actions.push(Action::Drop {
            item: drag.0,
            target: DropTarget::Cart(i),
        });
    }
    if resp.dnd_hover_payload::<LibraryDrag>().is_some() {
        let edge = Stroke::new(size::HAIRLINE * 2.0, t.accent);
        ui.painter()
            .rect_stroke(resp.rect, t.radius, edge, egui::StrokeKind::Outside);
    }
    let colour_name = colour.map_or("no colour", |c| c.0);
    let hint = format!(
        "{NO_AUDIO} · {} · {colour_name} · ducking {:.0} dB",
        slot.mode.label(),
        slot.duck_db
    );
    if resp.on_hover_text(hint).clicked() {
        state.flash_until[i] = now + size::CART_FLASH_S;
        state.last = Some(i);
    }
    if lit {
        ui.ctx()
            .request_repaint_after_secs((until - now).max(0.0) as f32);
    }
}

fn cart_menu(
    ui: &mut egui::Ui,
    i: usize,
    slot: CartSlot,
    has_clip: bool,
    actions: &mut Vec<Action>,
) {
    for label in ["Play", "Stop"] {
        if menu_item(ui, label, None) {
            actions.push(Action::Toast(context_menu::NEEDS_ENGINE.to_owned()));
        }
    }
    if menu_item(ui, "Assign from library…", None) {
        actions.push(Action::OpenWindow(ToolWindow::Library));
        actions.push(Action::Toast(
            "drag a title from the Library onto the cart".to_owned(),
        ));
    }
    if menu_item_if(ui, has_clip, "Paste", Some(PASTE)) {
        actions.push(Action::Paste(PasteTo::Cart(i)));
    }
    if menu_item(ui, "Clear", None) {
        actions.push(Action::Edit(Edit::AssignCart {
            cart: i,
            item: None,
        }));
    }
    ui.separator();
    ui.menu_button("Colour", |ui| {
        if check_item(ui, slot.colour.is_none(), "None") {
            actions.push(set_cart(i, CartChange::Colour(None)));
        }
        for (k, (name, _)) in theme::CART_COLOURS.iter().enumerate() {
            if check_item(ui, slot.colour == Some(k), name) {
                actions.push(set_cart(i, CartChange::Colour(Some(k))));
            }
        }
    });
    ui.menu_button("Ducking", |ui| {
        for db in DUCKS {
            let label = if db == 0.0 {
                "Off".to_owned()
            } else {
                format!("{db:.0} dB")
            };
            if check_item(ui, (slot.duck_db - db).abs() < f32::EPSILON, &label) {
                actions.push(set_cart(i, CartChange::Duck(db)));
            }
        }
    });
    ui.menu_button("Mode", |ui| {
        for mode in CartMode::ALL {
            if check_item(ui, slot.mode == mode, mode.label()) {
                actions.push(set_cart(i, CartChange::Mode(mode)));
            }
        }
    });
}

fn cart_settings(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let wall = &view.cartwall;
    let default_side = size::CART_COLS as u32;
    let old_cols = parts::u32(view, parts::CARTWALL, "cols", default_side);
    let old_rows = parts::u32(view, parts::CARTWALL, "rows", default_side);
    let (mut cols, mut rows) = (old_cols, old_rows);
    state.names.resize(wall.pages.len(), String::new());
    egui::Grid::new("cart_settings")
        .spacing(egui::vec2(space::M, space::S))
        .show(ui, |ui| {
            settings::label(ui, "Grid", t);
            ui.horizontal(|ui| {
                ui.add(
                    DragValue::new(&mut cols)
                        .range(1..=MAX_GRID)
                        .suffix(" columns"),
                );
                ui.add(
                    DragValue::new(&mut rows)
                        .range(1..=MAX_GRID)
                        .suffix(" rows"),
                );
            });
            ui.end_row();
            settings::label(ui, "Pages", t);
            ui.vertical(|ui| {
                for (page, name) in state.names.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let edit =
                            ui.add(egui::TextEdit::singleline(name).desired_width(size::COMBO_W));
                        let current = wall.pages.get(page).map_or("", String::as_str);
                        if edit.lost_focus() && name != current && !name.trim().is_empty() {
                            actions.push(Action::Edit(Edit::RenameCartPage {
                                page,
                                name: name.trim().to_owned(),
                            }));
                        }
                        if ui.small_button("Remove").clicked() {
                            actions.push(Action::Edit(Edit::RemoveCartPage { page }));
                        }
                    });
                }
                if ui.small_button("+ Add page").clicked() {
                    let name = format!("Page {}", wall.pages.len() + 1);
                    state.names.push(name.clone());
                    actions.push(Action::Edit(Edit::AddCartPage { name }));
                }
            });
            ui.end_row();
        });
    // Both sides go together, so the source sees one grid.
    if cols != old_cols || rows != old_rows {
        actions.push(parts::set(parts::CARTWALL, "cols", cols));
        actions.push(parts::set(parts::CARTWALL, "rows", rows));
    }
    ui.add_space(space::S);
    settings::label(
        ui,
        "Ducking per cart on this page (dB under the main output)",
        t,
    );
    let first = state.page * wall.per_page;
    egui::Grid::new("cart_ducking")
        .spacing(egui::vec2(space::M, space::XS))
        .show(ui, |ui| {
            let carts = view
                .carts
                .iter()
                .enumerate()
                .skip(first)
                .take(wall.per_page);
            for (n, (i, name)) in carts.enumerate() {
                let shown = if name.is_empty() { "empty" } else { name };
                ui.label(RichText::new(shown).font(theme::text(theme::LABEL)));
                let mut duck = wall.slots.get(i).map_or(0.0, |s| s.duck_db);
                let field = DragValue::new(&mut duck)
                    .range(MAX_DUCK_DB..=0.0)
                    .speed(0.5)
                    .suffix(" dB");
                if ui.add(field).changed() {
                    actions.push(set_cart(i, CartChange::Duck(duck)));
                }
                if (n + 1) % DUCK_COLS == 0 {
                    ui.end_row();
                }
            }
        });
}
