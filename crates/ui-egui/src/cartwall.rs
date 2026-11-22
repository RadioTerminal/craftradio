//! The cartwall: pages of carts in a grid, and a button that stops them all.
//! The preview plays no audio. A pressed cart only flashes, and the caption says so.
//! Pages, carts, ducking, mode and colour live in the view source.

use crate::numbers;
use crate::widgets;
use egui::{DragValue, RichText, Stroke};

use crate::app::{Action, PasteTo};
use crate::context_menu::{PASTE, check_item, menu_item, menu_item_if};
use crate::icons::{self, Icon};
use crate::library::LibraryDrag;
use crate::parts;
use crate::settings;
use crate::shortcuts;
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

pub(crate) const NO_AUDIO: &str = "preview — no audio";

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
    /// The carts on a page, for this frame.
    per_page: usize,
    /// The source is the preview, for this frame.
    preview: bool,
}

impl State {
    /// Fire cart `n` of the page on show, as its key does. It returns the cart index and name.
    pub(crate) fn fire_on_page(
        &mut self,
        view: &StationView,
        n: usize,
        now: f64,
    ) -> Option<(usize, String)> {
        let wall = &view.cartwall;
        if n >= wall.per_page {
            return None;
        }
        let i = self.page.min(wall.pages.len().saturating_sub(1)) * wall.per_page + n;
        let name = view.carts.get(i).filter(|c| !c.is_empty())?;
        self.flash_until.resize(view.carts.len(), f64::NEG_INFINITY);
        if let Some(u) = self.flash_until.get_mut(i) {
            *u = now + size::CART_FLASH_S;
        }
        self.last = Some(i);
        Some((i, name.clone()))
    }

