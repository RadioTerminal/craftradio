//! The as-played log: newest first, for the last hours, one day or a week, with a text filter
//! and a playlist filter. The table draws only the visible rows. Export writes the range on
//! show as the whole log, a royalty report or a proof of airing.

use crate::numbers;
use crate::widgets;
use egui::{Align, DragValue, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use jiff::{SignedDuration, Timestamp, ToSpan};
use lucide_icons::Icon;
use std::time::Duration;

use crate::app::{Action, Linked, Pane, hms, mmss};
use crate::context_menu::{COPY, menu_item};
use crate::icons;
use crate::parts;
use crate::preferences::Page;
use crate::select::Selection;
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{AirStatus, Edit, HistoryReport, MetaTarget};
use crate::view::{Row, StationView};
use crate::windows::{self, Band};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Aired,
    Ended,
    Playlist,
    Artist,
    Title,
    Length,
    Origin,
    Rule,
    Status,
}

/// Each column: its header, its settings key, and whether it shows before a person chooses.
const COLS: [(Col, &str, &str, bool); 9] = [
    (Col::Aired, "AIRED AT", "col_aired", true),
    (Col::Ended, "ENDED AT", "col_ended", false),
    (Col::Playlist, "PLAYLIST", "col_playlist", true),
    (Col::Artist, "ARTIST", "col_artist", false),
    (Col::Title, "TITLE", "col_title", true),
    (Col::Length, "LENGTH", "col_length", true),
    (Col::Origin, "ORIGIN", "col_origin", true),
    (Col::Rule, "RULE", "col_rule", false),
    (Col::Status, "STATUS", "col_status", true),
];
/// "Last hours" covers at most this many hours.
const MAX_HOURS_BACK: u32 = 3;
/// "Last 7 days" starts this many days before today.
const WEEK_DAYS_BACK: i64 = 6;

/// The part of the log on show.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Span {
    /// The last hours, up to now.
    #[default]
    Recent,
    /// One day of the station.
    Day(jiff::civil::Date),
    /// Today and the six days before it.
    Week,
}

/// The view state of the window: filters and selection. The settings live in the view source.
#[derive(Default)]
pub(crate) struct State {
    filter: String,
    playlist: Option<String>,
    selected: Selection,
    span: Span,
    /// The range that the view source reads now. `None` is the last hours.
    asked: Option<(Timestamp, Timestamp)>,
}

/// Why Delete keeps the rows of the log.
const KEEPS_ROWS: &str = "The as-played log keeps every row: it is the record of what aired.";

/// The first instant of `day` at the station.
fn day_start(view: &StationView, day: jiff::civil::Date) -> Timestamp {
    day.at(0, 0, 0, 0)
        .to_zoned(view.now.time_zone().clone())
        .map_or(view.now.timestamp(), |z| z.timestamp())
}

/// The rows of `span` started in this range: from the first instant, to before the second.
fn bounds(view: &StationView, span: Span, hours_back: u32) -> (Timestamp, Timestamp) {
    let today = view.now.date();
    let next = |day: jiff::civil::Date| {
        day.tomorrow()
            .map_or(Timestamp::MAX, |d| day_start(view, d))
    };
    match span {
        Span::Recent => {
            let now = view.now.timestamp();
            let since = now
                .checked_sub(SignedDuration::from_hours(i64::from(hours_back)))
                .unwrap_or(now);
            (since, Timestamp::MAX)
        }
        Span::Day(day) => (day_start(view, day), next(day)),
        Span::Week => {
            let first = today.checked_sub(WEEK_DAYS_BACK.days()).unwrap_or(today);
            (day_start(view, first), next(today))
        }
    }
}

/// The name of `span` in the range picker.
fn span_name(view: &StationView, span: Span, hours_back: u32) -> String {
    let today = view.now.date();
    match span {
        Span::Recent if hours_back == 1 => "Last hour".to_owned(),
        Span::Recent => format!("Last {hours_back} hours"),
        Span::Day(day) if day == today => "Today".to_owned(),
        Span::Day(day) if Some(day) == today.yesterday().ok() => "Yesterday".to_owned(),
        Span::Day(_) => "One day".to_owned(),
        Span::Week => "Last 7 days".to_owned(),
    }
}

