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
use crate::preferences::Page;
use crate::settings;
use crate::shortcuts;
use crate::theme::{self, Tokens, size, space};
use crate::view::{CartChange, CartMode, CartSlot, DropTarget, Edit, StationView};
use crate::windows::ToolWindow;

/// The cart modes are kept, but the playout does not play them yet (roadmap M6). Their
/// controls say so, so a choice does not look like it works (AGENTS.md rule 7).
const NOT_PLAYED_YET: &str = "Saved, but the playout does not apply this yet";

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
    /// True while the Cartwall page of Preferences edits `names`.
    names_drafted: bool,
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
    let mut stop_pressed = false;
    settings::header_to(ui, "Cartwall", Page::Cartwall, actions, |ui| {
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
        // Stop all carts, before the pages: easy to find in a hurry, in the danger colour.
        let text = RichText::new(icons::lead(Icon::Square, "Stop all carts"))
            .font(theme::text(theme::LABEL))
            .color(t.text);
        let stop = egui::Button::new(text)
            .fill(t.danger_fill)
            .stroke(Stroke::new(size::HAIRLINE, t.danger))
            .min_size(egui::vec2(size::STOP_W, size::CONTROL_SM));
        let keys = shortcuts::label(ui.ctx(), shortcuts::STOP_CARTS);
        let audio = if view.preview {
            format!(" · {NO_AUDIO}")
        } else {
            String::new()
        };
        let hint = format!("Stop every playing cart now ({keys}){audio}");
        if ui.add(stop).on_hover_text(hint).clicked() {
            stop_pressed = true;
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
    if stop_pressed {
        state.stop_all();
        actions.push(Action::EditQuiet(Edit::StopCarts));
    }
    ui.add_space(space::S);

    // Tiles share the width: wider in a wide window, never under the least tile.
    let gaps = cols.saturating_sub(1) as f32 * space::S;
    let tile_w = ((ui.available_width() - gaps) / cols as f32).max(size::CART_W);
    let tile = egui::vec2(tile_w, (tile_w * size::CART_ASPECT).max(size::CART_H));
    let first = state.page * wall.per_page;
    egui::Grid::new("cartwall_grid")
        .spacing(egui::vec2(space::S, space::S))
        .show(ui, |ui| {
            for n in 0..wall.per_page {
                let i = first + n;
                if let Some(name) = view.carts.get(i) {
                    let slot = wall.slots.get(i).copied().unwrap_or(DEFAULT_SLOT);
                    let c = Tile {
                        i,
                        name,
                        slot,
                        size: tile,
                    };
                    cart(ui, c, state, actions, t);
                }
                if (n + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
}

/// The settings of a cart that the source does not know.
const DEFAULT_SLOT: CartSlot = CartSlot {
    duck_db: 0.0,
    mode: CartMode::OneShot,
    colour: None,
};

/// A cart to draw: its index, name and settings, and the size of its tile.
struct Tile<'a> {
    i: usize,
    name: &'a str,
    slot: CartSlot,
    size: egui::Vec2,
}

/// One cart: a tile in its colour with the name in the middle, the mode at the top left, a
/// menu at the top right, and the key that fires it and its mode along the bottom. An empty
/// cart is a muted tile that asks for a title.
fn cart(
    ui: &mut egui::Ui,
    tile: Tile<'_>,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let Tile {
        i,
        name,
        slot,
        size: tile,
    } = tile;
    let now = ui.input(|i| i.time);
    // The place on the page, for the key that fires the cart.
    let n = i % state.per_page.max(1);
    let empty = name.is_empty();
    let until = state.flash_until[i];
    let lit = until > now;
    let colour = slot.colour.and_then(|c| theme::PALETTE.get(c));
    let fill = match (lit, empty, colour) {
        (true, _, _) => t.flash,
        (false, true, _) => t.card,
        (false, false, Some(c)) => Tokens::playlist(c.1),
        (false, false, None) => t.field,
    };
    // Dark or light ink, whichever reads on the tile.
    let ink = if lit {
        t.flash_ink
    } else if empty {
        t.text_muted
    } else if theme::contrast(fill, t.text) >= theme::contrast(fill, t.canvas) {
        t.text
    } else {
        t.canvas
    };
    let (rect, resp) = ui.allocate_exact_size(tile, egui::Sense::click());
    let label = if empty { "Empty cart" } else { name };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let edge = if resp.has_focus() {
            Stroke::new(size::HAIRLINE * 2.0, t.accent_ink)
        } else if resp.hovered() {
            Stroke::new(size::HAIRLINE, t.field_border)
        } else {
            Stroke::new(size::HAIRLINE, t.card_border)
        };
        p.rect(rect, t.radius_lg, fill, edge, egui::StrokeKind::Inside);
        let inner = rect.shrink(space::S);
        // The mode, top left.
        let mode = match slot.mode {
            CartMode::OneShot => Icon::Play,
            CartMode::Loop => Icon::Repeat,
            CartMode::Restart => Icon::RotateCcw,
        };
        p.text(
            inner.left_top(),
            egui::Align2::LEFT_TOP,
            icons::glyph(mode),
            icons::font(theme::ICON_SM),
            ink.gamma_multiply(size::CART_DIM),
        );
        // The name, in the middle, on up to two lines.
        if empty {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drag audio here",
                theme::text(theme::CAPTION),
                ink,
            );
        } else {
            let job = egui::text::LayoutJob {
                wrap: egui::text::TextWrapping {
                    max_width: inner.width(),
                    max_rows: 2,
                    break_anywhere: false,
                    overflow_character: Some('…'),
                },
                halign: egui::Align::Center,
                ..egui::text::LayoutJob::single_section(
                    name.to_owned(),
                    egui::TextFormat::simple(theme::text(theme::LEAD), ink),
                )
            };
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let at = egui::pos2(rect.center().x, rect.center().y - galley.size().y / 2.0);
            p.galley(at, galley, ink);
        }
        // The key and the mode along the bottom.
        if let Some(key) = shortcuts::cart_label(ui.ctx(), n) {
            p.text(
                inner.left_bottom(),
                egui::Align2::LEFT_BOTTOM,
                key,
                theme::mono(theme::CAPTION),
                ink,
            );
        }
        if !empty {
            let word = if slot.duck_db < 0.0 {
                format!("{} · {:.0} dB", slot.mode.label(), slot.duck_db)
            } else {
                slot.mode.label().to_owned()
            };
            p.text(
                inner.right_bottom(),
                egui::Align2::RIGHT_BOTTOM,
                word,
                theme::text(theme::CAPTION),
                ink.gamma_multiply(size::CART_DIM),
            );
        }
    }
    // The menu, top right: the same as a right-click.
    let more = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - size::CART_MENU - space::XS,
            rect.top() + space::XS,
        ),
        egui::Vec2::splat(size::CART_MENU),
    );
    let more_resp = ui.interact(
        more,
        ui.scope_id().with(("cart_more", i)),
        egui::Sense::click(),
    );
    more_resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{label} menu"))
    });
    if ui.is_rect_visible(more) {
        if more_resp.hovered() {
            ui.painter()
                .rect_filled(more, t.radius_sm, ink.gamma_multiply(size::CART_HOVER));
        }
        ui.painter().text(
            more.center(),
            egui::Align2::CENTER_CENTER,
            icons::glyph(Icon::Ellipsis),
            icons::font(theme::ICON_SM),
            ink,
        );
    }
    let has_clip = state.has_clip;
    egui::Popup::menu(&more_resp).show(|ui| cart_menu(ui, i, slot, has_clip, actions));
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
            .rect_stroke(rect, t.radius_lg, edge, egui::StrokeKind::Outside);
    }
    let colour_name = colour.map_or("no colour", |c| c.0);
    // The tooltip text is made only while the tooltip shows.
    let hint = |ui: &mut egui::Ui| {
        let key = shortcuts::cart_label(ui.ctx(), n).map_or(String::new(), |k| format!(" ({k})"));
        let audio = if state.preview {
            format!(" · {NO_AUDIO}")
        } else {
            String::new()
        };
        ui.label(label);
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
        not_played_yet(ui);
        for mode in CartMode::ALL {
            if check_item(ui, slot.mode == mode, mode.label()) {
                actions.push(set_cart(i, CartChange::Mode(mode)));
            }
        }
    });
}

