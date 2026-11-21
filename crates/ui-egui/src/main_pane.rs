//! The main pane: the main playlist as a table. The table draws only the visible rows.
//! Items from other playlists show as thin divider rows.

use egui::{Align, Layout, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, FileTarget, Linked, Pane, PasteTo, hms, is_on_air, mmss, row_fields};
use crate::context_menu;
use crate::library::LibraryDrag;
use crate::rules_editor;
use crate::theme::{self, Tokens, size, space};
use crate::view::{DropTarget, Edit, MoveTo, Place, Row, StationView};
use crate::windows::ToolWindow;

/// Show the main pane.
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let folder = view.main.source.is_some();
    ui.horizontal(|ui| {
        let badge = if folder { " 📁" } else { "" };
        let title = RichText::new(format!("Main: {}{badge}", view.main.name));
        let header = ui.add(
            egui::Label::new(title.font(theme::text(theme::SECTION)).strong())
                .sense(Sense::click()),
        );
        header.context_menu(|ui| empty_menu(ui, link, actions));
        if rules_editor::cog(ui, &view.main.name) {
            actions.push(Action::OpenRules(view.main.name.clone()));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let n = view.rows.iter().filter(|r| r.is_main).count();
            let note = if folder { " · estimated lengths" } else { "" };
            let caption = format!("{n} tracks · next 6 hours{note}");
            ui.label(
                RichText::new(caption)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        });
    });
    ui.add_space(space::S);

    let now = view.now.timestamp();
    let order: Vec<u64> = view.rows.iter().map(|r| r.id).collect();
    let files = ui.input(|i| !i.raw.hovered_files.is_empty() || !i.raw.dropped_files.is_empty());
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let pane = ui.max_rect();
    let target = link.scroll_main.take();
    let mut table = TableBuilder::new(ui)
        .id_salt("main_pane")
        .sense(Sense::click())
        .auto_shrink(false)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(size::COL_TIME))
        .column(Column::remainder().clip(true))
        .column(Column::exact(size::COL_LEN));
    if let Some(i) = target.and_then(|id| view.rows.iter().position(|r| r.id == id)) {
        table = table.scroll_to_row(i, Some(Align::Center));
    }
    let head = |s: &str| {
        RichText::new(s)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    let mut file_row = None;
    let mut last_bottom = None;
    let out = table
        .header(size::DIVIDER_ROW, |mut h| {
            h.col(|ui| {
                ui.label(head("START"));
            });
            h.col(|ui| {
                ui.label(head("ARTIST – TITLE"));
            });
            h.col(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(head("LENGTH"))
                });
            });
        })
        .body(|body| {
            let heights = view.rows.iter().map(|r| {
                if r.is_main {
                    size::ROW
                } else {
                    size::DIVIDER_ROW
                }
            });
            body.heterogeneous_rows(heights, |mut row| {
                let Some(r) = view.rows.get(row.index()) else {
                    return;
                };
                let on_air = is_on_air(r, now);
                row.set_selected(link.is_selected(r.id));
                if r.is_main {
                    track_row(&mut row, r, on_air, t);
                } else {
                    divider_row(&mut row, r, on_air, t);
                }
                let resp = row.response();
                last_bottom = Some(resp.rect.bottom());
                if resp.clicked() {
                    let m = resp.ctx.input(|i| i.modifiers);
                    link.click(Pane::Main, r.id, m, &order);
                }
                if resp.secondary_clicked() && !link.is_selected(r.id) {
                    link.click(Pane::Main, r.id, egui::Modifiers::NONE, &order);
                }
                resp.context_menu(|ui| row_menu(ui, view, r, link, actions, t));
                // A library item or a file dropped on a row goes in after that row.
                let over = files && pointer.is_some_and(|p| resp.rect.contains(p));
                if over {
                    file_row = Some(r.id);
                }
                if resp.dnd_hover_payload::<LibraryDrag>().is_some() || over {
                    let layer =
                        egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dnd_insert"));
                    let line = Stroke::new(size::ON_AIR_BAR, t.accent);
                    resp.ctx.layer_painter(layer).hline(
                        resp.rect.x_range(),
                        resp.rect.bottom(),
                        line,
                    );
                }
                if let Some(drag) = resp.dnd_release_payload::<LibraryDrag>() {
                    actions.push(Action::Drop {
                        item: drag.0,
                        target: DropTarget::Main { after: r.id },
                    });
                }
            });
        });

    // The space under the last row has its own menu: paste and insert.
    let body = out.inner_rect;
    let top = last_bottom.unwrap_or(body.top()).max(body.top());
    if body.bottom() > top {
        let rest = egui::Rect::from_x_y_ranges(body.x_range(), top..=body.bottom());
        let resp = ui.interact(rest, ui.id().with("main_empty"), Sense::click());
        resp.context_menu(|ui| empty_menu(ui, link, actions));
    }

    if files && pointer.is_some_and(|p| pane.contains(p)) {
        let at = file_row.map_or(Place::End, Place::After);
        link.hover_files(FileTarget::Main(at));
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("file_pane"));
        let edge = Stroke::new(size::HAIRLINE * 2.0, t.accent);
        ui.ctx().layer_painter(layer).rect_stroke(
            pane,
            t.radius_lg,
            edge,
            egui::StrokeKind::Inside,
        );
    }

    if link.focus == Pane::Main {
        shortcuts(ui.ctx(), view, link, actions);
    }
}

