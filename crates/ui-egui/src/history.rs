//! The as-played log: newest first, with a text filter and a playlist filter.
//! The table draws only the visible rows.

use crate::numbers;
use crate::widgets;
use egui::{Align, DragValue, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use jiff::SignedDuration;

use crate::app::{Action, Linked, Pane, hms, mmss};
use crate::context_menu::{COPY, menu_item};
use crate::parts;
use crate::preferences::Page;
use crate::select::Selection;
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::MetaTarget;
use crate::view::{Row, StationView};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Aired,
    Playlist,
    Title,
    Length,
    Origin,
}

const COLS: [(Col, &str, &str); 5] = [
    (Col::Aired, "AIRED AT", "col_aired"),
    (Col::Playlist, "PLAYLIST", "col_playlist"),
    (Col::Title, "TITLE", "col_title"),
    (Col::Length, "LENGTH", "col_length"),
    (Col::Origin, "ORIGIN", "col_origin"),
];
/// The log covers at most this many hours.
const MAX_HOURS_BACK: u32 = 3;

/// The view state of the window: filters and selection. The settings live in the view source.
#[derive(Default)]
pub(crate) struct State {
    filter: String,
    playlist: Option<String>,
    selected: Selection,
}

/// Why Delete keeps the rows of the log.
const KEEPS_ROWS: &str = "The as-played log keeps every row: it is the record of what aired.";

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let shown = COLS.map(|(_, _, key)| parts::bool(view, parts::HISTORY, key, true));
    let hours_back = parts::u32(view, parts::HISTORY, "hours_back", MAX_HOURS_BACK);
    // One row: the filters on the left, the settings button far right.
    ui.horizontal(|ui| {
        let hint = "Filter by title or playlist";
        let edit = egui::TextEdit::singleline(&mut state.filter).hint_text(hint);
        ui.add(edit.desired_width(size::FIELD_W))
            .on_hover_text("History filter");
        let shown = state.playlist.as_deref().unwrap_or("All playlists");
        craft_ui::ComboBox::from_id_salt("history_playlist")
            .width(size::COMBO_W)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                widgets::selectable_value(ui, &mut state.playlist, None, "All playlists");
                let mut names: Vec<&str> =
                    view.history.iter().map(|r| r.playlist.as_str()).collect();
                names.sort_unstable();
                names.dedup();
                for n in names {
                    widgets::selectable_value(ui, &mut state.playlist, Some(n.to_owned()), n);
                }
            });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut open = false;
            settings::settings_button(ui, "History", &mut open);
            if open {
                actions.push(Action::OpenPreferences(Page::History));
            }
        });
    });
    ui.add_space(space::S);

    let needle = state.filter.to_lowercase();
    let since = view
        .now
        .timestamp()
        .checked_sub(SignedDuration::from_hours(i64::from(hours_back)))
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
        .zip(shown)
        .filter(|(_, on)| *on)
        .map(|((c, _, _), _)| *c)
        .collect();
    let order: Vec<u64> = rows.iter().map(|r| r.id).collect();
    let mut scroll = None;
    if link.focus == Pane::History {
        if link.keys.step != 0 {
            scroll = state.selected.step(&order, link.keys.step);
        }
        if link.keys.select_all {
            state.selected.all(&order);
        }
        if link.keys.delete && !state.selected.ids.is_empty() {
            actions.push(Action::Toast(KEEPS_ROWS.to_owned()));
        }
    }
    egui::ScrollArea::horizontal()
        .id_salt("history_columns")
        .auto_shrink(false)
        .show(ui, |ui| {
            let table_id = ui
                .scope_id()
                .with(egui::IdSalt::new(("history_table", shown)));
            let mut widths = Vec::new();
            let mut table = TableBuilder::new(ui)
                .id_salt(("history_table", shown))
                .striped(true)
                .sense(Sense::click())
                .auto_shrink(false)
                .cell_layout(Layout::left_to_right(Align::Center));
            if let Some(i) = scroll {
                table = table.scroll_to_row(i, Some(Align::Center));
            }
            // The title takes the width that is left, so the table fills the window. Without a
            // title column the last column does.
            let fill = if cols.contains(&Col::Title) {
                Some(Col::Title)
            } else {
                cols.last().copied()
            };
            for c in &cols {
                if Some(*c) == fill {
                    table =
                        table.column(Column::remainder().at_least(size::TEXT_COL_MIN).clip(true));
                    continue;
                }
                table = table.column(match c {
                    Col::Aired => Column::exact(size::COL_TIME),
                    Col::Playlist => Column::initial(parts::f32(
                        view,
                        parts::HISTORY,
                        "width_playlist",
                        size::COL_PLAYLIST + space::XL,
                    ))
                    .at_least(size::TEXT_COL_MIN)
                    .at_most(size::TEXT_COL_MAX)
                    .resizable(true)
                    .clip(true),
                    Col::Title => Column::initial(parts::f32(
                        view,
                        parts::HISTORY,
                        "width_title",
                        size::COL_TITLE,
                    ))
                    .at_least(size::TEXT_COL_MIN)
                    .at_most(size::TEXT_COL_MAX)
                    .resizable(true)
                    .clip(true),
                    Col::Length => Column::exact(size::COL_LEN),
                    Col::Origin => Column::initial(parts::f32(
                        view,
                        parts::HISTORY,
                        "width_origin",
                        size::COL_ORIGIN,
                    ))
                    .at_least(size::TEXT_COL_MIN)
                    .at_most(size::TEXT_COL_MAX)
                    .resizable(true)
                    .clip(true),
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
                        let name = COLS
                            .iter()
                            .find(|(k, _, _)| k == c)
                            .map_or("", |(_, n, _)| n);
                        h.col(|ui| {
                            ui.label(head(name));
                        });
                    }
                })
                .body(|body| {
                    widths = body.widths().to_vec();
                    body.rows(size::ROW, rows.len(), |mut row| {
                        let Some(r) = rows.get(row.index()) else {
                            return;
                        };
                        row.set_selected(state.selected.contains(r.id));
                        for c in &cols {
                            row.col(|ui| cell(ui, *c, r, t));
                        }
                        let resp = row.response();
                        if resp.clicked() || resp.secondary_clicked() {
                            link.focus = Pane::History;
                        }
                        if resp.clicked() {
                            let m = resp.ctx.input(|i| i.modifiers);
                            state.selected.click(r.id, m, &order);
                        }
                        if resp.secondary_clicked() && !state.selected.contains(r.id) {
                            state.selected.click(r.id, egui::Modifiers::NONE, &order);
                        }
                        resp.context_menu(|ui| row_menu(ui, view, r, &state.selected.ids, actions));
                    });
                });
            let columns: Vec<_> = cols
                .iter()
                .map(|col| match col {
                    Col::Playlist => "width_playlist",
                    Col::Title => "width_title",
                    Col::Origin => "width_origin",
                    Col::Aired | Col::Length => "",
                })
                .zip(widths)
                .collect();
            parts::save_column_widths(ui, table_id, parts::HISTORY, &columns, actions);
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

fn line(r: &Row, sep: &str) -> String {
    [
        hms(&r.start),
        r.playlist.clone(),
        r.title.clone(),
        mmss(r.len),
        r.origin.label().to_owned(),
    ]
    .join(sep)
}

/// Quote a field for CSV when it holds a comma or a quote.
fn csv_field(f: &str) -> String {
    if f.contains([',', '"']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_owned()
    }
}

fn row_menu(
    ui: &mut egui::Ui,
    view: &StationView,
    r: &Row,
    selected: &[u64],
    actions: &mut Vec<Action>,
) {
    if menu_item(ui, "Copy row", Some(COPY)) {
        ui.ctx().copy_text(line(r, "\t"));
        actions.push(Action::Toast("copied 1 row".to_owned()));
    }
    if menu_item(ui, "Copy as CSV", None) {
        let rows: Vec<&Row> = if selected.contains(&r.id) {
            view.history
                .iter()
                .filter(|h| selected.contains(&h.id))
                .collect()
        } else {
            vec![r]
        };
        let mut csv = "aired_at,playlist,title,length,origin\n".to_owned();
        for h in &rows {
            let fields = [
                hms(&h.start),
                h.playlist.clone(),
                h.title.clone(),
                mmss(h.len),
                h.origin.label().to_owned(),
            ];
            let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
            csv.push_str(&line.join(","));
            csv.push('\n');
        }
        ui.ctx().copy_text(csv);
        actions.push(Action::Toast(format!("copied {} rows as CSV", rows.len())));
    }
    if menu_item(ui, &format!("Add to {} again", view.main_playlist), None) {
        actions.push(Action::Duplicate {
            titles: vec![(r.title.clone(), r.len)],
            after: crate::view::Place::End,
        });
    }
    if menu_item(ui, "Show in Library", None) {
        actions.push(Action::ShowInLibrary(r.title.clone()));
    }
    if menu_item(ui, "Properties", None) {
        let targets = if selected.contains(&r.id) {
            selected.iter().map(|id| MetaTarget::Row(*id)).collect()
        } else {
            vec![MetaTarget::Row(r.id)]
        };
        actions.push(Action::Metadata(targets));
    }
}

/// The History page of Preferences.
/// Restore page defaults on the History page: every column, and the longest look back.
pub(crate) fn restore_defaults(actions: &mut Vec<Action>) {
    for (_, _, key) in COLS {
        actions.push(parts::set(parts::HISTORY, key, true));
    }
    actions.push(parts::set(parts::HISTORY, "hours_back", MAX_HOURS_BACK));
}

pub(crate) fn preferences(
    ui: &mut egui::Ui,
    view: &StationView,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let old_shown = COLS.map(|(_, _, key)| parts::bool(view, parts::HISTORY, key, true));
    let old_hours = parts::u32(view, parts::HISTORY, "hours_back", MAX_HOURS_BACK);
    let mut shown = old_shown;
    let mut hours_back = old_hours;
    settings::section(ui, "Columns", t, |ui| {
        for (i, (_, name, _)) in COLS.iter().enumerate() {
            ui.checkbox(&mut shown[i], *name);
        }
    });
    settings::section(ui, "Range", t, |ui| {
        settings::form_row(ui, "Show the last", t, |ui| {
            ui.add(numbers::field(
                ui,
                DragValue::new(&mut hours_back)
                    .range(1..=MAX_HOURS_BACK)
                    .suffix(" h"),
            ))
        });
    });
    for (i, (_, _, key)) in COLS.iter().enumerate() {
        if shown[i] != old_shown[i] {
            actions.push(parts::set(parts::HISTORY, key, shown[i]));
        }
    }
    if hours_back != old_hours {
        actions.push(parts::set(parts::HISTORY, "hours_back", hours_back));
    }
}
