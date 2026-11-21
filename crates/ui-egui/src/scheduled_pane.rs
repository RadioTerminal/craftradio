//! The scheduled pane: the Upcoming list of woven-in items, and one tab per playlist.

use egui::{Align, Layout, RichText, Sense, Stroke, UiBuilder};
use jiff::SignedDuration;

use crate::app::{Action, CutSource, FileTarget, Linked, Pane, PasteTo, hms, mmss};
use crate::context_menu::{self, COPY, CUT, DELETE, PASTE, menu_item, menu_item_if};
use crate::library::LibraryDrag;
use crate::rules_editor;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DropTarget, Edit, MetaTarget, Pick, PlaylistTab, Row, StationView, Timing};

/// The tolerances that the Edit timing menu offers, in seconds.
const TOLERANCES: [f32; 3] = [0.5, 1.0, 2.0];
/// The weights that the Set weight menu offers.
const WEIGHTS: [u32; 5] = [1, 2, 3, 4, 5];
/// The tolerance of a fixed row that nobody set yet, in seconds.
const DEFAULT_TOLERANCE_S: f32 = 1.0;

/// The tab that the pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Upcoming,
    Playlist(usize),
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
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
        Tab::Upcoming => upcoming(ui, view, link, actions, t),
        Tab::Playlist(i) => match view.tabs.get(i) {
            Some(p) => playlist(ui, p, link, actions, t),
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

fn tab_bar(
    ui: &mut egui::Ui,
    view: &StationView,
    tab: &mut Tab,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        let label = |s: &str| RichText::new(s).font(theme::text(theme::LABEL));
        if ui
            .selectable_label(*tab == Tab::Upcoming, label("Upcoming").strong())
            .clicked()
        {
            *tab = Tab::Upcoming;
        }
        for (i, p) in view.tabs.iter().enumerate() {
            let selected = *tab == Tab::Playlist(i);
            // The playlist colour always comes with its name.
            let folder = if p.source.is_some() { " 📁" } else { "" };
            let off = if !p.enabled { " (off)" } else { "" };
            let text = label(&format!("    {}{folder}{off}", p.name));
            let r = ui.selectable_label(selected, text);
            let dot = egui::pos2(r.rect.left() + space::S + size::DOT_R, r.rect.center().y);
            ui.painter()
                .circle_filled(dot, size::DOT_R, Tokens::playlist(p.color));
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
        ui.add_enabled(false, egui::Button::new(label("+")))
            .on_disabled_hover_text("coming soon");
    });
}

fn tab_menu(ui: &mut egui::Ui, p: &PlaylistTab, actions: &mut Vec<Action>, t: &Tokens) {
    if menu_item(ui, "Rename…", None) {
        actions.push(Action::Rename(p.name.clone()));
    }
    if menu_item(ui, "⚙ Rules…", None) {
        actions.push(Action::OpenRules(p.name.clone()));
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
    ui.separator();
    if context_menu::danger_item(ui, "Delete playlist…", t) {
        actions.push(Action::Confirm {
            text: format!(
                "Delete the playlist {}? The preview cannot undo this.",
                p.name
            ),
            then: Box::new(Action::Edit(Edit::DeletePlaylist(p.name.clone()))),
        });
    }
}

fn row_end(r: &Row) -> jiff::Timestamp {
    let start = r.start.timestamp();
    SignedDuration::try_from(r.len).map_or(start, |d| start + d)
}

fn upcoming(
    ui: &mut egui::Ui,
    view: &StationView,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let now = view.now.timestamp();
    let target = link.scroll_scheduled.take();
    let rows: Vec<&Row> = view
        .rows
        .iter()
        .filter(|r| !r.is_main && row_end(r) > now)
        .collect();
    let order: Vec<u64> = rows.iter().map(|r| r.id).collect();
    egui::ScrollArea::vertical()
        .id_salt("upcoming")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::XXS;
            for r in rows {
                let timing = r.timing;
                let on_air = r.start.timestamp() <= now;
                let resp = upcoming_row(ui, r, link.is_selected(r.id), on_air, timing, t);
                if resp.clicked() {
                    let m = resp.ctx.input(|i| i.modifiers);
                    link.click(Pane::Scheduled, r.id, m, &order);
                }
                resp.context_menu(|ui| upcoming_menu(ui, r, link, actions));
                if target == Some(r.id) {
                    resp.scroll_to_me(Some(Align::Center));
                }
            }
        });
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
            "preview — moving a fixed event needs the scheduler".to_owned(),
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
    if menu_item(ui, "⚙ Open playlist rules", None) {
        actions.push(Action::OpenRules(r.playlist.clone()));
    }
    if menu_item(ui, "Properties", None) {
        actions.push(Action::Metadata(vec![MetaTarget::Row(r.id)]));
    }
}

