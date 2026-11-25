//! The snapshot UIs read once a second.

use crate::alarm::RaisedAlarm;
use jiff::Timestamp;
use radio_audio::MeterSnapshot;
use radio_engine::Mode;
use radio_engine::State;
use radio_model::ItemId;
use radio_scheduler::Timeline;
use radio_stream::{StreamStatus, WebRtcStatus};
use std::path::PathBuf;
use std::sync::Arc;

/// Everything a UI shows about playout, cheap to take once a second. Meters for 30 fps
/// come from [`crate::Playout::meters`] instead.
#[derive(Debug, Clone)]
pub struct PlayoutView {
    pub monitor_open: bool,
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
    /// The station state of the engine, as the last plan used it.
    pub state: Arc<State>,
    /// The intro end and outro start that playout's analysis found, by item, for songs
    /// that the engine holds no markers for. The plan uses them.
    pub found_markers: std::collections::BTreeMap<
        radio_model::ItemId,
        (Option<std::time::Duration>, Option<std::time::Duration>),
    >,
    /// Counts the changes of `state`, so a UI rebuilds what depends on it only when it
    /// changed.
    pub revision: u64,
    /// The number of plans that playout made so far.
    pub plans: u64,
    /// Control ticks that exceeded half of the command lead.
    pub tick_overruns: u64,
}

/// One deck.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeckView {
    pub item: Option<ItemId>,
    pub title: String,
    pub phase: DeckPhase,
    pub role: Option<DeckRole>,
    /// The real start (on air) or the planned start.
    pub start: Option<Timestamp>,
    /// The planned end.
    pub end: Option<Timestamp>,
    /// Where the item is at [`PlayoutView::now`], counted from its cue-in. On a deck that
    /// waits, this is where it will start.
    pub position: std::time::Duration,
    /// The position moves with the clock: the item plays, and no seek waits.
    pub running: bool,
    /// The planned length from the cue-in to the planned end.
    pub len: std::time::Duration,
    /// `deck.seek` works on this deck now: it holds a file that is loaded, and its end
    /// fade did not begin.
    pub seekable: bool,
    /// The item gain that playout sent to the mixer for a file item: the loudness gain
    /// with the trim and the clamps. `None` for an empty deck, a stream or live input.
    pub gain: Option<radio_model::ItemGain>,
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
    /// The WebRTC listener server while it runs.
    pub webrtc: Option<WebRtcStatus>,
    /// The configuration has a WebRTC server, so the setting can turn it on.
    pub webrtc_available: bool,
    /// The On air setting. Off air, neither Icecast nor WebRTC sends anything; the decks
    /// and the sound card go on.
    pub on_air: bool,
    /// The Icecast switch (`icecast.enabled`). Switched off, Icecast sends nothing even on
    /// air; WebRTC has a switch of its own.
    pub icecast_enabled: bool,
    /// What plays now as listeners see it, for the cover a paired server shows. `None`
    /// before the first item that shows anything.
    pub now: Option<NowShown>,
}

/// The item on air as listeners see it: a revision that changes with each item, and the
/// file that gives its cover (`None` when it shows none, as on WebRTC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowShown {
    pub rev: u64,
    pub cover: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalOutput {
    /// The device is lost; Icecast continues with software pacing.
    Lost {
        name: String,
    },
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
