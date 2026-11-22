//! The right pane: Upcoming, the items that rules weave into Main, and one tab per playlist.

use crate::widgets;
use egui::{Align, Layout, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::add_menu;
use crate::app::{Action, CutSource, FileTarget, Linked, Pane, PasteTo, mmss};
use crate::context_menu::{self, COPY, CUT, DELETE, PASTE, menu_item, menu_item_if};
use crate::icons::{self, Icon};
use crate::library::LibraryDrag;
use crate::one_off::OneOffDialog;
use crate::prepared::Prepared;
use crate::rules_editor;
use crate::select::Selection;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{
    DropTarget, Edit, FitView, MetaTarget, OneOffView, Pick, PlaylistTab, Row, StationView, Timing,
};

/// The tolerances that the Edit timing menu offers, in seconds.
const TOLERANCES: [f32; 3] = [0.5, 1.0, 2.0];
/// The weights that the Set weight menu offers.
const WEIGHTS: [u32; 5] = [1, 2, 3, 4, 5];
/// The tolerance of a fixed row that nobody set yet, in seconds.
const DEFAULT_TOLERANCE_S: f32 = 1.0;
/// A fixed item closer than this to its time reads as "on time", in seconds.
const FIT_ON_TIME_S: f32 = 0.05;

/// The tab that the pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Upcoming,
    Playlist(usize),
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    prep: &Prepared,
    tab: &mut Tab,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    tab_bar(ui, view, tab, link, actions, t);
    ui.add_space(space::S);
    ui.separator();
    ui.add_space(space::XS);
    match *tab {
        Tab::Upcoming => upcoming(ui, view, prep, link, actions, t),
        Tab::Playlist(i) => match view.tabs.get(i) {
            Some(p) => playlist(ui, p, view, link, actions, t),
            None => *tab = Tab::Upcoming,
        },
    }
}

/// True while files from the file manager hover or drop over `rect`.
fn files_over(ui: &egui::Ui, rect: egui::Rect) -> bool {
    ui.input(|i| {
        (!i.raw.hovered_files.is_empty() || !i.raw.dropped_files.is_empty())
            && i.pointer.latest_pos().is_some_and(|p| rect.contains(p))
    })
}

