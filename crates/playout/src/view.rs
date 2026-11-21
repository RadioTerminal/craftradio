//! The snapshot UIs read once a second.

use crate::alarm::RaisedAlarm;
use jiff::Timestamp;
use radio_audio::MeterSnapshot;
use radio_engine::Mode;
use radio_model::ItemId;
use radio_scheduler::Timeline;
use radio_stream::StreamStatus;

/// Everything a UI shows about playout, cheap to take once a second. Meters for 30 fps
/// come from [`crate::Playout::meters`] instead.
#[derive(Debug, Clone)]
pub struct PlayoutView {
    pub now: Timestamp,
    pub mode: Mode,
    /// Stop-after-current is on: nothing new starts after the item on air.
    pub stop_after_current: bool,
    /// In `Assist` mode, the next item is on a deck and waits for `playout.next`.
    pub waiting_for_next: bool,
    /// The emergency fallback folder is playing instead of the plan.
    pub emergency: bool,
    pub decks: [DeckView; 4],
    pub timeline: Timeline,
    pub meters: MeterSnapshot,
    pub outputs: OutputStatus,
    pub stream_states: Vec<StreamState>,
    /// The most recent alarms, oldest first.
    pub alarms: Vec<RaisedAlarm>,
    /// Alarms that a full alarm stream could not take.
    pub alarms_dropped: u64,
}

/// One deck.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeckView {
    pub item: Option<ItemId>,
    pub title: String,
    pub phase: DeckPhase,
    pub role: Option<DeckRole>,
    /// The real start (on air) or the planned start.
    pub start: Option<Timestamp>,
    /// The planned end.
    pub end: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeckPhase {
    #[default]
    Empty,
    Loading,
    Ready,
    /// Playout sent `Play` for an exact sample.
    Cued,
    Playing,
    Paused,
}

/// Why a deck holds its item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckRole {
    /// An entry of the planned timeline.
    Plan,
    /// Loaded by an operator command.
    Operator,
    /// A fill-pool bed under a keep-alive stream that has no audio.
    Bed,
    /// A file from the emergency fallback folder.
    Emergency,
}

/// Where Program goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputStatus {
    pub local: LocalOutput,
    /// The Icecast mount, if the configuration has one.
    pub icecast: Option<StreamStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalOutput {
    Device {
        name: String,
    },
    /// No sound card. A system-clock timer drives the mixer, and nothing plays the audio.
    Null,
    /// Rendered only when [`crate::Playout::render`] asks (tests, offline runs).
    Offline,
}

/// An internet stream on a deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamState {
    pub deck: u8,
    pub item: ItemId,
    pub url: String,
    pub keep_alive: bool,
    /// Audio is arriving.
    pub up: bool,
    pub down_since: Option<Timestamp>,
    pub icy_title: Option<String>,
}