fn titles(view: &StationView, ids: &[u64]) -> Vec<(String, std::time::Duration)> {
    view.rows
        .iter()
        .filter(|r| ids.contains(&r.id))
        .map(|r| (r.title.clone(), r.len))
        .collect()
}

/// The clipboard shortcuts for the main pane selection.
fn shortcuts(ctx: &egui::Context, view: &StationView, link: &Linked, actions: &mut Vec<Action>) {
    let p = context_menu::pressed(ctx);
    let sel = link.selected.clone();
    let last = sel.last().copied();
    if p.paste {
        let at = last.map_or(Place::End, Place::After);
        actions.push(Action::Paste(PasteTo::Main(at)));
    }
    if sel.is_empty() {
        return;
    }
    if p.copy || p.cut {
        actions.push(Action::CopyTitles {
            titles: titles(view, &sel),
            rows: sel.clone(),
            cut: p.cut,
        });
    }
    if p.duplicate
        && let Some(last) = last
    {
        actions.push(Action::Duplicate {
            titles: titles(view, &sel),
            after: Place::After(last),
        });
    }
    if p.delete {
        actions.push(Action::Edit(Edit::Remove(sel)));
    }
}

fn empty_menu(ui: &mut egui::Ui, link: &Linked, actions: &mut Vec<Action>) {
    if context_menu::menu_item_if(ui, link.has_clip, "Paste", Some(context_menu::PASTE)) {
        actions.push(Action::Paste(PasteTo::Main(Place::End)));
    }
    if context_menu::menu_item(ui, "Insert from library…", None) {
        actions.push(Action::OpenWindow(ToolWindow::Library));
        actions.push(Action::Toast(
            "drag a title from the Library onto a row".to_owned(),
        ));
    }
}

fn row_menu(
    ui: &mut egui::Ui,
    view: &StationView,
    r: &Row,
    link: &Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    use context_menu::{COPY, CUT, DELETE, DUPLICATE, PASTE, menu_item, menu_item_if};
    let sel = link.targets(r.id);
    let first = sel.first().copied().unwrap_or(r.id);
    let last = sel.last().copied().unwrap_or(r.id);
    if menu_item(ui, "Play next", None) {
        actions.push(Action::Edit(Edit::Move {
            id: r.id,
            to: MoveTo::Next,
        }));
    }
    ui.menu_button("Load to deck", |ui| {
        for deck in ["A", "B", "C", "D"] {
            if menu_item(ui, &format!("Deck {deck}"), None) {
                actions.push(Action::Toast(context_menu::NEEDS_ENGINE.to_owned()));
            }
        }
    });
    if menu_item(ui, "Insert from library…", None) {
        actions.push(Action::OpenWindow(ToolWindow::Library));
        actions.push(Action::Toast(
            "drag a title from the Library onto a row".to_owned(),
        ));
    }
    ui.separator();
    if menu_item(ui, "Cut", Some(CUT)) {
        actions.push(Action::CopyTitles {
            titles: titles(view, &sel),
            rows: sel.clone(),
            cut: true,
        });
    }
    if menu_item(ui, "Copy", Some(COPY)) {
        actions.push(Action::CopyTitles {
            titles: titles(view, &sel),
            rows: sel.clone(),
            cut: false,
        });
    }
    if menu_item_if(ui, link.has_clip, "Paste above", None) {
        actions.push(Action::Paste(PasteTo::Main(Place::Before(first))));
    }
    if menu_item_if(ui, link.has_clip, "Paste below", Some(PASTE)) {
        actions.push(Action::Paste(PasteTo::Main(Place::After(last))));
    }
    if menu_item(ui, "Duplicate", Some(DUPLICATE)) {
        actions.push(Action::Duplicate {
            titles: titles(view, &sel),
            after: Place::After(last),
        });
    }
    if menu_item(ui, "Remove", Some(DELETE)) {
        actions.push(Action::Edit(Edit::Remove(sel.clone())));
    }
    ui.separator();
    let lock = if r.locked {
        "📌 Unlock position"
    } else {
        "📌 Lock position"
    };
    if menu_item(ui, lock, None) {
        for id in &sel {
            actions.push(Action::Edit(Edit::ToggleLock(*id)));
        }
    }
    if menu_item(ui, "Move to top", None) {
        actions.push(Action::Edit(Edit::Move {
            id: r.id,
            to: MoveTo::Top,
        }));
    }
    if menu_item(ui, "Move to bottom", None) {
        actions.push(Action::Edit(Edit::Move {
            id: r.id,
            to: MoveTo::Bottom,
        }));
    }
    ui.separator();
    if menu_item(ui, "Show in Library", None) {
        actions.push(Action::ShowInLibrary(r.title.clone()));
    }
    if menu_item(ui, "Properties", None) {
        let extra = [
            ("Playlist", r.playlist.clone()),
            ("Starts", hms(&r.start)),
            ("Origin", r.origin.label().to_owned()),
        ];
        actions.push(Action::Properties {
            title: r.title.clone(),
            fields: row_fields(&r.title, r.len, &extra),
        });
    }
    let _ = t;
}

