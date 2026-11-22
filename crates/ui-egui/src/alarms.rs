//! The Alarms window: the current alarms and warnings, the new ones first, then the newest.
//! A person can filter them, acknowledge them, copy them and export them as CSV.
//! The table draws only the visible rows.

use egui::{Align, Color32, Layout, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, hms};
use crate::icons::{self, Icon};
use crate::parts;
use crate::preferences::Page;
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{AlarmView, Edit, Severity, StationView};

/// The view state of the window: the filters.
#[derive(Default)]
pub(crate) struct State {
    filter: String,
    /// The severities that a person switched off, in the order of `Severity::ALL`.
    hidden: [bool; 3],
}

/// The icon of a severity. The word always comes with it.
pub(crate) fn icon(s: Severity) -> Icon {
    match s {
        Severity::Alarm => Icon::OctagonAlert,
        Severity::Warning => Icon::TriangleAlert,
        Severity::Info => Icon::Info,
    }
}

/// The colour of a severity.
pub(crate) fn colour(s: Severity, t: &Tokens) -> Color32 {
    match s {
        Severity::Alarm => t.danger,
        Severity::Warning => t.warn,
        Severity::Info => t.text_muted,
    }
}

fn index(s: Severity) -> usize {
    Severity::ALL.iter().position(|x| *x == s).unwrap_or(0)
}

/// One alarm as a line of text, with the fields apart by `sep`.
fn line(a: &AlarmView, sep: &str) -> String {
    [
        hms(&a.at),
        a.severity.label().to_owned(),
        a.source.label().to_owned(),
        a.text.clone(),
        a.concerns.clone().unwrap_or_default(),
        if a.acked { "acknowledged" } else { "new" }.to_owned(),
    ]
    .join(sep)
}