/// The tabs of the pane on one row: Upcoming, then the playlists. craft-ui fits them; the ones
/// that do not fit are in a "More options" menu at the end of the row. Each is a tab to a
/// screen reader, with its selected state.
fn tab_bar(
    ui: &mut egui::Ui,
    view: &StationView,
    tab: &mut Tab,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let font = theme::text(theme::LABEL);
    let pad_x = ui.spacing().button_padding.x;
    let height = ui.spacing().interact_size.y;
    // A playlist tab has room for its colour dot before the name.
    let dot_room = space::S + size::DOT_R * 2.0;
    let labels: Vec<(String, bool)> = view
        .tabs
        .iter()
        .map(|p| {
            // The playlist colour always comes with its name.
            let folder = if p.source.is_some() {
                format!(" {}", icons::glyph(Icon::Folder))
            } else {
                String::new()
            };
            let off = if !p.enabled { " (off)" } else { "" };
            elide_chars(&format!("{}{folder}{off}", p.name), size::TAB_MAX_CHARS)
        })
        .collect();
    let measure = |ui: &egui::Ui, text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), t.text)
            .size()
            .x
    };
    let mut natural = vec![measure(ui, "Upcoming") + pad_x * 2.0];
    natural.extend(
        labels
            .iter()
            .map(|(l, _)| measure(ui, l) + pad_x * 2.0 + dot_room),
    );
    let selected = match *tab {
        Tab::Upcoming => 0,
        Tab::Playlist(i) => i + 1,
    };
    let new_label = icons::lead(Icon::Plus, "Playlist");
    let new_w = measure(ui, &new_label) + pad_x * 2.0 + space::XS;
    let more_w = height;
    let available = ui.available_width() - new_w;
    let fit = craft_ui::tabs::fit_whole(&natural, selected, available, more_w);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for &(k, w) in &fit.shown {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, height), Sense::hover());
            if k == 0 {
                let r = tab_at(ui, rect, "Upcoming", "Upcoming", None, selected == 0, t);
                if r.clicked() {
                    *tab = Tab::Upcoming;
                }
                continue;
            }
            let i = k - 1;
            let p = &view.tabs[i];
            let (shown, cut) = &labels[i];
            let r = tab_at(ui, rect, &p.name, shown, Some(p.color), selected == k, t);
            let r = if *cut {
                r.on_hover_text(p.name.as_str())
            } else {
                r
            };
            // A library item dropped on a tab joins that playlist.
            if let Some(drag) = r.dnd_release_payload::<LibraryDrag>() {
                actions.push(Action::Drop {
                    item: drag.0,
                    target: DropTarget::Playlist(p.name.clone()),
                });
            }
            if files_over(ui, r.rect) {
                link.hover_files(FileTarget::Playlist(p.name.clone()));
                let edge = Stroke::new(size::HAIRLINE * 2.0, t.accent);
                ui.painter()
                    .rect_stroke(r.rect, t.radius, edge, egui::StrokeKind::Outside);
            }
            if r.clicked() {
                *tab = Tab::Playlist(i);
            }
            r.context_menu(|ui| tab_menu(ui, p, actions, t));
        }
        if !fit.overflow.is_empty() {
            let items: Vec<craft_ui::menu::MenuItem> = fit
                .overflow
                .iter()
                .map(|&k| match k {
                    0 => craft_ui::menu::MenuItem::new("0", "Upcoming"),
                    k => {
                        craft_ui::menu::MenuItem::new(k.to_string(), view.tabs[k - 1].name.as_str())
                    }
                })
                .collect();
            let labels = craft_ui::menu::MenuLabels {
                more: "More playlists".to_owned(),
                ..Default::default()
            };
            if let Some(k) = craft_ui::menu::overflow(ui, &items, &labels)
                .and_then(|id| id.parse::<usize>().ok())
            {
                *tab = if k == 0 {
                    Tab::Upcoming
                } else {
                    Tab::Playlist(k - 1)
                };
            }
        }
        ui.add_space(space::XS);
        if ui
            .button(new_label)
            .on_hover_text("Make a new, empty playlist")
            .clicked()
        {
            actions.push(Action::Edit(Edit::NewPlaylist("New playlist".to_owned())));
        }
    });
}

