//! Display data that the app makes once per snapshot, not once per row per frame.
//!
//! The snapshot changes at most once a second. Lists draw every frame while the pointer moves.

use std::collections::HashMap;

use crate::app::{hms, is_on_air, mmss};
use crate::icons::{self, Icon};
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
    /// The start time of each row, as HH:MM:SS, with a timer icon in front for a fixed row.
    pub start: Vec<String>,
    /// The start time of each row, as HH:MM:SS alone. The main pane draws the timer in its own slot.
    pub time: Vec<String>,
    /// The length of each row, as m:ss.
    pub len: Vec<String>,
    /// The divider text of each row from another playlist.
    pub divider: Vec<String>,
    /// The indexes of the rows in the Upcoming list.
    pub upcoming: Vec<usize>,
    pub on_air: Option<usize>,
    pub main_count: usize,
    /// The number of decks in use. The row on air and the next rows sit on them.
    decks: usize,
}

/// The divider text. The length goes in the LENGTH column, like a track's.
fn divider_text(r: &Row) -> String {
    if r.title == r.playlist {
        r.playlist.clone()
    } else {
        format!("{} · {}", r.playlist, r.title)
    }
}

/// The start time, with a timer icon and a thin space in front for a fixed row.
fn start_text(r: &Row) -> String {
    let mark = if r.fixed.is_some() {
        icons::lead(Icon::Timer, "")
    } else {
        String::new()
    };
    format!("{mark}{}", hms(&r.start))
}

impl Prepared {
    /// Why a row cannot be removed: it is on air, or on a deck. `None` for other rows.
    pub fn protected(&self, id: u64) -> Option<String> {
        let on_air = self.on_air?;
        let i = *self.index.get(&id)?;
        if i == on_air {
            return Some("this row is on air; use Play next instead".to_owned());
        }
        let k = i.checked_sub(on_air).filter(|k| *k < self.decks)?;
        let deck = u8::try_from(k).map_or('?', |k| char::from(b'A' + k));
        Some(format!("this row is loaded on deck {deck}"))
    }

    pub fn new(view: &StationView, generation: u64) -> Self {
        let now = view.now.timestamp();
        let rows = &view.rows;
        Self {
            generation,
            index: rows.iter().enumerate().map(|(i, r)| (r.id, i)).collect(),
            ids: rows.iter().map(|r| r.id).collect(),
            start: rows.iter().map(start_text).collect(),
            time: rows.iter().map(|r| hms(&r.start)).collect(),
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
            decks: crate::parts::deck_count(view),
        }
    }
}