/// Paint the on-air background behind a cell, over the gaps between cells too.
fn on_air_bg(ui: &egui::Ui, on_air: bool, first: bool, t: &Tokens) {
    if !on_air {
        return;
    }
    let rect = ui.max_rect().expand2(egui::vec2(space::XS, 0.0));
    ui.painter().rect_filled(rect, 0.0, t.on_air_bg);
    if first {
        let mut bar = rect;
        bar.set_width(size::ON_AIR_BAR);
        ui.painter().rect_filled(bar, 0.0, t.on_air);
    }
}

fn track_row(row: &mut egui_extras::TableRow<'_, '_>, r: &Row, on_air: bool, t: &Tokens) {
    let time_ink = if on_air { t.on_air } else { t.text_muted };
    row.col(|ui| {
        on_air_bg(ui, on_air, true, t);
        ui.add_space(space::S);
        ui.label(
            RichText::new(hms(&r.start))
                .font(theme::mono(theme::CODE))
                .color(time_ink),
        );
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        let pin = if r.locked { "📌 " } else { "" };
        let text = RichText::new(format!("{pin}{}", r.title)).font(theme::text(theme::BODY));
        ui.add(egui::Label::new(if on_air { text.strong() } else { text }).truncate());
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(mmss(r.len))
                    .font(theme::mono(theme::CODE))
                    .color(t.text_muted),
            );
        });
    });
}

/// A row from another playlist: `── HH:MM:SS Name (m:ss) ──` in the playlist colour.
fn divider_row(row: &mut egui_extras::TableRow<'_, '_>, r: &Row, on_air: bool, t: &Tokens) {
    let bg = if on_air { t.on_air_bg } else { t.canvas };
    let ink = t.readable(r.color, bg);
    let line = Stroke::new(size::HAIRLINE, theme::Tokens::playlist(r.color));
    let rule = |ui: &mut egui::Ui| {
        let rect = ui.available_rect_before_wrap();
        let y = rect.center().y;
        if rect.width() > 0.0 {
            ui.painter().hline(rect.x_range(), y, line);
        }
    };
    row.col(|ui| {
        on_air_bg(ui, on_air, true, t);
        let rect = ui.max_rect();
        let lead = egui::Rangef::new(rect.left(), rect.left() + space::S - space::XXS);
        ui.painter().hline(lead, rect.center().y, line);
        ui.add_space(space::S);
        let mark = if r.fixed.is_some() { "⏱" } else { "" };
        let time = format!("{mark}{}", hms(&r.start));
        ui.label(
            RichText::new(time)
                .font(theme::mono(theme::TINY))
                .color(ink),
        );
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        let name = if r.title == r.playlist {
            format!("{} ({})", r.playlist, mmss(r.len))
        } else {
            format!("{} · {} ({})", r.playlist, r.title, mmss(r.len))
        };
        ui.label(
            RichText::new(name)
                .font(theme::text(theme::CAPTION))
                .color(ink),
        );
        rule(ui);
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        rule(ui);
    });
}
