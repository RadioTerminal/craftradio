//! Display data that the app makes once per snapshot, not once per row per frame.
//!
//! The snapshot changes at most once a second. Lists draw every frame while the pointer moves.

use std::collections::HashMap;

use crate::app::{hms, is_on_air, mmss};
use crate::view::{Row, StationView};

/// Text and lookups for the rows of one snapshot.
#[derive(Default)]
pub(crate) struct Prepared {
    /// The snapshot that this data comes from.
    pub generation: u64,
    /// The index in `StationView::rows` of each row id.
    pub index: HashMap<u64, usize>,
    /// The row ids, in order, for range selection.
    pub ids: Vec<u64>,
    /// The start time of each row, as HH:MM:SS, with ⏱ in front for a fixed row.
    pub start: Vec<String>,
    /// The length of each row, as m:ss.
    pub len: Vec<String>,
    /// The divider text of each row from another playlist.
    pub divider: Vec<String>,
    /// The indexes of the rows in the Upcoming list.
    pub upcoming: Vec<usize>,
    pub on_air: Option<usize>,
    pub main_count: usize,
}

fn divider_text(r: &Row) -> String {
    if r.title == r.playlist {
        format!("{} ({})", r.playlist, mmss(r.len))
    } else {
        format!("{} · {} ({})", r.playlist, r.title, mmss(r.len))
    }
}

fn start_text(r: &Row) -> String {
    let mark = if r.fixed.is_some() { "⏱" } else { "" };
    format!("{mark}{}", hms(&r.start))
}

impl Prepared {
    pub fn new(view: &StationView, generation: u64) -> Self {
        let now = view.now.timestamp();
        let rows = &view.rows;
        Self {
            generation,
            index: rows.iter().enumerate().map(|(i, r)| (r.id, i)).collect(),
            ids: rows.iter().map(|r| r.id).collect(),
            start: rows.iter().map(start_text).collect(),
            len: rows.iter().map(|r| mmss(r.len)).collect(),
            divider: rows
                .iter()
                .map(|r| {
                    if r.is_main {
                        String::new()
                    } else {
                        divider_text(r)
                    }
                })
                .collect(),
            upcoming: rows
                .iter()
                .enumerate()
                .filter(|(_, r)| !r.is_main && crate::app::row_end(r) > now)
                .map(|(i, _)| i)
                .collect(),
            on_air: rows.iter().position(|r| is_on_air(r, now)),
            main_count: rows.iter().filter(|r| r.is_main).count(),
        }
    }
}