fn upcoming_row(
    ui: &mut egui::Ui,
    r: &Row,
    selected: bool,
    on_air: bool,
    timing: Option<Timing>,
    t: &Tokens,
) -> egui::Response {
    let id = ui.id().with(("upcoming_row", r.id));
    let hovered = ui.ctx().read_response(id).is_some_and(|h| h.hovered());
    let fill = match (selected, on_air, hovered) {
        (true, _, _) => t.selection,
        (_, true, _) => t.on_air_bg,
        (_, _, true) => t.hover,
        _ => t.card,
    };
    let edge = if selected { t.accent } else { t.card_border };
    let res = ui.scope_builder(UiBuilder::new().id(id).sense(Sense::click()), |ui| {
        egui::Frame::new()
            .fill(fill)
            .stroke(Stroke::new(size::HAIRLINE, edge))
            .corner_radius(t.radius_sm)
            .inner_margin(pad::ROW)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let ink = t.readable(r.color, fill);
                    let mark = if r.fixed.is_some() { "⏱" } else { " " };
                    let time = format!("{mark}{}", hms(&r.start));
                    let time_ink = if on_air { t.on_air } else { t.text };
                    let time = RichText::new(time)
                        .font(theme::mono(theme::CODE))
                        .color(time_ink);
                    ui.add_sized([size::COL_TIME, size::CONTROL_SM], egui::Label::new(time));
                    let (dot, _) = ui
                        .allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
                    ui.painter().circle_filled(
                        dot.center(),
                        size::DOT_R,
                        Tokens::playlist(r.color),
                    );
                    let name = RichText::new(&r.playlist)
                        .font(theme::text(theme::LABEL))
                        .strong();
                    ui.add_sized(
                        [size::COL_PLAYLIST, size::CONTROL_SM],
                        egui::Label::new(name.color(ink)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let len = RichText::new(mmss(r.len)).font(theme::mono(theme::CODE));
                        ui.label(len.color(t.text_muted));
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let title = RichText::new(&r.title).font(theme::text(theme::SMALL));
                            ui.add(egui::Label::new(title).truncate());
                        });
                    });
                });
                if let Some(fit) = r.fixed {
                    ui.horizontal(|ui| {
                        ui.add_space(size::COL_TIME + size::DOT_R * 2.0 + space::S * 2.0);
                        let sign = if fit.error_s < 0.0 { "−" } else { "+" };
                        let (mark, colour) = if fit.ok {
                            ("✔", t.ok)
                        } else {
                            ("✖", t.danger)
                        };
                        let mut text = format!("fit: {sign}{:.1} s {mark}", fit.error_s.abs());
                        if let Some(tm) = timing {
                            let kind = if tm.hard { "hard" } else { "soft" };
                            text.push_str(&format!(" · {kind} ±{:.1} s (preview)", tm.tolerance_s));
                        }
                        ui.set_min_height(size::FIT_LINE);
                        ui.label(
                            RichText::new(text)
                                .font(theme::mono(theme::CAPTION))
                                .color(colour),
                        );
                    });
                }
            });
    });
    res.response
}

/// Show one playlist tab.
fn playlist(
    ui: &mut egui::Ui,
    p: &PlaylistTab,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    egui::ScrollArea::vertical()
        .id_salt(("playlist_tab", &p.name))
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), size::DOT_R, Tokens::playlist(p.color));
                let folder = if p.source.is_some() { " 📁" } else { "" };
                let name = RichText::new(format!("{}{folder}", p.name));
                ui.label(name.font(theme::text(theme::SECTION)).strong());
                if rules_editor::cog(ui, &p.name) {
                    actions.push(Action::OpenRules(p.name.clone()));
                }
            });
            if let Some(src) = &p.source {
                caption(ui, &format!("From folder {}", src.path), t);
            }
            ui.add_space(space::S);
            caption(ui, "PICK", t);
            ui.label(RichText::new(&p.pick).font(theme::text(theme::BODY)));
            ui.add_space(space::M);
            caption(ui, "RULES", t);
            for rule in &p.rules {
                ui.label(RichText::new(format!("•  {rule}")).font(theme::text(theme::BODY)));
            }
            ui.add_space(space::M);
            caption(ui, &format!("ITEMS ({})", p.items.len()), t);
            let frame = theme::card_frame(t).inner_margin(pad::TIGHT);
            let (zone, dropped) = ui.dnd_drop_zone::<LibraryDrag, _>(frame, |ui| {
                ui.set_width(ui.available_width());
                for (i, (name, len)) in p.items.iter().enumerate() {
                    let row = egui::Frame::new().inner_margin(pad::ROW).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let weight = p.weights.get(i).copied().unwrap_or(1);
                            let w = if weight == 1 {
                                String::new()
                            } else {
                                format!("  ×{weight}")
                            };
                            ui.label(
                                RichText::new(format!("{name}{w}")).font(theme::text(theme::SMALL)),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let len = RichText::new(mmss(*len)).font(theme::mono(theme::CODE));
                                ui.label(len.color(t.text_muted));
                            });
                        });
                    });
                    let resp = row.response.interact(Sense::click());
                    resp.context_menu(|ui| item_menu(ui, p, i, name, *len, link, actions));
                }
                let hint = RichText::new("Drop a library title or files here to add them");
                ui.label(hint.font(theme::text(theme::CAPTION)).color(t.text_muted));
            });
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
}

fn item_menu(
    ui: &mut egui::Ui,
    p: &PlaylistTab,
    i: usize,
    name: &str,
    len: std::time::Duration,
    link: &mut Linked,
    actions: &mut Vec<Action>,
) {
    let copy = |cut: bool| Action::CopyTitles {
        titles: vec![(name.to_owned(), len)],
        cut: cut.then(|| CutSource::PlaylistItem {
            playlist: p.name.clone(),
            index: i,
        }),
    };
    let remove = || {
        Action::Edit(Edit::RemovePlaylistItem {
            playlist: p.name.clone(),
            index: i,
        })
    };
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