/// One tab at `rect`: craft-ui's `Tab`, painted as the pane's buttons are, with the colour dot
/// of a playlist before its name. `name` is what a screen reader says.
fn tab_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    name: &str,
    shown: &str,
    colour: Option<[u8; 3]>,
    selected: bool,
    t: &Tokens,
) -> egui::Response {
    let id = ui.make_persistent_id(("pane_tab", name));
    let font = theme::text(theme::LABEL);
    craft_ui::tabs::Tab::new(id, name, selected)
        .focus_stroke(Stroke::new(size::HAIRLINE, t.accent_ink))
        .show_at(ui, rect, |ui, r| {
            let v = ui.style().interact_selectable(r, selected);
            if selected || r.hovered() {
                ui.painter().rect(
                    rect,
                    t.radius,
                    v.weak_bg_fill,
                    v.bg_stroke,
                    egui::StrokeKind::Inside,
                );
            }
            let mut x = rect.left() + ui.spacing().button_padding.x;
            if let Some(rgb) = colour {
                let dot = egui::pos2(x + size::DOT_R, rect.center().y);
                ui.painter()
                    .circle_filled(dot, size::DOT_R, Tokens::playlist(rgb));
                x += space::S + size::DOT_R * 2.0;
            }
            let ink = v.text_color();
            ui.painter().text(
                egui::pos2(x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                shown,
                font.clone(),
                ink,
            );
        })
}

fn tab_menu(ui: &mut egui::Ui, p: &PlaylistTab, actions: &mut Vec<Action>, t: &Tokens) {
    if menu_item(ui, "Rename…", None) {
        actions.push(Action::Rename(p.name.clone()));
    }
    if menu_item(ui, &icons::lead(Icon::Settings, "Rules…"), None) {
        actions.push(Action::OpenRules(p.name.clone()));
    }
    if menu_item(ui, "Remove duplicates…", None) {
        actions.push(Action::RemoveDuplicates(p.name.clone()));
    }
    if menu_item(ui, "Clear playlist…", None) {
        actions.push(Action::ClearPlaylist(p.name.clone()));
    }
    if menu_item(ui, "Duplicate playlist", None) {
        actions.push(Action::Edit(Edit::DuplicatePlaylist(p.name.clone())));
    }
    if menu_item(ui, "Make main", None) {
        actions.push(Action::UseAsMain(p.name.clone()));
    }
    if menu_item(ui, if p.enabled { "Disable" } else { "Enable" }, None) {
        actions.push(Action::Edit(Edit::SetPlaylistEnabled {
            playlist: p.name.clone(),
            enabled: !p.enabled,
        }));
    }
    ui.menu_button("Colour", |ui| {
        // Only palette colours: red, amber and the accent mean something else.
        for (k, (name, rgb)) in crate::theme::PALETTE.iter().enumerate() {
            let label =
                RichText::new(icons::lead(Icon::CircleDot, name)).color(t.readable(*rgb, t.card));
            if widgets::selectable(ui, p.color == *rgb, label).clicked() {
                actions.push(Action::Edit(Edit::SetPlaylistColour {
                    playlist: p.name.clone(),
                    colour: k,
                }));
                ui.close();
            }
        }
    });
    ui.separator();
    if context_menu::danger_item(ui, "Delete playlist…", t) {
        actions.push(Action::Confirm {
            text: format!("Delete the playlist {}? Undo brings it back.", p.name),
            yes: "Delete playlist".to_owned(),
            then: Box::new(Action::Edit(Edit::DeletePlaylist(p.name.clone()))),
        });
    }
}

fn upcoming(
    ui: &mut egui::Ui,
    view: &StationView,
    prep: &Prepared,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    ui.horizontal(|ui| {
        let text =
            RichText::new(icons::lead(Icon::Plus, "Add one-off")).font(theme::text(theme::LABEL));
        let resp = ui
            .button(text)
            .on_hover_text("Play a file, a library item, a stream or the live input at a set time");
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Add one-off"));
        if resp.clicked() {
            actions.push(Action::OneOff(Box::new(OneOffDialog::new(view))));
        }
    });
    ui.add_space(space::XS);
    let clip_rect = ui.clip_rect();
    let now = view.now.timestamp();
    let order: Vec<u64> = prep.upcoming.iter().map(|i| prep.ids[*i]).collect();
    link.step_rows(Pane::Scheduled, &order);
    let target = link.scroll_scheduled.take();
    let mut table = TableBuilder::new(ui)
        .id_salt("upcoming")
        .sense(Sense::click())
        .auto_shrink(false)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(size::UPCOMING_TIME).clip(true))
        .column(
            Column::initial(size::UPCOMING_PLAYLIST)
                .at_least(size::TEXT_COL_MIN)
                .clip(true),
        )
        .column(Column::remainder().clip(true))
        .column(Column::exact(size::COL_FIT).clip(true))
        .column(Column::exact(size::UPCOMING_LEN));
    if let Some(k) = target.and_then(|id| order.iter().position(|o| *o == id)) {
        table = table.scroll_to_row(k, Some(Align::Center));
    }
    table
        .header(size::DIVIDER_ROW, |mut header| {
            for name in ["START", "PLAYLIST", "TITLE", "FIT", "LENGTH"] {
                header.col(|ui| {
                    caption(ui, name, t);
                });
            }
        })
        .body(|body| {
            body.rows(size::UPCOMING_ROW, order.len(), |mut row| {
                let i = prep.upcoming[row.index()];
                let Some(r) = view.rows.get(i) else {
                    return;
                };
                let on_air = r.start.timestamp() <= now;
                row.set_selected(link.is_selected(r.id));
                row.col(|ui| {
                    crate::main_pane::on_air_bg(ui, on_air, true, t);
                    crate::widgets::deck_mark(ui, r.deck, &r.title);
                    let ink = if on_air { t.on_air } else { t.text };
                    ui.label(
                        RichText::new(&prep.start[i])
                            .font(theme::mono(theme::CODE))
                            .color(ink),
                    );
                });
                row.col(|ui| {
                    crate::main_pane::on_air_bg(ui, on_air, false, t);
                    ui.add(
                        egui::Label::new(
                            RichText::new(&r.playlist).color(t.readable(r.color, t.card)),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&r.playlist);
                });
                row.col(|ui| {
                    crate::main_pane::on_air_bg(ui, on_air, false, t);
                    ui.add(egui::Label::new(&r.title).truncate())
                        .on_hover_text(&r.title);
                });
                row.col(|ui| {
                    crate::main_pane::on_air_bg(ui, on_air, false, t);
                    if let Some(fit) = r.fixed {
                        ui.label(
                            RichText::new(fit_words(fit))
                                .font(theme::mono(theme::CAPTION))
                                .color(if fit.ok { t.ok } else { t.danger }),
                        );
                    }
                });
                row.col(|ui| {
                    crate::main_pane::on_air_bg(ui, on_air, false, t);
                    ui.label(
                        RichText::new(&prep.len[i])
                            .font(theme::mono(theme::CODE))
                            .color(t.text_muted),
                    );
                });
                let mut resp = row.response();
                resp.rect = resp.rect.intersect(clip_rect);
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!(
                            "{} {} {} {} {}",
                            prep.start[i],
                            r.playlist,
                            r.title,
                            r.fixed.map(fit_words).unwrap_or_default(),
                            prep.len[i]
                        ),
                    )
                });
                let resp = match r.fixed {
                    Some(fit) => resp.on_hover_text(fit_detail(r, fit)),
                    None => resp,
                };
                if resp.clicked() || (resp.secondary_clicked() && !link.is_selected(r.id)) {
                    let m = if resp.clicked() {
                        resp.ctx.input(|i| i.modifiers)
                    } else {
                        egui::Modifiers::NONE
                    };
                    link.click(Pane::Scheduled, r.id, m, &order);
                }
                resp.context_menu(|ui| match &r.one_off {
                    Some(o) => one_off_menu(ui, r, o, view, link, actions),
                    None => upcoming_menu(ui, r, link, actions),
                });
            });
        });
}

