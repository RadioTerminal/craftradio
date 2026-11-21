//! The main pane: the main playlist as a table. The table draws only the visible rows.
//! Items from other playlists show as thin divider rows.

use egui::{Align, Layout, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, Linked, hms, is_on_air, mmss};
use crate::library::LibraryDrag;
use crate::rules_editor;
use crate::theme::{self, Tokens, size, space};
use crate::view::{DropTarget, Row, StationView};

/// Show the main pane. It returns true when the ⚙ rules button was clicked.
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> bool {
    let mut cog = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("Main: {}", view.main.name))
                .font(theme::text(theme::SECTION))
                .strong(),
        );
        cog = rules_editor::cog(ui, &view.main.name);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let n = view.rows.iter().filter(|r| r.is_main).count();
            let caption = format!("{n} tracks · next 6 hours");
            ui.label(
                RichText::new(caption)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        });
    });
    ui.add_space(space::S);

    let now = view.now.timestamp();
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
    table
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
                row.set_selected(link.selected == Some(r.id));
                if r.is_main {
                    track_row(&mut row, r, on_air, t);
                } else {
                    divider_row(&mut row, r, on_air, t);
                }
                let resp = row.response();
                if resp.clicked() {
                    link.pick_in_main(r.id);
                }
                // A library item dropped on a row goes in after that row.
                if resp.dnd_hover_payload::<LibraryDrag>().is_some() {
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
    cog
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
        let text = RichText::new(&r.title).font(theme::text(theme::BODY));
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