/// The range picker: the last hours, a day with steps to the days around it, or a week.
fn span_picker(ui: &mut egui::Ui, view: &StationView, span: &mut Span, hours_back: u32) {
    let today = view.now.date();
    let mut options = vec![Span::Recent, Span::Day(today)];
    options.extend(today.yesterday().ok().map(Span::Day));
    options.push(Span::Week);
    craft_ui::ComboBox::from_id_salt("history_range")
        .width(size::COMBO_W)
        .selected_text(span_name(view, *span, hours_back))
        .show_ui(ui, |ui| {
            for option in options {
                let name = span_name(view, option, hours_back);
                widgets::selectable_value(ui, span, option, name);
            }
        })
        .response
        .on_hover_text("History range");
    let Span::Day(day) = *span else {
        return;
    };
    let hint = "Show the day before";
    if widgets::icon_button(ui, Icon::ChevronLeft, false, "Previous day", hint).clicked()
        && let Ok(before) = day.yesterday()
    {
        *span = Span::Day(before);
    }
    ui.label(
        RichText::new(day.strftime("%a %-d %b %Y").to_string()).font(theme::text(theme::LABEL)),
    );
    let next = ui.add_enabled_ui(day < today, |ui| {
        widgets::icon_button(
            ui,
            Icon::ChevronRight,
            false,
            "Next day",
            "Show the day after",
        )
    });
    if next.inner.clicked()
        && let Ok(after) = day.tomorrow()
    {
        *span = Span::Day(after);
    }
}

/// The Export menu: each entry asks for a file and writes the range on show to it.
fn export_menu(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &State,
    range: (Timestamp, Timestamp),
    actions: &mut Vec<Action>,
) {
    let (since, until) = range;
    // An open range ends now: nothing later is in the log.
    let until = until.min(view.now.timestamp());
    let day = |t: Timestamp| t.to_zoned(view.now.time_zone().clone()).date();
    let days = match state.span {
        Span::Week => format!("{}_to_{}", day(since), view.now.date()),
        Span::Recent | Span::Day(_) => day(since).to_string(),
    };
    let airings = match &state.playlist {
        Some(p) => format!("Proof of airing for {p} (CSV)…"),
        None => "Proof of airing (CSV)…".to_owned(),
    };
    let plays = match &state.playlist {
        Some(p) => format!("Royalty report for {p} (CSV)…"),
        None => "Royalty report (CSV)…".to_owned(),
    };
    let entries = [
        (HistoryReport::Log, "As-played log (CSV)…", "as-played"),
        (HistoryReport::Plays, plays.as_str(), "royalty"),
        (HistoryReport::Airings, airings.as_str(), "airings"),
    ];
    ui.menu_button(icons::lead(Icon::FileDown, "Export"), |ui| {
        for (report, label, stem) in entries {
            if menu_item(ui, label, None)
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("{stem}-{days}.csv"))
                    .add_filter("CSV", &["csv"])
                    .save_file()
            {
                actions.push(Action::Edit(Edit::ExportHistory {
                    path: path.display().to_string(),
                    report,
                    since,
                    until,
                    playlist: state.playlist.clone(),
                }));
            }
        }
    })
    .response
    .on_hover_text(
        "Write the range on show to a file. The log holds every playlist; the reports keep to \
         the playlist on show",
    );
}

