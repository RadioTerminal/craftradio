//! The view-model the panels draw from. The UI depends only on these types, so a later
//! adapter can map the real scheduler into a [`StationView`] without touching the panels.

use std::time::Duration;

/// What a deck is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckState {
    Playing,
    Ready,
    Empty,
}

/// One of the four decks.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckView {
    pub label: char,
    pub title: String,
    pub elapsed: Duration,
    pub len: Duration,
    pub state: DeckState,
}

/// How well a fixed-time item fits its slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitView {
    pub error_s: f32,
    pub ok: bool,
}

/// One row of the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub start: jiff::Zoned,
    pub playlist: String,
    pub color: [u8; 3],
    pub title: String,
    pub len: Duration,
    pub is_main: bool,
    pub fixed: Option<FitView>,
}

/// A named playlist's tab in the scheduled pane.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistTab {
    pub name: String,
    pub color: [u8; 3],
    pub pick: String,
    pub rules: Vec<String>,
    pub items: Vec<(String, Duration)>,
}

/// Everything the window shows at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct StationView {
    pub now: jiff::Zoned,
    pub mode: String,
    pub decks: [DeckView; 4],
    pub rows: Vec<Row>,
    pub tabs: Vec<PlaylistTab>,
    pub carts: Vec<String>,
    pub warnings: usize,
}

/// Something that can describe the station at a given instant.
pub trait ViewSource {
    fn snapshot(&mut self, now: jiff::Timestamp) -> StationView;
}
