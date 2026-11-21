//! The main pane: the main playlist as a table. The table draws only the visible rows.
//! Items from other playlists show as thin divider rows.

use egui::{Align, Layout, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::add_menu;
use crate::app::{Action, CutSource, FileTarget, Linked, Pane, PasteTo};
use crate::context_menu;
use crate::icons::{self, Icon};
use crate::library::LibraryDrag;
use crate::prepared::Prepared;
use crate::rules_editor;
use crate::shortcuts;
use crate::theme::{self, Tokens, size, space};
use crate::view::{DropTarget, Edit, MetaTarget, MoveTo, Place, Row, StationView, StreamState};
use crate::windows::ToolWindow;

/// More rows than this ask before a remove.
pub(crate) const CONFIRM_REMOVE_OVER: usize = 5;

/// Show the main pane.
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    prep: &Prepared,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let folder = view.main.source.is_some();
    ui.horizontal(|ui| {
        // The controls come first, from the right. The title truncates in the room left.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut follow = link.follow;
            ui.checkbox(&mut follow, "Follow on air")
                .on_hover_text("Keep the row on air near the top. Scrolling by hand turns it off.");
            if !link.follow && ui.button("Jump to on air").clicked() {
                follow = true;
                link.followed = None;
            }
            if follow != link.follow {
                link.follow = follow;
                link.followed = None;
            }
            let n = prep.main_count;
            let note = if folder { " · estimated lengths" } else { "" };
            let picked = link.selected.len();
            let caption = if picked > 1 {
                format!(
                    "{} selected · {n} tracks · next 6 hours{note}",
                    thousands(picked)
                )
            } else {
                format!("{n} tracks · next 6 hours{note}")
            };
            ui.label(
                RichText::new(caption)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
            // New items go after the last selected row, or at the end.
            let after = link
                .selected
                .last()
                .and_then(|id| prep.index.get(id).copied())
                .map(|i| i + 1);
            add_menu::button(ui, &view.main.name, after, actions);
            if rules_editor::cog(ui, &view.main.name) {
                actions.push(Action::OpenRules(view.main.name.clone()));
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let badge = if folder {
                    format!(" {}", icons::glyph(Icon::Folder))
                } else {
                    String::new()
                };
                let title = RichText::new(format!("Main: {}{badge}", view.main.name));
                let header = ui.add(
                    egui::Label::new(title.font(theme::text(theme::SECTION)).strong())
                        .truncate()
                        .sense(Sense::click()),
                );
                header.context_menu(|ui| empty_menu(ui, view, link, actions));
            });
        });
    });
    ui.add_space(space::S);

    let order = &prep.ids;
    let files = ui.input(|i| !i.raw.hovered_files.is_empty() || !i.raw.dropped_files.is_empty());
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let pane = ui.max_rect();
    link.step_rows(Pane::Main, order);
    // A wheel or trackpad scroll over the pane turns follow off.
    let scrolled =
        ui.input(|i| i.smooth_scroll_delta.y != 0.0) && pointer.is_some_and(|p| pane.contains(p));
    if scrolled {
        link.follow = false;
    }
    let on_air_id = prep.on_air.and_then(|i| prep.ids.get(i).copied());
    // Follow: when a new row goes on air, it scrolls to the second visible row.
    let mut follow_to = None;
    if link.follow && on_air_id.is_some() && link.followed != on_air_id {
        link.followed = on_air_id;
        follow_to = prep.on_air.map(|i| i.saturating_sub(1));
    }
    let target = link.scroll_main.take();
    let mut table = TableBuilder::new(ui)
        .id_salt("main_pane")
        .sense(Sense::click())
        .auto_shrink(false)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(size::COL_TIME))
        .column(Column::remainder().clip(true))
        .column(Column::exact(size::COL_LEN));
    if let Some(i) = follow_to {
        // Follow jumps without animation, so the list does not drift while a person aims.
        table = table
            .scroll_to_row(i, Some(Align::Min))
            .animate_scrolling(false);
    } else if let Some(i) = target.and_then(|id| prep.index.get(&id).copied()) {
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
            // One fixed height for every row, so the table lays out only the visible rows.
            body.rows(size::ROW, view.rows.len(), |mut row| {
                let i = row.index();
                let Some(r) = view.rows.get(i) else {
                    return;
                };
                let on_air = prep.on_air == Some(i);
                let played = prep.on_air.is_some_and(|a| i < a);
                row.set_selected(link.is_selected(r.id));
                let look = Look {
                    on_air,
                    played,
                    preview: view.preview,
                };
                if r.is_main {
                    track_row(&mut row, r, i, prep, look, t);
                } else {
                    divider_row(&mut row, r, i, prep, look, t);
                }
                let resp = row.response();
                if on_air {
                    // The "now" line: played rows above, the rest below.
                    // The row's own layer, so a window above the pane covers the line too.
                    let layer = resp.layer_id;
                    let line = Stroke::new(size::ON_AIR_EDGE, t.on_air);
                    let clip = resp.rect.intersect(pane);
                    resp.ctx
                        .layer_painter(layer)
                        .with_clip_rect(clip.expand2(egui::vec2(0.0, size::ON_AIR_EDGE)))
                        .hline(resp.rect.x_range(), resp.rect.top(), line);
                }
                last_bottom = Some(resp.rect.bottom());
                if resp.clicked() {
                    let m = resp.ctx.input(|i| i.modifiers);
                    link.click(Pane::Main, r.id, m, order);
                }
                if resp.secondary_clicked() && !link.is_selected(r.id) {
                    link.click(Pane::Main, r.id, egui::Modifiers::NONE, order);
                }
                resp.context_menu(|ui| row_menu(ui, view, prep, r, link, actions, t));
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
        resp.context_menu(|ui| empty_menu(ui, view, link, actions));
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

    // Main and Upcoming share one selection, so the list keys act on both.
    if matches!(link.focus, Pane::Main | Pane::Scheduled) {
        if link.keys.select_all {
            link.selected.clone_from(&prep.ids);
        }
        shortcuts(view, prep, link, actions);
    }
}

/// The ids in the order of the timeline, so the first id matches the first copied title.
fn ordered(view: &StationView, ids: &[u64]) -> Vec<u64> {
    view.rows
        .iter()
        .filter(|r| ids.contains(&r.id))
        .map(|r| r.id)
        .collect()
}

fn titles(view: &StationView, ids: &[u64]) -> Vec<(String, std::time::Duration)> {
    view.rows
        .iter()
        .filter(|r| ids.contains(&r.id))
        .map(|r| (r.title.clone(), r.len))
        .collect()
}

/// The clipboard shortcuts for the main pane selection.
fn shortcuts(view: &StationView, prep: &Prepared, link: &Linked, actions: &mut Vec<Action>) {
    let p = link.keys;
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
            cut: p.cut.then(|| CutSource::Rows(ordered(view, &sel))),
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
        actions.push(remove_action(prep, sel));
    }
}

/// The action that removes rows. Rows on air or on a deck stay, with the reason.
/// More than a few rows ask first. Every remove can be undone.
pub(crate) fn remove_action(prep: &Prepared, rows: Vec<u64>) -> Action {
    if let Some(why) = rows.iter().find_map(|id| prep.protected(*id)) {
        return Action::Toast(why.to_owned());
    }
    if rows.len() > CONFIRM_REMOVE_OVER {
        return Action::Confirm {
            text: format!("Remove {} rows from Main? You can undo it.", rows.len()),
            yes: "Remove".to_owned(),
            then: Box::new(Action::Edit(Edit::Remove(rows))),
        };
    }
    Action::Edit(Edit::Remove(rows))
}

fn empty_menu(ui: &mut egui::Ui, view: &StationView, link: &Linked, actions: &mut Vec<Action>) {
    add_menu::items(ui, &view.main.name, None, actions);
    ui.separator();
    if context_menu::menu_item_if(ui, link.has_clip, "Paste", Some(context_menu::PASTE)) {
        actions.push(Action::Paste(PasteTo::Main(Place::End)));
    }
    if context_menu::menu_item(ui, "Remove duplicates…", None) {
        actions.push(Action::RemoveDuplicates(view.main.name.clone()));
    }
    if context_menu::menu_item(ui, "From library…", None) {
        actions.push(Action::OpenWindow(ToolWindow::Library));
        actions.push(Action::Toast(
            "drag a title from the Library onto a row".to_owned(),
        ));
    }
}

fn row_menu(
    ui: &mut egui::Ui,
    view: &StationView,
    prep: &Prepared,
    r: &Row,
    link: &Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    use context_menu::{COPY, CUT, DELETE, DUPLICATE, PASTE, menu_item, menu_item_if};
    let sel = link.targets(r.id);
    let first = sel.first().copied().unwrap_or(r.id);
    let last = sel.last().copied().unwrap_or(r.id);
    let below = view
        .rows
        .iter()
        .position(|row| row.id == last)
        .map(|i| i + 1);
    ui.menu_button(icons::lead(Icon::Plus, "Add below"), |ui| {
        add_menu::items(ui, &view.main.name, below, actions)
    });
    if menu_item(ui, "Play next", None) {
        actions.push(Action::Edit(Edit::Move {
            id: r.id,
            to: MoveTo::Next,
        }));
    }
    ui.menu_button("Load to deck", |ui| {
        for deck in ["A", "B", "C", "D"] {
            if menu_item(ui, &format!("Deck {deck}"), None) {
                actions.push(Action::Toast(context_menu::not_here(view)));
            }
        }
    });
    if menu_item(ui, "From library…", None) {
        actions.push(Action::OpenWindow(ToolWindow::Library));
        actions.push(Action::Toast(
            "drag a title from the Library onto a row".to_owned(),
        ));
    }
    ui.separator();
    if menu_item(ui, "Cut", Some(CUT)) {
        actions.push(Action::CopyTitles {
            titles: titles(view, &sel),
            cut: Some(CutSource::Rows(ordered(view, &sel))),
        });
    }
    if menu_item(ui, "Select all", Some(crate::shortcuts::SELECT_ALL)) {
        actions.push(Action::SelectAllRows);
    }
    if menu_item(ui, "Copy", Some(COPY)) {
        actions.push(Action::CopyTitles {
            titles: titles(view, &sel),
            cut: None,
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
    ui.separator();
    let lock = if r.locked {
        icons::lead(Icon::Pin, "Unlock position")
    } else {
        icons::lead(Icon::Pin, "Lock position")
    };
    if menu_item(ui, &lock, None) {
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
    if menu_item(ui, "Edit metadata…", None) {
        actions.push(Action::Metadata(
            sel.iter().map(|id| MetaTarget::Row(*id)).collect(),
        ));
    }
    // Remove sits last, alone, in the danger colour. A row on air or on a deck cannot go.
    ui.separator();
    match sel.iter().find_map(|id| prep.protected(*id)) {
        Some(why) => {
            let label = format!("Remove ({})", shortcuts::label(ui.ctx(), &DELETE));
            ui.add_enabled(false, egui::Button::new(label))
                .on_disabled_hover_text(why);
        }
        None => {
            if context_menu::danger_item(ui, "Remove", t) {
                actions.push(remove_action(prep, sel.clone()));
            }
        }
    }
}

/// Paint the on-air background behind a cell, over the gaps between cells too.
pub(crate) fn on_air_bg(ui: &egui::Ui, on_air: bool, first: bool, t: &Tokens) {
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

/// How a row looks: on air, or played already.
#[derive(Clone, Copy)]
struct Look {
    on_air: bool,
    played: bool,
    /// The source is the preview, which plays no real stream.
    preview: bool,
}

/// The time cell. A played row has a ✔ and the word in its tooltip.
fn start_cell(ui: &mut egui::Ui, r: &Row, text: &str, font: f32, ink: egui::Color32, played: bool) {
    // The deck that holds the row: its letter in its colour. The room stays when no deck has it.
    let hint = r
        .deck
        .map_or_else(String::new, |d| crate::widgets::deck_hint(d, &r.start));
    crate::widgets::deck_mark(ui, r.deck, &hint);
    // The timer icon sits in a slot of its own, so every time starts at the same x.
    let (slot, _) = ui.allocate_exact_size(egui::vec2(size::GLYPH_SLOT, font), Sense::hover());
    if r.fixed.is_some() {
        ui.painter().text(
            slot.center(),
            egui::Align2::CENTER_CENTER,
            icons::glyph(Icon::Timer),
            theme::text(font),
            ink,
        );
    }
    let r = ui.label(RichText::new(text).font(theme::mono(font)).color(ink));
    if played {
        ui.label(RichText::new("✔").font(theme::mono(font)).color(ink));
        r.on_hover_text("played");
    }
}

fn track_row(
    row: &mut egui_extras::TableRow<'_, '_>,
    r: &Row,
    i: usize,
    prep: &Prepared,
    look: Look,
    t: &Tokens,
) {
    let Look {
        on_air,
        played,
        preview,
    } = look;
    let time_ink = if on_air { t.on_air } else { t.text_muted };
    row.col(|ui| {
        on_air_bg(ui, on_air, true, t);
        ui.add_space(space::S);
        start_cell(ui, r, &prep.time[i], theme::CODE, time_ink, played);
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        let text = if r.locked {
            RichText::new(icons::lead(Icon::Pin, &r.title))
        } else {
            RichText::new(&r.title)
        };
        let text = text.font(theme::text(theme::BODY));
        // Played rows are dimmed; the ✔ in the time cell says why.
        let text = if played {
            text.color(t.text_muted)
        } else {
            text
        };
        if let Some(state) = r.stream {
            stream_badge(ui, state, r.meta.keep_alive.unwrap_or(false), preview, t);
        }
        ui.add(egui::Label::new(if on_air { text.strong() } else { text }).truncate());
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(&prep.len[i])
                    .font(theme::mono(theme::CODE))
                    .color(t.text_muted),
            );
        });
    });
}

/// The state badge of a stream row. It says that no real stream plays in the preview.
fn stream_badge(
    ui: &mut egui::Ui,
    state: StreamState,
    keep_alive: bool,
    preview: bool,
    t: &Tokens,
) {
    let (text, colour) = match state {
        StreamState::Connected if preview => {
            (icons::lead(Icon::Circle, "stream simulated"), t.text_muted)
        }
        StreamState::Connected => (icons::lead(Icon::CircleDot, "stream connected"), t.ok),
        StreamState::Reconnecting { attempt, next_in_s } => (
            format!("reconnecting · bed playing · try {attempt}, next in {next_in_s} s"),
            t.warn,
        ),
        StreamState::Down => ("down · keep-alive is off".to_owned(), t.danger),
    };
    let keep = if keep_alive { " · keep alive" } else { "" };
    let text = if preview {
        format!("{text}{keep} · preview — no real stream")
    } else {
        format!("{text}{keep}")
    };
    // The badge takes at most a third of the cell, so the title keeps its room.
    let max = ui.available_width() / 3.0;
    crate::widgets::truncated_label_max(
        ui,
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(colour),
        max,
    );
}

/// A row from another playlist: `── HH:MM:SS Name (m:ss) ──` in the playlist colour.
fn divider_row(
    row: &mut egui_extras::TableRow<'_, '_>,
    r: &Row,
    i: usize,
    prep: &Prepared,
    look: Look,
    t: &Tokens,
) {
    let Look { on_air, played, .. } = look;
    let bg = if on_air { t.on_air_bg } else { t.canvas };
    let ink = if played {
        t.text_muted
    } else {
        t.readable(r.color, bg)
    };
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
        ui.add_space(space::S);
        start_cell(ui, r, &prep.time[i], theme::TINY, ink, played);
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        ui.label(
            RichText::new(&prep.divider[i])
                .font(theme::text(theme::CAPTION))
                .color(ink),
        );
        rule(ui);
    });
    row.col(|ui| {
        on_air_bg(ui, on_air, false, t);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(&prep.len[i])
                    .font(theme::mono(theme::CAPTION))
                    .color(t.text_muted),
            );
        });
    });
}

/// A count with thin spaces between thousands, as "3 412".
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(c);
    }
    out
}