/// The alarms in the order of the window: new before acknowledged, then newest first.
fn sorted(view: &StationView) -> Vec<&AlarmView> {
    let mut list: Vec<&AlarmView> = view.alarms.iter().collect();
    list.sort_by(|a, b| a.acked.cmp(&b.acked).then_with(|| b.at.cmp(&a.at)));
    list
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let show_acked = parts::bool(view, parts::ALARMS, "show_acked", true);
    let counts = Severity::ALL.map(|s| view.open_alarms().filter(|a| a.severity == s).count());
    settings::header_to(ui, "Alarms", Page::Alarms, actions, |ui| {
        let words: Vec<String> = Severity::ALL
            .iter()
            .zip(counts)
            .map(|(s, n)| s.count_label(n))
            .collect();
        let text = format!("New: {}", words.join(" · "));
        ui.label(
            RichText::new(text)
                .font(theme::text(theme::LABEL))
                .color(t.text_muted),
        );
    });
    if view.preview {
        ui.label(
            RichText::new("preview — sample alarms, not from a station")
                .font(theme::text(theme::CAPTION))
                .color(t.text_muted),
        );
    }

    let needle = state.filter.to_lowercase();
    let rows: Vec<&AlarmView> = sorted(view)
        .into_iter()
        .filter(|a| !state.hidden[index(a.severity)])
        .filter(|a| show_acked || !a.acked)
        .filter(|a| {
            needle.is_empty()
                || a.text.to_lowercase().contains(&needle)
                || a.source.label().to_lowercase().contains(&needle)
                || a.concerns
                    .as_ref()
                    .is_some_and(|c| c.to_lowercase().contains(&needle))
        })
        .collect();
    let new: Vec<u64> = rows.iter().filter(|a| !a.acked).map(|a| a.id).collect();

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        for s in Severity::ALL {
            let i = index(s);
            let n = view.alarms.iter().filter(|a| a.severity == s).count();
            let text = RichText::new(icons::lead(icon(s), &format!("{} {n}", s.label())))
                .font(theme::text(theme::LABEL))
                .color(colour(s, t));
            let resp = ui
                .add(egui::Button::new(text).selected(!state.hidden[i]))
                .on_hover_text(format!("Show or hide the {} rows", s.label()));
            if resp.clicked() {
                state.hidden[i] = !state.hidden[i];
            }
        }
        ui.add_space(space::S);
        let edit = egui::TextEdit::singleline(&mut state.filter)
            .hint_text("Filter by message, source or item");
        ui.add(edit.desired_width(size::FIELD_W))
            .on_hover_text("Alarms filter");
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        let all = ui
            .add_enabled(
                !new.is_empty(),
                egui::Button::new(icons::lead(Icon::CheckCheck, "Acknowledge all")),
            )
            .on_hover_text("Acknowledge every new alarm on show")
            .on_disabled_hover_text("No new alarm on show");
        if all.clicked() {
            actions.push(Action::Edit(Edit::AckAlarms(new.clone())));
        }
        let copy = ui
            .add_enabled(
                !rows.is_empty(),
                egui::Button::new(icons::lead(Icon::Copy, "Copy")),
            )
            .on_hover_text("Copy the alarms on show as text");
        if copy.clicked() {
            let text: Vec<String> = rows.iter().map(|a| line(a, "\t")).collect();
            ui.ctx().copy_text(text.join("\n"));
            actions.push(Action::Toast(format!("copied {} alarms", rows.len())));
        }
        let export = ui
            .add_enabled(
                !rows.is_empty(),
                egui::Button::new(icons::lead(Icon::FileDown, "Export CSV…")),
            )
            .on_hover_text("Write the alarms on show to a CSV file");
        if export.clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("alarms.csv")
                .add_filter("CSV", &["csv"])
                .save_file()
        {
            actions.push(Action::Edit(Edit::ExportAlarms {
                path: path.display().to_string(),
                ids: rows.iter().map(|a| a.id).collect(),
            }));
        }
    });
    ui.add_space(space::S);

    if rows.is_empty() {
        let text = if view.alarms.is_empty() {
            "No alarms or warnings."
        } else {
            "No alarm matches the filter."
        };
        ui.label(
            RichText::new(text)
                .font(theme::text(theme::SMALL))
                .color(t.text_muted),
        );
    }
    let head = |s: &str| {
        RichText::new(s)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    egui::ScrollArea::horizontal()
        .id_salt("alarms_columns")
        .auto_shrink(false)
        .show(ui, |ui| {
            // The rows sense no click: the buttons and links inside them take the clicks.
            TableBuilder::new(ui)
                .id_salt("alarms_table")
                .striped(true)
                .auto_shrink(false)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(size::COL_LEN))
                .column(Column::exact(size::COL_SEVERITY))
                .column(Column::exact(size::COL_SOURCE))
                .column(
                    Column::initial(size::COL_MESSAGE)
                        .at_least(size::TEXT_COL_MIN)
                        .at_most(size::TEXT_COL_MAX)
                        .resizable(true)
                        .clip(true),
                )
                .column(
                    Column::initial(size::COL_ARTIST)
                        .at_least(size::TEXT_COL_MIN)
                        .at_most(size::TEXT_COL_MAX)
                        .resizable(true)
                        .clip(true),
                )
                .column(Column::exact(size::COL_ACK))
                .header(size::DIVIDER_ROW, |mut h| {
                    for name in ["TIME", "SEVERITY", "SOURCE", "MESSAGE", "CONCERNS", ""] {
                        h.col(|ui| {
                            ui.label(head(name));
                        });
                    }
                })
                .body(|body| {
                    body.rows(size::ROW, rows.len(), |mut row| {
                        let Some(a) = rows.get(row.index()) else {
                            return;
                        };
                        let ink = if a.acked { t.text_muted } else { t.text };
                        row.col(|ui| {
                            ui.label(
                                RichText::new(hms(&a.at))
                                    .font(theme::mono(theme::CODE))
                                    .color(ink),
                            );
                        });
                        row.col(|ui| {
                            let c = if a.acked {
                                t.text_muted
                            } else {
                                colour(a.severity, t)
                            };
                            ui.label(
                                RichText::new(icons::lead(icon(a.severity), a.severity.label()))
                                    .font(theme::text(theme::LABEL))
                                    .color(c),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(a.source.label())
                                    .font(theme::text(theme::LABEL))
                                    .color(ink),
                            );
                        });
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&a.text)
                                        .font(theme::text(theme::SMALL))
                                        .color(ink),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&a.text);
                        });
                        row.col(|ui| concerns(ui, view, a, ink, actions));
                        row.col(|ui| {
                            if a.acked {
                                ui.label(
                                    RichText::new("Acknowledged")
                                        .font(theme::text(theme::CAPTION))
                                        .color(t.text_muted),
                                );
                            } else if ui
                                .small_button("Acknowledge")
                                .on_hover_text("It stops counting as new")
                                .clicked()
                            {
                                actions.push(Action::EditQuiet(Edit::AckAlarm(a.id)));
                            }
                        });
                    });
                });
        });
}

/// The item or playlist that an alarm concerns. A click selects the item in Main and Upcoming.
fn concerns(
    ui: &mut egui::Ui,
    view: &StationView,
    a: &AlarmView,
    ink: Color32,
    actions: &mut Vec<Action>,
) {
    let Some(name) = &a.concerns else {
        return;
    };
    let text = RichText::new(name).font(theme::text(theme::SMALL));
    let row = a.item.filter(|id| view.rows.iter().any(|r| r.id == *id));
    match row {
        Some(id) => {
            let link = egui::Link::new(text.color(ink));
            if ui
                .add(link)
                .on_hover_text("Select the item in Main and Upcoming")
                .clicked()
            {
                actions.push(Action::SelectRow(id));
            }
        }
        None => {
            ui.add(egui::Label::new(text.color(ink)).truncate());
        }
    }
}

/// The Alarms page of Preferences.
pub(crate) fn preferences(
    ui: &mut egui::Ui,
    view: &StationView,
    actions: &mut Vec<Action>,
    _t: &Tokens,
) {
    let old = parts::bool(view, parts::ALARMS, "show_acked", true);
    let mut show_acked = old;
    ui.checkbox(&mut show_acked, "Show acknowledged alarms");
    if show_acked != old {
        actions.push(parts::set(parts::ALARMS, "show_acked", show_acked));
    }
}
