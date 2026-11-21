//! Read and write the settings of each part of the app. The view source keeps them.
//!
//! Each read has a default, so the UI also shows a source that has no settings yet.

use serde_json::Value;

use crate::app::Action;
use crate::view::{Edit, StationView};

pub(crate) const CLOCK: &str = "clock";
pub(crate) const HISTORY: &str = "history";
pub(crate) const LIBRARY: &str = "library";
pub(crate) const CARTWALL: &str = "cartwall";
pub(crate) const MIXER: &str = "mixer";
pub(crate) const DECKS: &str = "decks";

fn get<'a>(view: &'a StationView, part: &str, key: &str) -> Option<&'a Value> {
    view.settings.get(part)?.get(key)
}

pub(crate) fn bool(view: &StationView, part: &str, key: &str, default: bool) -> bool {
    get(view, part, key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

pub(crate) fn f32(view: &StationView, part: &str, key: &str, default: f32) -> f32 {
    get(view, part, key)
        .and_then(Value::as_f64)
        .map_or(default, |v| v as f32)
}

pub(crate) fn u32(view: &StationView, part: &str, key: &str, default: u32) -> u32 {
    get(view, part, key)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(default)
}

pub(crate) fn strings(view: &StationView, part: &str, key: &str, default: &[&str]) -> Vec<String> {
    match get(view, part, key).and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        None => default.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// The action that stores one setting. It shows a message only when it fails.
pub(crate) fn set(part: &str, key: &str, value: impl Into<Value>) -> Action {
    Action::EditQuiet(Edit::SetPartSetting {
        part: part.to_owned(),
        key: key.to_owned(),
        value: value.into(),
    })
}

/// Save resized columns only after the separator drag ends.
pub(crate) fn save_column_widths(
    ui: &egui::Ui,
    table: egui::Id,
    part: &str,
    columns: &[(&str, f32)],
    actions: &mut Vec<Action>,
) {
    let stopped = columns.iter().enumerate().any(|(i, _)| {
        ui.ctx()
            .read_response(table.with("resize_column").with(i))
            .is_some_and(|response| response.drag_stopped())
    });
    if stopped {
        for (key, width) in columns {
            if !key.is_empty() {
                actions.push(set(part, key, *width));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn column_resize_saves_only_after_release() {
        let mut harness = Harness::builder().build_ui_state(
            |ui, actions: &mut Vec<Action>| {
                let table_id = ui.id().with(egui::IdSalt::new("width_test"));
                let mut widths = Vec::new();
                egui_extras::TableBuilder::new(ui)
                    .id_salt("width_test")
                    .column(egui_extras::Column::initial(180.0).resizable(true))
                    .column(egui_extras::Column::remainder())
                    .header(24.0, |mut row| {
                        row.col(|ui| {
                            ui.label("Title");
                        });
                        row.col(|ui| {
                            ui.label("Artist");
                        });
                    })
                    .body(|body| {
                        widths = body.widths().to_vec();
                    });
                save_column_widths(
                    ui,
                    table_id,
                    LIBRARY,
                    &[("width_title", widths[0])],
                    actions,
                );
            },
            Vec::new(),
        );
        harness.run_steps(3);
        let artist = harness.get_by_label("Artist").rect();
        let before = artist.left();
        let start = egui::pos2(before - 4.0, artist.center().y);
        let end = start + egui::vec2(48.0, 0.0);
        harness.hover_at(start);
        harness.run_steps(1);
        harness.drag_at(start);
        harness.run_steps(1);
        harness.hover_at(end);
        harness.run_steps(2);
        assert!(harness.get_by_label("Artist").rect().left() > before);
        assert!(
            harness.state().is_empty(),
            "a drag must not write every frame"
        );
        harness.drop_at(end);
        harness.run_steps(3);
        assert_eq!(harness.state().len(), 1, "release writes once");
        let Action::EditQuiet(Edit::SetPartSetting { part, key, value }) = &harness.state()[0]
        else {
            panic!("resize must use a part setting command");
        };
        assert_eq!(part, LIBRARY);
        assert_eq!(key, "width_title");
        assert!(value.as_f64().expect("numeric width") > 180.0);
    }
}