/// The line under the filters: how much is on show, and how much of it was cut off.
fn totals(view: &StationView, rows: &[&Row]) -> String {
    let air: Duration = rows.iter().map(|r| r.len).sum();
    let cut = rows
        .iter()
        .filter(|r| {
            r.aired
                .as_ref()
                .is_some_and(|a| a.status == AirStatus::CutOff)
        })
        .count();
    let items = if rows.len() == 1 { "item" } else { "items" };
    let mut text = format!("{} {items} · {} on air", rows.len(), mmss(air));
    if rows.is_empty() {
        text = "No airings match.".to_owned();
    }
    if cut > 0 {
        text.push_str(&format!(" · {cut} cut off"));
    }
    if view.history_more {
        text.push_str(
            " · the range holds more rows than the window reads, so the newest are missing; \
             Export writes them all",
        );
    }
    text
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let shown = COLS.map(|(_, _, key, default)| parts::bool(view, parts::HISTORY, key, default));
    let hours_back = parts::u32(view, parts::HISTORY, "hours_back", MAX_HOURS_BACK);
    // In a window with a merged title bar the settings button sits there, as in the other
    // windows. Elsewhere it ends the filter row, so it takes no row of its own.
    let in_content = windows::title_band(ui) == Band::None;
    if !in_content {
        settings::opens_preferences(ui, "History", Page::History, actions);
    }
    let range = bounds(view, state.span, hours_back);
    ui.horizontal_wrapped(|ui| {
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
        span_picker(ui, view, &mut state.span, hours_back);
        export_menu(ui, view, state, range, actions);
        if in_content {
            let mut open = false;
            settings::settings_button(ui, "History", &mut open);
            if open {
                actions.push(Action::OpenPreferences(Page::History));
            }
        }
    });
    // The view source reads the range that the picker shows. The last hours are its own range.
    let range = bounds(view, state.span, hours_back);
    let wanted = (state.span != Span::Recent).then_some(range);
    if wanted != state.asked {
        state.asked = wanted;
        actions.push(Action::HistoryRange(wanted));
    }

    let needle = state.filter.to_lowercase();
    let rows: Vec<&Row> = view
        .history
        .iter()
        .rev()
        .filter(|r| (range.0..range.1).contains(&r.start.timestamp()))
        .filter(|r| state.playlist.as_ref().is_none_or(|p| *p == r.playlist))
        .filter(|r| {
            needle.is_empty()
                || r.title.to_lowercase().contains(&needle)
                || r.playlist.to_lowercase().contains(&needle)
        })
        .collect();
    ui.label(
        RichText::new(totals(view, &rows))
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
    ui.add_space(space::S);
    // A range of more than one day says the day of each row.
    let dated = state.span == Span::Week;
    let cols: Vec<Col> = COLS
        .iter()
        .zip(shown)
        .filter(|(_, on)| *on)
        .map(|((c, ..), _)| *c)
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
    let text_col = |key: &str, initial: f32| {
        Column::initial(parts::f32(view, parts::HISTORY, key, initial))
            .at_least(size::TEXT_COL_MIN)
            .at_most(size::TEXT_COL_MAX)
            .resizable(true)
            .clip(true)
    };
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
                    Col::Aired | Col::Ended => Column::exact(size::COL_TIME),
                    Col::Length => Column::exact(size::COL_LEN),
                    Col::Title => text_col(width_key(*c), size::COL_TITLE),
                    Col::Playlist | Col::Artist => {
                        text_col(width_key(*c), size::COL_PLAYLIST + space::XL)
                    }
                    Col::Origin | Col::Rule | Col::Status => {
                        text_col(width_key(*c), size::COL_ORIGIN)
                    }
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
                            .find(|(k, ..)| k == c)
                            .map_or("", |(_, n, ..)| n);
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
                            row.col(|ui| cell(ui, *c, r, dated, t));
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
            let columns: Vec<_> = cols.iter().map(|col| width_key(*col)).zip(widths).collect();
            parts::save_column_widths(ui, table_id, parts::HISTORY, &columns, actions);
        });
}

/// The settings key of the width of a column that a person can resize, or "" for a fixed one.
fn width_key(c: Col) -> &'static str {
    match c {
        Col::Playlist => "width_playlist",
        Col::Artist => "width_artist",
        Col::Title => "width_title",
        Col::Origin => "width_origin",
        Col::Rule => "width_rule",
        Col::Status => "width_status",
        Col::Aired | Col::Ended | Col::Length => "",
    }
}

