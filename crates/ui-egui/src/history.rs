//! The as-played log: newest first, with a text filter and a playlist filter.
//! The table draws only the visible rows.

use egui::{Align, DragValue, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use jiff::SignedDuration;

use crate::app::{hms, mmss};
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{Row, StationView};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Aired,
    Playlist,
    Title,
    Length,
    Origin,
}

const COLS: [(Col, &str); 5] = [
    (Col::Aired, "AIRED AT"),
    (Col::Playlist, "PLAYLIST"),
    (Col::Title, "TITLE"),
    (Col::Length, "LENGTH"),
    (Col::Origin, "ORIGIN"),
];

/// The history state. It is local to the window.
pub(crate) struct State {
    filter: String,
    playlist: Option<String>,
    settings: bool,
    shown: [bool; COLS.len()],
    hours_back: u32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            filter: String::new(),
            playlist: None,
            settings: false,
            shown: [true; COLS.len()],
            hours_back: 3,
        }
    }
}

pub(crate) fn show(ui: &mut egui::Ui, view: &StationView, state: &mut State, t: &Tokens) {
    let mut open = state.settings;
    settings::header(ui, "History", &mut open, |_| {});
    state.settings = open;
    if state.settings {
        settings::section(ui, "History", t, |ui| {
            ui.horizontal_wrapped(|ui| {
                settings::label(ui, "Columns", t);
                for (i, (_, name)) in COLS.iter().enumerate() {
                    ui.checkbox(&mut state.shown[i], *name);
                }
            });
            ui.horizontal(|ui| {
                settings::label(ui, "Show the last", t);
                ui.add(
                    DragValue::new(&mut state.hours_back)
                        .range(1..=3)
                        .suffix(" h"),
                );
            });
        });
    }
    ui.horizontal(|ui| {
        let hint = "Filter by title or playlist";
        let edit = egui::TextEdit::singleline(&mut state.filter).hint_text(hint);
        ui.add(edit.desired_width(size::FIELD_W))
            .on_hover_text("History filter");
        let shown = state.playlist.as_deref().unwrap_or("All playlists");
        egui::ComboBox::from_id_salt("history_playlist")
            .width(size::COMBO_W)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.playlist, None, "All playlists");
                let mut names: Vec<&str> =
                    view.history.iter().map(|r| r.playlist.as_str()).collect();
                names.sort_unstable();
                names.dedup();
                for n in names {
                    ui.selectable_value(&mut state.playlist, Some(n.to_owned()), n);
                }
            });
    });
    ui.add_space(space::S);

    let needle = state.filter.to_lowercase();
    let since = view
        .now
        .timestamp()
        .checked_sub(SignedDuration::from_hours(i64::from(state.hours_back)))
        .unwrap_or(view.now.timestamp());
    let rows: Vec<&Row> = view
        .history
        .iter()
        .rev()
        .filter(|r| r.start.timestamp() >= since)
        .filter(|r| state.playlist.as_ref().is_none_or(|p| *p == r.playlist))
        .filter(|r| {
            needle.is_empty()
                || r.title.to_lowercase().contains(&needle)
                || r.playlist.to_lowercase().contains(&needle)
        })
        .collect();
    let cols: Vec<Col> = COLS
        .iter()
        .zip(state.shown)
        .filter(|(_, on)| *on)
        .map(|((c, _), _)| *c)
        .collect();
    let mut table = TableBuilder::new(ui)
        .id_salt("history_table")
        .striped(true)
        .auto_shrink(false)
        .cell_layout(Layout::left_to_right(Align::Center));
    for c in &cols {
        table = table.column(match c {
            Col::Aired => Column::exact(size::COL_TIME),
            Col::Playlist => Column::exact(size::COL_PLAYLIST + space::XL),
            Col::Title => Column::remainder().clip(true),
            Col::Length => Column::exact(size::COL_LEN),
            Col::Origin => Column::exact(size::COL_ORIGIN),
        });
    }
    let head = |s: &str| {
        RichText::new(s)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    table
        .header(size::DIVIDER_ROW, |mut h| {
            for c in &cols {
                let name = COLS.iter().find(|(k, _)| k == c).map_or("", |(_, n)| n);
                h.col(|ui| {
                    ui.label(head(name));
                });
            }
        })
        .body(|body| {
            body.rows(size::ROW, rows.len(), |mut row| {
                let Some(r) = rows.get(row.index()) else {
                    return;
                };
                for c in &cols {
                    row.col(|ui| cell(ui, *c, r, t));
                }
            });
        });
}

fn cell(ui: &mut egui::Ui, c: Col, r: &Row, t: &Tokens) {
    match c {
        Col::Aired => {
            ui.label(RichText::new(hms(&r.start)).font(theme::mono(theme::CODE)));
        }
        Col::Playlist => {
            let (dot, _) =
                ui.allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
            ui.painter()
                .circle_filled(dot.center(), size::DOT_R, Tokens::playlist(r.color));
            let ink = t.readable(r.color, t.canvas);
            ui.label(
                RichText::new(&r.playlist)
                    .font(theme::text(theme::LABEL))
                    .color(ink),
            );
        }
        Col::Title => {
            ui.add(
                egui::Label::new(RichText::new(&r.title).font(theme::text(theme::SMALL)))
                    .truncate(),
            );
        }
        Col::Length => {
            ui.label(
                RichText::new(mmss(r.len))
                    .font(theme::mono(theme::CODE))
                    .color(t.text_muted),
            );
        }
        Col::Origin => {
            ui.label(
                RichText::new(r.origin.label())
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        }
    }
}