/// How a fixed item meets its time, in words: "on time", "0.4 s early", "2.0 s late" with a warning icon.
pub(crate) fn fit_words(fit: FitView) -> String {
    let off = fit.error_s.abs();
    let words = if off < FIT_ON_TIME_S {
        "on time".to_owned()
    } else if fit.error_s < 0.0 {
        format!("{off:.1} s early")
    } else {
        format!("{off:.1} s late")
    };
    if fit.ok {
        words
    } else {
        format!("{words} {}", icons::glyph(Icon::TriangleAlert))
    }
}

/// The raw numbers behind the words, for the tooltip.
fn fit_detail(r: &Row, fit: FitView) -> String {
    let sign = if fit.error_s < 0.0 { "−" } else { "+" };
    let mut text = format!(
        "Fixed at {} · starts {sign}{:.1} s from that time",
        crate::app::hms(&r.start),
        fit.error_s.abs()
    );
    if let Some(tm) = r.timing {
        let kind = if tm.hard { "hard" } else { "soft" };
        text.push_str(&format!(" · {kind}, tolerance ±{:.1} s", tm.tolerance_s));
    }
    if !fit.ok {
        text.push_str(" · outside the tolerance");
    }
    text
}

/// The menu of a one-off row: edit it, remove it, or copy its time.
pub(crate) fn one_off_menu(
    ui: &mut egui::Ui,
    r: &Row,
    o: &OneOffView,
    view: &StationView,
    link: &Linked,
    actions: &mut Vec<Action>,
) {
    if menu_item(ui, "Edit one-off…", None) {
        actions.push(Action::OneOff(Box::new(OneOffDialog::edit(o, view))));
    }
    if menu_item(ui, "Remove", Some(DELETE)) {
        actions.push(Action::Edit(Edit::Remove(link.targets(r.id))));
    }
    if menu_item(ui, "Copy time", None) {
        let text = o.start.strftime("%Y-%m-%d %H:%M:%S").to_string();
        ui.ctx().copy_text(text.clone());
        actions.push(Action::Toast(format!("Copied {text}")));
    }
}

