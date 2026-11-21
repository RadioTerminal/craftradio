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