/// A time of day, with the day in front in a range of more than one day.
fn clock(z: &jiff::Zoned, dated: bool) -> String {
    if dated {
        z.strftime("%m-%d %H:%M:%S").to_string()
    } else {
        hms(z)
    }
}

/// The end of the airing of `r`, if it has ended.
fn ended(r: &Row) -> Option<&jiff::Zoned> {
    r.aired.as_ref()?.end.as_ref()
}

fn status(r: &Row) -> &'static str {
    r.aired.as_ref().map_or("", |a| a.status.label())
}

fn rule(r: &Row) -> &str {
    r.aired
        .as_ref()
        .and_then(|a| a.rule.as_deref())
        .unwrap_or("")
}

fn cell(ui: &mut egui::Ui, c: Col, r: &Row, dated: bool, t: &Tokens) {
    let muted = |s: &str| {
        RichText::new(s)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    match c {
        Col::Aired => {
            ui.label(RichText::new(clock(&r.start, dated)).font(theme::mono(theme::CODE)));
        }
        Col::Ended => {
            let text = ended(r).map_or("—".to_owned(), |z| clock(z, dated));
            ui.label(
                RichText::new(text)
                    .font(theme::mono(theme::CODE))
                    .color(t.text_muted),
            );
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
        Col::Artist => {
            ui.add(
                egui::Label::new(RichText::new(&r.meta.artist).font(theme::text(theme::SMALL)))
                    .truncate(),
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
            ui.label(muted(r.origin.label()));
        }
        Col::Rule => {
            ui.add(egui::Label::new(muted(rule(r))).truncate());
        }
        Col::Status => {
            // An airing that did not run to its end stands out; the word says which.
            let plain = r
                .aired
                .as_ref()
                .is_none_or(|a| a.status == AirStatus::Aired);
            let text = RichText::new(status(r)).font(theme::text(theme::CAPTION));
            ui.label(text.color(if plain { t.text_muted } else { t.text }));
        }
    }
}

/// The fields of a row as text: the day, the start and the end, then what aired and how.
fn fields(r: &Row) -> [String; 10] {
    [
        r.start.date().to_string(),
        hms(&r.start),
        ended(r).map(hms).unwrap_or_default(),
        r.playlist.clone(),
        r.meta.artist.clone(),
        r.title.clone(),
        mmss(r.len),
        r.origin.label().to_owned(),
        rule(r).to_owned(),
        status(r).to_owned(),
    ]
}

fn line(r: &Row, sep: &str) -> String {
    fields(r).join(sep)
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
        let mut csv =
            "date,aired_at,ended_at,playlist,artist,title,length,origin,rule,status\n".to_owned();
        for h in &rows {
            let line: Vec<String> = fields(h).iter().map(|f| csv_field(f)).collect();
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

/// Restore page defaults on the History page: the columns of a new station, and the longest
/// look back.
pub(crate) fn restore_defaults(actions: &mut Vec<Action>) {
    for (_, _, key, default) in COLS {
        actions.push(parts::set(parts::HISTORY, key, default));
    }
    actions.push(parts::set(parts::HISTORY, "hours_back", MAX_HOURS_BACK));
}

/// The History page of Preferences.
pub(crate) fn preferences(
    ui: &mut egui::Ui,
    view: &StationView,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let old_shown =
        COLS.map(|(_, _, key, default)| parts::bool(view, parts::HISTORY, key, default));
    let old_hours = parts::u32(view, parts::HISTORY, "hours_back", MAX_HOURS_BACK);
    let mut shown = old_shown;
    let mut hours_back = old_hours;
    settings::section(ui, "Columns", t, |ui| {
        for (i, (_, name, ..)) in COLS.iter().enumerate() {
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
    for (i, (_, _, key, _)) in COLS.iter().enumerate() {
        if shown[i] != old_shown[i] {
            actions.push(parts::set(parts::HISTORY, key, shown[i]));
        }
    }
    if hours_back != old_hours {
        actions.push(parts::set(parts::HISTORY, "hours_back", hours_back));
    }
}