fn upcoming_menu(ui: &mut egui::Ui, r: &Row, link: &mut Linked, actions: &mut Vec<Action>) {
    ui.menu_button("Edit timing", |ui| {
        let now = r.timing.unwrap_or(Timing {
            hard: true,
            tolerance_s: DEFAULT_TOLERANCE_S,
        });
        let mut set = |ui: &mut egui::Ui, label: &str, next: Timing, on: bool| {
            if context_menu::check_item(ui, on, label) {
                actions.push(Action::Edit(Edit::SetTiming {
                    row: r.id,
                    timing: Some(next),
                }));
            }
        };
        set(ui, "Hard", Timing { hard: true, ..now }, now.hard);
        set(ui, "Soft", Timing { hard: false, ..now }, !now.hard);
        ui.separator();
        for tol in TOLERANCES {
            let on = (now.tolerance_s - tol).abs() < f32::EPSILON;
            set(
                ui,
                &format!("Tolerance ±{tol:.1} s"),
                Timing {
                    tolerance_s: tol,
                    ..now
                },
                on,
            );
        }
    });
    if menu_item(ui, "Move to…", None) {
        actions.push(Action::Toast(
            "Move the event in the rules of its playlist".to_owned(),
        ));
    }
    if menu_item(ui, "Skip this occurrence", None) {
        actions.push(Action::Edit(Edit::Remove(vec![r.id])));
    }
    if menu_item(ui, "Remove", Some(DELETE)) {
        actions.push(Action::Edit(Edit::Remove(link.targets(r.id))));
    }
    if menu_item(ui, "Copy", Some(COPY)) {
        actions.push(Action::CopyTitles {
            titles: vec![(r.title.clone(), r.len)],
            cut: None,
        });
    }
    ui.separator();
    if menu_item(
        ui,
        &icons::lead(Icon::Settings, "Open playlist rules"),
        None,
    ) {
        actions.push(Action::OpenRules(r.playlist.clone()));
    }
    if menu_item(ui, "Properties", None) {
        actions.push(Action::Metadata(vec![MetaTarget::Row(r.id)]));
    }
}