    /// Stop every cart, as Escape does.
    pub(crate) fn stop_all(&mut self) {
        self.flash_until.fill(f64::NEG_INFINITY);
        self.last = None;
    }
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
    state.per_page = wall.per_page;
    state.preview = view.preview;
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
    let caption = match (flashing, view.preview) {
        (Some(name), true) => format!("{name} · {NO_AUDIO}"),
        (None, true) => NO_AUDIO.to_owned(),
        (Some(name), false) => format!("{name} fired"),
        (None, false) => String::new(),
    };
    settings::header(ui, "Cartwall", &mut open, |ui| {
        if wall.pages.len() > size::PAGE_TABS_MAX {
            // Many pages: one picker instead of a row of tabs that runs out of the window.
            let current = wall.pages.get(state.page).cloned().unwrap_or_default();
            craft_ui::ComboBox::from_id_salt("cart_page")
                .width(size::COMBO_W)
                .truncate()
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (page, name) in wall.pages.iter().enumerate() {
                        widgets::selectable_value(ui, &mut state.page, page, name);
                    }
                });
        } else {
            for page in (0..wall.pages.len()).rev() {
                let name = &wall.pages[page];
                let on = state.page == page;
                let tab = egui::Button::new(name.as_str()).truncate();
                let r = crate::widgets::with_max_width(ui, size::PAGE_TAB_MAX_W, |ui| {
                    crate::widgets::steady(ui, on, tab)
                });
                if r.on_hover_text(name).clicked() {
                    state.page = page;
                }
            }
        }
        ui.add(
            egui::Label::new(
                RichText::new(caption)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            )
            .truncate(),
        );
    });
    if open && !state.settings {
        state.names.clone_from(&wall.pages);
    }
    state.settings = open;
    if state.settings {
        settings::section(ui, "Cartwall", view.preview, t, |ui| {
            cart_settings(ui, view, state, actions, t)
        });
    }
    ui.add_space(space::S);

    // The bar under the grid is as wide as the grid, so an auto-sized window fits the carts.
    let grid_w = cols as f32 * size::CART_W + cols.saturating_sub(1) as f32 * space::S;
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
    // A wide gap and a full-width bar: easy to find in a hurry, hard to hit by mistake.
    ui.add_space(space::XL);
    let text = RichText::new(icons::lead(Icon::Square, "Stop all carts"))
        .font(theme::text(theme::BODY))
        .color(t.text);
    let stop = egui::Button::new(text)
        .fill(t.danger_fill)
        .stroke(Stroke::new(size::HAIRLINE, t.danger))
        .min_size(egui::vec2(
            grid_w.min(ui.available_width()),
            size::CONTROL_MD,
        ));
    let keys = shortcuts::label(ui.ctx(), &shortcuts::STOP_CARTS);
    let audio = if view.preview {
        format!(" · {NO_AUDIO}")
    } else {
        String::new()
    };
    let hint = format!("Stop every playing cart now ({keys}){audio}");
    if ui.add(stop).on_hover_text(hint).clicked() {
        state.stop_all();
        actions.push(Action::EditQuiet(Edit::StopCarts));
    }
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
    // The place on the page, for the key that fires the cart.
    let n = i % state.per_page.max(1);
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
    // Every cart has one size. A long name truncates, and the tooltip shows all of it.
    let button = egui::Button::new(text)
        .truncate()
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, edge));
    let resp = ui.add_sized(egui::vec2(size::CART_W, size::CART_H), button);
    let colour = slot.colour.and_then(|c| theme::PALETTE.get(c));
    if let Some(c) = colour {
        let mut bar = resp.rect;
        bar.set_width(size::CART_BAR);
        ui.painter()
            .rect_filled(bar, t.radius_sm, Tokens::playlist(c.1));
    }
    if let Some(key) = shortcuts::cart_label(n) {
        // The key that fires the cart, in the top-right corner.
        let at = resp.rect.right_top() + egui::vec2(-size::KEY_INSET, size::KEY_INSET);
        let ink = if lit { t.flash_ink } else { t.text_muted };
        ui.painter().text(
            at,
            egui::Align2::RIGHT_TOP,
            key,
            theme::mono(theme::CAPTION),
            ink,
        );
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
    // The tooltip text is made only while the tooltip shows.
    let hint = |ui: &mut egui::Ui| {
        let key = shortcuts::cart_label(n).map_or(String::new(), |k| format!(" ({k})"));
        let audio = if state.preview {
            format!(" · {NO_AUDIO}")
        } else {
            String::new()
        };
        ui.label(name);
        ui.label(format!(
            "Fire the cart{key}{audio} · {} · {colour_name} · ducking {:.0} dB",
            slot.mode.label(),
            slot.duck_db
        ));
    };
    if resp.on_hover_ui(hint).clicked() {
        state.flash_until[i] = now + size::CART_FLASH_S;
        state.last = Some(i);
        if !empty {
            actions.push(Action::EditQuiet(Edit::FireCart { cart: i }));
        }
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
    if menu_item(ui, "Play", None) {
        actions.push(Action::EditQuiet(Edit::FireCart { cart: i }));
    }
    if menu_item(ui, "Stop", None) {
        actions.push(Action::EditQuiet(Edit::StopCarts));
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
        for (k, (name, _)) in theme::PALETTE.iter().enumerate() {
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
                ui.add(numbers::field(
                    ui,
                    DragValue::new(&mut cols)
                        .range(1..=MAX_GRID)
                        .suffix(" columns"),
                ));
                ui.add(numbers::field(
                    ui,
                    DragValue::new(&mut rows)
                        .range(1..=MAX_GRID)
                        .suffix(" rows"),
                ));
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
                if ui
                    .small_button(icons::lead(Icon::Plus, "Add page"))
                    .clicked()
                {
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
                let field = numbers::field(
                    ui,
                    DragValue::new(&mut duck)
                        .range(MAX_DUCK_DB..=0.0)
                        .speed(0.5)
                        .suffix(" dB"),
                );
                if ui.add(field).changed() {
                    actions.push(set_cart(i, CartChange::Duck(duck)));
                }
                if (n + 1) % DUCK_COLS == 0 {
                    ui.end_row();
                }
            }
        });
}
