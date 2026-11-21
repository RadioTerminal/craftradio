//! The scheduled pane: the Upcoming list of woven-in items, and one tab per playlist.

use egui::{Align, Layout, RichText, Sense, Stroke, UiBuilder};
use jiff::SignedDuration;

use crate::app::{Action, Linked, hms, mmss};
use crate::library::LibraryDrag;
use crate::rules_editor;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DropTarget, PlaylistTab, Row, StationView};

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
) -> Option<usize> {
    tab_bar(ui, view, tab, actions);
    ui.add_space(space::S);
    ui.separator();
    ui.add_space(space::XS);
    match *tab {
        Tab::Upcoming => {
            upcoming(ui, view, link, t);
            None
        }
        Tab::Playlist(i) => match view.tabs.get(i) {
            Some(p) => playlist(ui, p, actions, t).then_some(i),
            None => {
                *tab = Tab::Upcoming;
                None
            }
        },
    }
}

fn tab_bar(ui: &mut egui::Ui, view: &StationView, tab: &mut Tab, actions: &mut Vec<Action>) {
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
            // The playlist colour always comes with its name.
            let text = RichText::new(format!("    {}", p.name)).font(theme::text(theme::LABEL));
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
            if r.clicked() {
                *tab = Tab::Playlist(i);
            }
        }
        ui.add_enabled(false, egui::Button::new(label("+")))
            .on_disabled_hover_text("coming soon");
    });
}

fn row_end(r: &Row) -> jiff::Timestamp {
    let start = r.start.timestamp();
    SignedDuration::try_from(r.len).map_or(start, |d| start + d)
}

fn upcoming(ui: &mut egui::Ui, view: &StationView, link: &mut Linked, t: &Tokens) {
    let now = view.now.timestamp();
    let target = link.scroll_scheduled.take();
    egui::ScrollArea::vertical()
        .id_salt("upcoming")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::XXS;
            for r in view.rows.iter().filter(|r| !r.is_main && row_end(r) > now) {
                let resp = upcoming_row(
                    ui,
                    r,
                    link.selected == Some(r.id),
                    r.start.timestamp() <= now,
                    t,
                );
                if resp.clicked() {
                    link.pick_in_scheduled(r.id);
                }
                if target == Some(r.id) {
                    resp.scroll_to_me(Some(Align::Center));
                }
            }
        });
}

fn upcoming_row(
    ui: &mut egui::Ui,
    r: &Row,
    selected: bool,
    on_air: bool,
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
                    ui.add_sized(
                        [size::COL_TIME, size::CONTROL_SM],
                        egui::Label::new(
                            RichText::new(time)
                                .font(theme::mono(theme::CODE))
                                .color(time_ink),
                        ),
                    );
                    let (dot, _) = ui
                        .allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
                    ui.painter().circle_filled(
                        dot.center(),
                        size::DOT_R,
                        Tokens::playlist(r.color),
                    );
                    ui.add_sized(
                        [size::COL_PLAYLIST, size::CONTROL_SM],
                        egui::Label::new(
                            RichText::new(&r.playlist)
                                .font(theme::text(theme::LABEL))
                                .strong()
                                .color(ink),
                        ),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let len = RichText::new(mmss(r.len))
                            .font(theme::mono(theme::CODE))
                            .color(t.text_muted);
                        ui.label(len);
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
                        let text = format!("fit: {sign}{:.1} s {mark}", fit.error_s.abs());
                        let font = theme::mono(theme::CAPTION);
                        ui.set_min_height(size::FIT_LINE);
                        ui.label(RichText::new(text).font(font).color(colour));
                    });
                }
            });
    });
    res.response
}

/// Show one playlist tab. It returns true when the ⚙ rules button was clicked.
fn playlist(ui: &mut egui::Ui, p: &PlaylistTab, actions: &mut Vec<Action>, t: &Tokens) -> bool {
    let mut cog = false;
    egui::ScrollArea::vertical()
        .id_salt(("playlist_tab", &p.name))
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), size::DOT_R, Tokens::playlist(p.color));
                ui.label(
                    RichText::new(&p.name)
                        .font(theme::text(theme::SECTION))
                        .strong(),
                );
                cog = rules_editor::cog(ui, &p.name);
            });
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
            let (_, dropped) = ui.dnd_drop_zone::<LibraryDrag, _>(frame, |ui| {
                ui.set_width(ui.available_width());
                for (name, len) in &p.items {
                    egui::Frame::new().inner_margin(pad::ROW).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(name).font(theme::text(theme::SMALL)));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let len = RichText::new(mmss(*len)).font(theme::mono(theme::CODE));
                                ui.label(len.color(t.text_muted));
                            });
                        });
                    });
                }
                let hint = RichText::new("Drop a library title here to add it");
                ui.label(hint.font(theme::text(theme::CAPTION)).color(t.text_muted));
            });
            if let Some(drag) = dropped {
                actions.push(Action::Drop {
                    item: drag.0,
                    target: DropTarget::Playlist(p.name.clone()),
                });
            }
        });
    cog
}

fn caption(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.label(
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
}