/// Show one playlist tab.
fn playlist(
    ui: &mut egui::Ui,
    p: &PlaylistTab,
    view: &StationView,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let decks = &view.decks;
    let artists: std::collections::HashMap<_, _> = view
        .library
        .iter()
        .map(|item| (item.id, item.artist.as_deref().unwrap_or("—")))
        .collect();
    // The selection belongs to one playlist. Another tab starts with none.
    if link.playlist_sel_of != p.name {
        link.playlist_sel = Selection::default();
        p.name.clone_into(&mut link.playlist_sel_of);
    }
    let order = &p.ids;
    if link.focus == Pane::Playlist {
        if link.keys.step != 0 {
            link.playlist_sel.step(order, link.keys.step);
        }
        if link.keys.select_all {
            link.playlist_sel.all(order);
        }
        let sel = link.playlist_sel.in_order(order);
        if link.keys.delete && !sel.is_empty() {
            actions.push(remove_items(p, decks, sel));
        }
    }
    // One scroll only: the header stays put and the items list fills the rest of the pane.
    ui.scope(|ui| {
        ui.push_id(("playlist_tab", &p.name), |ui| {
            ui.horizontal(|ui| {
                let (dot, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), size::DOT_R, Tokens::playlist(p.color));
                let folder = if p.source.is_some() {
                    format!(" {}", icons::glyph(Icon::Folder))
                } else {
                    String::new()
                };
                let name = RichText::new(format!("{}{folder}", p.name));
                ui.label(name.font(theme::text(theme::SECTION)).strong());
                if rules_editor::cog(ui, &p.name) {
                    actions.push(Action::OpenRules(p.name.clone()));
                }
                add_menu::button(ui, &p.name, &add_menu::live(view), None, actions);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let n = p.items.len();
                    let noun = if n == 1 { "item" } else { "items" };
                    let summary = format!("{n} {noun} · {}", p.pick);
                    ui.add(
                        egui::Label::new(
                            RichText::new(summary)
                                .font(theme::text(theme::CAPTION))
                                .color(t.text_muted),
                        )
                        .truncate(),
                    );
                });
            });
            // The rules and the folder in one muted line each; the tooltip has all of it.
            let rules = if p.rules.is_empty() {
                "No rules: this playlist does not air by itself".to_owned()
            } else {
                format!("Rules: {}", p.rules.join("; "))
            };
            let line = |ui: &mut egui::Ui, text: &str| {
                ui.add(
                    egui::Label::new(
                        RichText::new(text)
                            .font(theme::text(theme::CAPTION))
                            .color(t.text_muted),
                    )
                    .truncate(),
                )
                .on_hover_text(text);
            };
            line(ui, &rules);
            if let Some(src) = &p.source {
                line(ui, &format!("From folder {}", src.path));
            }
            ui.add_space(space::XS);
            let frame = theme::card_frame(t).inner_margin(pad::TIGHT);
            let mut rows = Vec::new();
            let dragging = egui::DragAndDrop::has_payload_of_type::<LibraryDrag>(ui.ctx())
                || ui.input(|i| !i.raw.hovered_files.is_empty());
            let (zone, dropped) = ui.dnd_drop_zone::<LibraryDrag, _>(frame, |ui| {
                ui.set_width(ui.available_width());
                // The hint shows only when it helps: on an empty list, or during a drag.
                if p.items.is_empty() || dragging {
                    let hint = RichText::new("Drop a library title or files here to add them");
                    ui.label(hint.font(theme::text(theme::CAPTION)).color(t.text_muted));
                }
                // The list fills the pane down to its bottom edge: room for the header
                // row and the card's bottom edge.
                let edge = f32::from(pad::TIGHT.bottom) + size::HAIRLINE;
                let fill = (ui.available_height() - size::DIVIDER_ROW - edge).max(size::ROW * 3.0);
                TableBuilder::new(ui)
                    .id_salt("items")
                    .min_scrolled_height(fill)
                    .max_scroll_height(fill)
                    .sense(Sense::click())
                    .auto_shrink(false)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::remainder().clip(true))
                    .column(
                        Column::initial(size::COL_ARTIST)
                            .at_least(size::TEXT_COL_MIN)
                            .clip(true),
                    )
                    .column(Column::exact(size::COL_LEN))
                    .header(size::DIVIDER_ROW, |mut header| {
                        for name in ["TITLE", "ARTIST", "LENGTH"] {
                            header.col(|ui| {
                                caption(ui, name, t);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(size::ROW, p.items.len(), |mut row| {
                            let i = row.index();
                            let (name, len) = &p.items[i];
                            let id = p.ids.get(i).copied();
                            row.set_selected(id.is_some_and(|id| link.playlist_sel.contains(id)));
                            row.col(|ui| {
                                let on = decks.iter().position(|d| id.is_some() && d.item == id);
                                let hint = on
                                    .and_then(|d| decks[d].start.as_ref().map(|s| (d, s)))
                                    .map_or_else(String::new, |(d, s)| {
                                        crate::widgets::deck_hint(d, s)
                                    });
                                crate::widgets::deck_mark(ui, on, &hint);
                                let weight = p.weights.get(i).copied().unwrap_or(1);
                                let title = if weight == 1 {
                                    name.clone()
                                } else {
                                    format!("{name}  ×{weight}")
                                };
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(title).font(theme::text(theme::SMALL)),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(name);
                            });
                            row.col(|ui| {
                                let artist =
                                    id.and_then(|id| artists.get(&id).copied()).unwrap_or("—");
                                ui.add(egui::Label::new(artist).truncate())
                                    .on_hover_text(artist);
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(mmss(*len))
                                        .font(theme::mono(theme::CODE))
                                        .color(t.text_muted),
                                );
                            });
                            let resp = row.response();
                            rows.push(resp.rect);
                            if let Some(id) = id {
                                let plain = !link.playlist_sel.contains(id);
                                if resp.clicked() || (resp.secondary_clicked() && plain) {
                                    let m = if resp.clicked() {
                                        resp.ctx.input(|i| i.modifiers)
                                    } else {
                                        egui::Modifiers::NONE
                                    };
                                    link.focus = Pane::Playlist;
                                    link.playlist_sel.click(id, m, order);
                                }
                            }
                            resp.context_menu(|ui| {
                                item_menu(ui, p, decks, &add_menu::live(view), i, link, actions)
                            });
                        });
                    });
            });
            // The zone takes clicks only outside the rows. A click on the zone on top of the
            // rows would take the clicks of the rows.
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let on_row = pointer.is_some_and(|at| rows.iter().any(|r| r.contains(at)));
            if !on_row {
                zone.response.interact(Sense::click()).context_menu(|ui| {
                    add_menu::items(ui, &p.name, &add_menu::live(view), None, actions)
                });
            }
            if let Some(drag) = dropped {
                actions.push(Action::Drop {
                    item: drag.0,
                    target: DropTarget::Playlist(p.name.clone()),
                });
            }
            if files_over(ui, zone.response.rect) {
                link.hover_files(FileTarget::Playlist(p.name.clone()));
                let edge = Stroke::new(size::HAIRLINE * 2.0, t.accent);
                ui.painter().rect_stroke(
                    zone.response.rect,
                    t.radius_lg,
                    edge,
                    egui::StrokeKind::Outside,
                );
            }
        });
    });
}