/// The note on the mode controls: kept, not played yet.
fn not_played_yet(ui: &mut egui::Ui) {
    ui.label(
        RichText::new(NOT_PLAYED_YET)
            .font(theme::text(theme::CAPTION))
            .weak(),
    );
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
    settings::section(ui, "Layout", t, |ui| {
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
                            let edit = ui
                                .add(egui::TextEdit::singleline(name).desired_width(size::COMBO_W));
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
    });
    // Both sides go together, so the source sees one grid.
    if cols != old_cols || rows != old_rows {
        actions.push(parts::set(parts::CARTWALL, "cols", cols));
        actions.push(parts::set(parts::CARTWALL, "rows", rows));
    }
    settings::section(ui, "Ducking", t, |ui| {
        settings::form_note(
            ui,
            RichText::new("Per cart on this page, in dB under the main output.")
                .font(theme::text(theme::CAPTION))
                .color(t.text_muted),
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
    });
}

impl State {
    /// The Cartwall page of Preferences is no longer showing: the next visit drafts the page
    /// names again.
    pub(crate) fn leave_preferences(&mut self) {
        self.names_drafted = false;
    }
}

/// The Cartwall page of Preferences.
pub(crate) fn preferences(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    if !state.names_drafted {
        state.names.clone_from(&view.cartwall.pages);
        state.names_drafted = true;
    }
    cart_settings(ui, view, state, actions, t);
}