/// The action that removes items of playlist `p`. Items on a deck stay, with the reason.
/// More than a few items ask first. Every remove can be undone.
fn remove_items(p: &PlaylistTab, decks: &[crate::view::DeckView], ids: Vec<u64>) -> Action {
    let on_deck = decks
        .iter()
        .enumerate()
        .find(|(_, d)| d.item.is_some_and(|i| ids.contains(&i)));
    if let Some((d, _)) = on_deck {
        let letter = ['A', 'B', 'C', 'D'].get(d).copied().unwrap_or('?');
        return Action::Toast(format!(
            "An item is on deck {letter}, so the items stay. Select the items without it."
        ));
    }
    let n = ids.len();
    let edit = Action::Edit(Edit::RemovePlaylistItems {
        playlist: p.name.clone(),
        ids,
    });
    if n > crate::main_pane::CONFIRM_REMOVE_OVER {
        return Action::Confirm {
            text: format!("Remove {n} items from {}? You can undo it.", p.name),
            yes: "Remove".to_owned(),
            then: Box::new(edit),
        };
    }
    edit
}

fn item_menu(
    ui: &mut egui::Ui,
    p: &PlaylistTab,
    decks: &[crate::view::DeckView],
    live: &add_menu::Live<'_>,
    i: usize,
    link: &mut Linked,
    actions: &mut Vec<Action>,
) {
    let Some((name, len)) = p.items.get(i).cloned() else {
        return;
    };
    let name = name.as_str();
    let id = p.ids.get(i).copied().unwrap_or_default();
    let copy = |cut: bool| Action::CopyTitles {
        titles: vec![(name.to_owned(), len)],
        cut: cut.then(|| CutSource::PlaylistItem {
            playlist: p.name.clone(),
            id,
        }),
    };
    // Remove acts on the selection when it holds this item.
    let remove = || remove_items(p, decks, link.playlist_sel.targets(id));
    ui.menu_button(icons::lead(Icon::Plus, "Add below"), |ui| {
        add_menu::items(ui, &p.name, live, Some(i + 1), actions)
    });
    // A cut removes the item only after a successful paste.
    if menu_item(ui, "Cut", Some(CUT)) {
        actions.push(copy(true));
    }
    if menu_item(ui, "Copy", Some(COPY)) {
        actions.push(copy(false));
    }
    if menu_item_if(ui, link.has_clip, "Paste", Some(PASTE)) {
        actions.push(Action::Paste(PasteTo::Playlist(p.name.clone())));
    }
    if menu_item(ui, "Remove", Some(DELETE)) {
        actions.push(remove());
    }
    ui.separator();
    for (label, up) in [("Move up", true), ("Move down", false)] {
        if menu_item(ui, label, None) {
            actions.push(Action::Edit(Edit::MovePlaylistItem {
                playlist: p.name.clone(),
                index: i,
                up,
            }));
        }
    }
    let weighted = p.order == Pick::Weighted;
    ui.add_enabled_ui(weighted, |ui| {
        ui.menu_button("Set weight", |ui| {
            for w in WEIGHTS {
                if menu_item(ui, &format!("×{w}"), None) {
                    actions.push(Action::Edit(Edit::SetWeight {
                        playlist: p.name.clone(),
                        index: i,
                        weight: w,
                    }));
                }
            }
        })
        .response
        .on_disabled_hover_text("Only for the weighted pick order");
    });
    if menu_item(ui, "Properties", None) {
        actions.push(Action::MetadataTitle(name.to_owned(), len));
    }
}

fn caption(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.label(
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
}

/// The first `max` characters of `text`, with "…" when the text was longer.
fn elide_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_owned(), false);
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    (out, true)
}
