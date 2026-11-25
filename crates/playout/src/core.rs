//! The control thread: plan, load, start, end and log. Everything here runs on one thread.

use crate::alarm::{Alarm, Alarms, RaisedAlarm};
use crate::clock::{Clock, ClockMap};
use crate::deadair::{Analysis, DeadAir};
use crate::ops::{Carts, Check, Pending};
use crate::output::{ProgramSource, feed_tap, feed_taps};
use crate::streams::{NowPlaying, StreamSlot};
use crate::view::{DeckPhase, DeckRole, DeckView, LocalOutput, OutputStatus, PlayoutView};
use crate::work::{Done, Workers};
use crate::{OutputChoice, PlayoutConfig, PlayoutError, Reply};
use jiff::Timestamp;
use radio_audio::Decoder;
use radio_audio::{
    AudioEvent, CHANNELS, Command as Cmd, CommandKind, DeckId, DeckState, LiveInput, LoadOptions,
    MeterReader, Mixer, MixerConfig, MixerHandle, ProgramDynamics, SAMPLE_RATE,
};
use radio_audio_device::{NullOutput, SampleClock};
use radio_engine::{Command, Engine, EngineEvent, LogEntry, LogHandle, LogStatus, RowId};
use radio_model::{FixMode, Item, ItemId, Source};
use radio_scheduler::{Entry, Origin, PlanInput, PlayHistory, Played, Timeline, plan};
use radio_stream::{IcecastClient, IcecastHandle, TapSlot, WebRtcHandle};
use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// An entry taken off a deck for a hard event. It cannot load again before `until`.
struct Blocked {
    item: ItemId,
    start: Timestamp,
    until: Timestamp,
}

/// Tells if two entries are the same fixed event: the same playlist, item and target.
fn same_event(a: &Entry, b: &Entry) -> bool {
    let target = |e: &Entry| e.fixed.as_ref().map(|f| f.target);
    a.playlist == b.playlist && a.item == b.item && target(a) == target(b)
}

fn is_hard_entry(e: &Entry) -> bool {
    e.fixed.as_ref().is_some_and(|f| f.mode == FixMode::Hard)
}

/// Tells if two entries are the same occurrence of a rule: the same rule and target. The
/// item may differ.
fn same_occurrence(a: &Entry, b: &Entry) -> bool {
    let target = |e: &Entry| e.fixed.as_ref().map(|f| f.target);
    a.origin == b.origin && target(a).is_some() && target(a) == target(b)
}

/// The previous plan without what the fresh plan no longer has: fixed events and their
/// fits, and the entries of rules that are gone.
fn still_planned(
    prev: &Timeline,
    old: Option<&radio_model::Station>,
    new: &radio_model::Station,
) -> Timeline {
    let mut kept = prev.clone();
    let unchanged = |e: &Entry| match e.origin {
        Origin::Rule { playlist, rule } => old
            .and_then(|s| rules_of(s, playlist))
            .zip(rules_of(new, playlist))
            .is_some_and(|(a, b)| a.get(rule) == b.get(rule) && b.get(rule).is_some()),
        _ => true,
    };
    kept.entries.retain(|e| {
        unchanged(e)
            && new
                .playlists
                .iter()
                .any(|p| p.enabled && p.items.iter().any(|i| i.id == e.item))
    });
    kept.fits.retain(|fit| {
        kept.entries.iter().any(|e| {
            e.playlist == fit.playlist && e.fixed.as_ref().is_some_and(|f| f.target == fit.target)
        })
    });
    kept
}

/// An occurrence that a late edit moved a locked hard event to. It does not air, because
/// the locked event (`stand_in`) airs at its old time instead.
#[derive(Clone)]
pub(crate) struct Superseded {
    entry: Entry,
    stand_in: Entry,
}

/// Tells if a station edit renumbered the rules of `origin`'s playlist, or changed any
/// rule of it other than `origin`'s own. Then the rule index of `origin` may name another
/// rule now, so an occurrence of that index is not a move of `origin`'s event.
fn renumbered(
    old: Option<Vec<radio_model::Rule>>,
    new: &radio_model::Station,
    origin: &Origin,
) -> bool {
    let Origin::Rule { playlist, rule } = origin else {
        return true;
    };
    match (old, rules_of(new, *playlist)) {
        (Some(o), Some(n)) => {
            o.len() != n.len()
                || o.iter()
                    .zip(&n)
                    .enumerate()
                    .any(|(i, (a, b))| i != *rule && a != b)
        }
        _ => true,
    }
}

/// The rule list of a playlist.
fn rules_of(
    s: &radio_model::Station,
    playlist: radio_model::PlaylistId,
) -> Option<Vec<radio_model::Rule>> {
    s.playlists
        .iter()
        .find(|p| p.id == playlist)
        .map(|p| p.rules.clone())
}

/// Takes the superseded occurrences out of a plan, with the rest of their take after them.
fn drop_superseded(tl: &mut Timeline, superseded: &[Superseded]) {
    if superseded.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(tl.entries.len());
    let mut skipping: Option<Origin> = None;
    for e in std::mem::take(&mut tl.entries) {
        if skipping
            .as_ref()
            .is_some_and(|o| e.fixed.is_none() && e.origin == *o)
        {
            continue;
        }
        skipping = None;
        if superseded.iter().any(|x| same_occurrence(&x.entry, &e)) {
            skipping = Some(e.origin.clone());
            continue;
        }
        out.push(e);
    }
    tl.entries = out;
    tl.fits.retain(|fit| {
        !superseded.iter().any(|x| {
            x.entry.playlist == fit.playlist
                && x.entry
                    .fixed
                    .as_ref()
                    .is_some_and(|f| f.target == fit.target)
        })
    });
}

/// In the last 10 s before its target, and after it, planning never cancels a hard event.
pub(crate) const HARD_LOCK: Duration = Duration::from_secs(10);

/// Playout plans again at least this often.
const PLAN_EVERY: Duration = Duration::from_secs(1);
/// The most planned entries that wait on decks: every deck but the one on air holds what
/// comes next.
const MAX_QUEUED: usize = 3;
/// Playout tries a broken item again after this time.
const RETRY_BROKEN: Duration = Duration::from_secs(300);
/// The fade of an early end when the item has no fade-out of its own.
pub(crate) const CUT_FADE: Duration = Duration::from_secs(1);

pub(crate) fn add(t: Timestamp, d: Duration) -> Timestamp {
    t.checked_add(d).unwrap_or(Timestamp::MAX)
}

pub(crate) fn sub(t: Timestamp, d: Duration) -> Timestamp {
    t.checked_sub(d).unwrap_or(Timestamp::MIN)
}

/// The time from `a` to `b`, or zero when `b` is not after `a`.
pub(crate) fn since(a: Timestamp, b: Timestamp) -> Duration {
    b.duration_since(a).try_into().unwrap_or(Duration::ZERO)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// A worker opens the file.
    Opening,
    Loading,
    Ready,
    /// `Play` waits in the mixer for this frame.
    Cued(u64),
    Playing,
    Paused,
}

/// What playout put on a deck.
pub(crate) struct Slot {
    pub(crate) tag: u64,
    pub(crate) entry: Entry,
    pub(crate) item: Item,
    pub(crate) role: DeckRole,
    pub(crate) phase: Phase,
    /// The real start on air.
    pub(crate) started: Option<Timestamp>,
    /// When the item first went on air. A pause or a seek moves `started`, and never this,
    /// so it gives the order of the items on air.
    pub(crate) aired: Option<Timestamp>,
    /// When the operator paused the item, while it is paused.
    pub(crate) paused_at: Option<Timestamp>,
    /// Playout sent the fade at the planned end.
    pub(crate) end_sent: bool,
    /// Something ended the item before its planned end.
    pub(crate) cut: bool,
    pub(crate) row: Option<RowId>,
    pub(crate) stream: Option<StreamSlot>,
    pub(crate) capture: Option<Box<dyn crate::Capture>>,
    live_end: Option<Timestamp>,
    /// The load starts this far after the cue-in (a resume or a seek).
    pub(crate) offset: Duration,
    /// A seek that the mixer did not take yet: its id and its position from the cue-in.
    pub(crate) seek: Option<(u64, Duration)>,
    /// The seek with this id did not happen: its file did not open.
    pub(crate) seek_failed: Option<u64>,
    /// A worker still opens the file for `seek`.
    pub(crate) reopening: bool,
    /// A seek set `offset` before the start, so the entry still starts when the plan says
    /// and ends sooner by `offset`.
    pub(crate) seeked: bool,
    /// The mixer reported that a fade-out began.
    pub(crate) fading: bool,
    /// Start as soon as the deck is ready, whatever the plan says.
    pub(crate) asap: bool,
    pub(crate) options: LoadOptions,
    /// The item gain that playout sent to the mixer (or puts in the load), for a file item.
    pub(crate) gain: Option<radio_model::ItemGain>,
    /// The loudness from the tags of the file, once the worker read them.
    pub(crate) tagged: Option<radio_model::Loudness>,
}

impl Slot {
    pub(crate) fn not_started(&self) -> bool {
        matches!(self.phase, Phase::Opening | Phase::Loading | Phase::Ready)
    }

    /// The plan keeps this entry where it is: a plan entry on a deck, or an operator item
    /// on air.
    pub(crate) fn pinned(&self) -> bool {
        self.role == DeckRole::Plan || (self.role == DeckRole::Operator && !self.not_started())
    }

    /// Tells if this is a soft event and `now` is before its target less its tolerance.
    /// Then it may not start yet.
    pub(crate) fn soft_too_early(&self, now: Timestamp) -> bool {
        self.entry
            .fixed
            .as_ref()
            .is_some_and(|f| f.mode == FixMode::Soft && now < sub(f.target, f.tolerance))
    }

    pub(crate) fn is_hard(&self) -> bool {
        self.entry
            .fixed
            .as_ref()
            .is_some_and(|f| f.mode == FixMode::Hard)
    }

    /// Where the item really started at its cue-in, once it is on air. A hard event keeps
    /// its target in `entry.start`, so a late one starts after that.
    pub(crate) fn real_start(&self) -> Timestamp {
        match self.started {
            Some(w) => sub(w, self.offset),
            // A deck that waits after a seek starts in the middle of its item.
            None if self.seeked => sub(self.entry.start, self.offset),
            None => self.entry.start,
        }
    }

    /// The planned length, from the cue-in to the planned end.
    pub(crate) fn planned_len(&self) -> Duration {
        match self.live_end {
            // A live input ends on the clock, whenever it started.
            Some(end) => since(self.real_start(), end),
            None => self.entry.len.saturating_sub(self.entry.trim),
        }
    }

    /// A seek works on this slot now: a file that is loaded, before its end fade.
    pub(crate) fn seekable(&self) -> bool {
        matches!(self.item.source, Source::File { .. })
            && !matches!(self.phase, Phase::Opening | Phase::Loading)
            && !self.end_sent
            && !self.fading
    }

    /// Where the item is at `now`, counted from its cue-in, and whether that moves with
    /// the clock. A seek that waits shows its target, so the position does not jump back.
    pub(crate) fn position(&self, now: Timestamp) -> (Duration, bool) {
        let (at, running) = match (self.seek, self.phase) {
            (Some((_, to)), _) => (to, false),
            (None, Phase::Playing) => (since(self.real_start(), now), true),
            (None, Phase::Paused) => {
                let at = self.paused_at.unwrap_or(now);
                (since(self.real_start(), at), false)
            }
            (None, _) => (self.offset, false),
        };
        (at.min(self.planned_len()), running)
    }

    /// The planned end: the real start plus the length, less the trim of the planner.
    pub(crate) fn planned_end(&self) -> Timestamp {
        self.live_end
            .unwrap_or_else(|| add(self.real_start(), self.planned_len()))
    }
}

enum Driver {
    Offline {
        mixer: Box<Mixer>,
        taps: Vec<rtrb::Producer<f32>>,
        cue: Option<rtrb::Producer<f32>>,
    },
    Null {
        _out: NullOutput,
    },
    Device {
        out: crate::device::Device,
    },
}

struct OpenedOutput {
    /// The clock of a real or null output, for its latency.
    clock: Option<SampleClock>,
    driver: Driver,
    handle: MixerHandle,
    live: LiveInput,
    cue: crate::monitor::Monitor,
    local: LocalOutput,
    taps: Vec<rtrb::Consumer<f32>>,
    fallback: Option<String>,
}

/// Four seconds of Program between the mixer and each encoder.
const TAP_SAMPLES: usize = 4 * SAMPLE_RATE as usize * CHANNELS;

type Taps = (Vec<rtrb::Producer<f32>>, Vec<rtrb::Consumer<f32>>);

fn tap_rings(count: usize) -> Taps {
    (0..count)
        .map(|_| rtrb::RingBuffer::<f32>::new(TAP_SAMPLES))
        .unzip()
}

/// Opens the output that `choice` asks for. If a device fails or does not run at 48
/// kHz, the null output runs instead. It returns the reason too.
fn open_output(
    choice: &OutputChoice,
    config: MixerConfig,
    dynamics: Option<ProgramDynamics>,
    taps: usize,
    device_meter: &Arc<radio_stream::LevelMeter>,
) -> Result<OpenedOutput, PlayoutError> {
    let (mut mixer, handle, live) = Mixer::new(config);
    mixer.set_dynamics(dynamics)?;
    let (tap_tx, tap_rx) = tap_rings(taps);
    let mut fallback = None;
    let (cue_tx, cue_rx) = rtrb::RingBuffer::new(48_000 * 2);
    let mut cue = crate::monitor::Monitor::start(cue_rx);
    let mut cue_tx = Some(cue_tx);
    let (mixer, handle, live, tap_tx, tap_rx) = match choice {
        OutputChoice::Offline => {
            return Ok(OpenedOutput {
                clock: None,
                driver: Driver::Offline {
                    mixer: Box::new(mixer),
                    taps: tap_tx,
                    cue: cue_tx.take(),
                },
                handle,
                live,
                cue,
                local: LocalOutput::Offline,
                taps: tap_rx,
                fallback,
            });
        }
        OutputChoice::Device { name } => {
            let mut source = ProgramSource::new(mixer, tap_tx);
            source.cue = cue_tx.take();
            source.device_meter = Some(device_meter.clone());
            let source = Box::new(source);
            match crate::device::Device::open(name.clone(), source) {
                Ok((out, name, clock)) => {
                    let local = LocalOutput::Device { name };
                    return Ok(OpenedOutput {
                        clock: Some(clock),
                        driver: Driver::Device { out },
                        handle,
                        live,
                        cue,
                        local,
                        taps: tap_rx,
                        fallback,
                    });
                }
                Err(e) => fallback = Some(e.to_string()),
            }
            let (new_tx, new_rx) = rtrb::RingBuffer::new(48_000 * 2);
            cue_tx = Some(new_tx);
            cue = crate::monitor::Monitor::start(new_rx);
            let (mut mixer, handle, live) = Mixer::new(config);
            mixer.set_dynamics(dynamics)?;
            let (tap_tx, tap_rx) = tap_rings(taps);
            (mixer, handle, live, tap_tx, tap_rx)
        }
        OutputChoice::Null => (mixer, handle, live, tap_tx, tap_rx),
    };
    let mut source = ProgramSource::new(mixer, tap_tx);
    source.cue = cue_tx;
    let source = Box::new(source);
    let null = NullOutput::start(SAMPLE_RATE, CHANNELS as u16, source)
        .map_err(|e| PlayoutError::Output(e.to_string()))?;
    Ok(OpenedOutput {
        clock: Some(null.clock()),
        driver: Driver::Null { _out: null },
        handle,
        live,
        cue,
        local: LocalOutput::Null,
        taps: tap_rx,
        fallback,
    })
}

pub(crate) struct Core {
    pub(crate) engine: Engine,
    engine_rx: Receiver<EngineEvent>,
    pub(crate) mixer: MixerHandle,
    driver: Driver,
    _live: LiveInput,
    cue: crate::monitor::Monitor,
    cue_name: String,
    local: LocalOutput,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) map: ClockMap,
    pub(crate) cfg: PlayoutConfig,
    pub(crate) timeline: Timeline,
    planned: bool,
    tick_overruns: u64,
    timeline_changed: bool,
    last_plan: Option<Timestamp>,
    pub(crate) dirty: bool,
    pub(crate) history: PlayHistory,
    pub(crate) slots: [Option<Slot>; 4],
    next_deck: usize,
    next_tag: u64,
    pub(crate) next_seek: u64,
    /// Items that failed to load, with the time of the failure.
    pub(crate) broken: BTreeMap<ItemId, Timestamp>,
    /// The station items at the last plan.
    pub(crate) items: BTreeMap<ItemId, Item>,
    pub(crate) alarms: Alarms,
    ticks: u64,
    view: Arc<Mutex<PlayoutView>>,
    /// Commands that wait for the mixer to confirm them.
    pub(crate) pending: Vec<Pending>,
    /// Stop after current: nothing new starts by itself, except hard events.
    pub(crate) hold: bool,
    pub(crate) carts: Carts,
    carts_dirty: bool,
    /// The engine state changed since the last publish.
    state_dirty: bool,
    /// The number of plans so far, for a check of the plan cost.
    plans: u64,
    pub(crate) dead_air: DeadAir,
    pub(crate) next_bed: usize,
    /// The Icecast source. It waits without settings while none are set up.
    pub(crate) icecast: Option<IcecastHandle>,
    /// Icecast has settings, so the view shows the status of the source.
    pub(crate) icecast_set_up: bool,
    /// The On air setting as Icecast and WebRTC last followed it.
    pub(crate) on_air: bool,
    pub(crate) webrtc: Option<WebRtcHandle>,
    /// Program for the WebRTC server, kept while the server is off.
    pub(crate) webrtc_tap: Option<TapSlot>,
    pub(crate) output_meters: crate::OutputMeters,
    /// The address of the running server.
    pub(crate) webrtc_bind: Option<std::net::SocketAddr>,
    /// The name and logo that the running server has, so a change is sent once.
    pub(crate) brand_sent: Option<(
        String,
        Option<std::path::PathBuf>,
        Option<std::time::SystemTime>,
    )>,
    /// Reads the modified time of the logo off this thread.
    pub(crate) logo_watch: Option<crate::streams::LogoWatch>,
    /// The logo the server had when it stopped.
    pub(crate) logo_before: Option<std::path::PathBuf>,
    /// The thread that stops the last server and frees its ports.
    pub(crate) webrtc_stopping: Option<std::thread::JoinHandle<()>>,
    /// What plays now, for a server that starts later.
    pub(crate) webrtc_now: Option<NowPlaying>,
    /// What listeners saw before the item on air, newest first, for the last hour.
    pub(crate) webrtc_played: std::collections::VecDeque<radio_stream::Played>,
    /// The last stream title sent.
    pub(crate) title: String,
    pub(crate) save_due: bool,
    last_prune: Option<Timestamp>,
    pub(crate) last_save: Option<Timestamp>,
    pub(crate) analysis: Analysis,
    pub(crate) log: LogHandle,
    pub(crate) work: Workers,
    /// Rows of items that ended before the writer returned their row id.
    ended: BTreeMap<u64, (Timestamp, LogStatus)>,
    /// Hard entries seen within the preload time. The planner drops a late one from the
    /// timeline, but playout still finds it here.
    hard_seen: Vec<Entry>,
    /// Items taken off a deck for a hard event. They wait until that event starts.
    blocked: Vec<Blocked>,
    /// The station of the last plan, to tell a real edit from other state changes.
    last_station: Option<radio_model::Station>,
    /// Hard events that an operator cancelled. Playout keeps them for a day.
    cancelled: Vec<Entry>,
    pub(crate) aired_fixed: Vec<Entry>,
    /// Occurrences that a late edit moved a locked hard event to. The locked event airs at
    /// its old time instead, so these do not air. Kept until their target passes.
    superseded: Vec<Superseded>,
    /// The rule list of the playlist of each hard event when the plan first had the event.
    /// A later edit compares with it, not with the station of the last plan.
    planned_rules: Vec<(Entry, Vec<radio_model::Rule>)>,
    out_clock: Option<SampleClock>,
    /// One-offs that ended on air. The next sweep removes them from the schedule.
    pub(crate) aired_one_offs: std::collections::BTreeSet<(ItemId, jiff::civil::DateTime)>,
}

fn empty_timeline() -> Timeline {
    Timeline {
        entries: Vec::new(),
        fits: Vec::new(),
        warnings: Vec::new(),
    }
}

impl Core {
    pub(crate) fn new(mut engine: Engine, cfg: PlayoutConfig) -> Result<Core, PlayoutError> {
        let engine_rx = engine.events();
        let log = engine.log_handle()?;
        let carts = Carts::new(cfg.mixer.cart_slots);
        let meters = crate::OutputMeters::default();
        // Icecast always has a tap, so settings that come while playout runs can use it.
        let tap = 1 + usize::from(cfg.webrtc.is_some());
        // The setting that a person stored wins over the default of the config. A stored
        // setting that the processors refuse falls back to that default.
        let stored = engine.state().dynamics.map(|d| ProgramDynamics::from(&d));
        let mut opened = match stored {
            Some(d) => {
                open_output(&cfg.output, cfg.mixer, Some(d), tap, &meters.device).or_else(|_| {
                    open_output(&cfg.output, cfg.mixer, cfg.dynamics, tap, &meters.device)
                })?
            }
            None => open_output(&cfg.output, cfg.mixer, cfg.dynamics, tap, &meters.device)?,
        };
        let mut taps = std::mem::take(&mut opened.taps).into_iter();
        // A station saved off air does not dial Icecast until it goes on air.
        let on_air = crate::streams::on_air(engine.state());
        let icecast_set_up = cfg.icecast.is_some();
        let icecast = taps.next().map(|tap| {
            IcecastClient::start_metered_with(
                cfg.icecast.clone(),
                on_air,
                tap,
                meters.icecast.clone(),
            )
        });
        let webrtc_tap = cfg
            .webrtc
            .as_ref()
            .and_then(|_| Some(Arc::new(Mutex::new(Some(taps.next()?)))));
        let now = cfg.clock.now();
        let mut alarms = Alarms::new(cfg.webhook.clone());
        if let Some(reason) = opened.fallback {
            alarms.raise(now, Alarm::OutputFallback { reason });
        }
        let view = PlayoutView {
            monitor_open: false,
            now,
            mode: engine.state().mode,
            stop_after_current: false,
            waiting_for_next: false,
            emergency: false,
            decks: Default::default(),
            timeline: empty_timeline(),
            meters: Default::default(),
            outputs: OutputStatus {
                local: opened.local.clone(),
                icecast: None,
                webrtc: None,
                webrtc_available: cfg.webrtc.is_some(),
                on_air,
            },
            stream_states: Vec::new(),
            alarms: Vec::new(),
            alarms_dropped: 0,
            state: Arc::new(engine.state().clone()),
            revision: 0,
            plans: 0,
            tick_overruns: 0,
        };
        let mut core = Core {
            engine,
            engine_rx,
            mixer: opened.handle,
            driver: opened.driver,
            _live: opened.live,
            cue: opened.cue,
            cue_name: String::new(),
            local: opened.local,
            clock: Arc::clone(&cfg.clock),
            map: ClockMap::default(),
            cfg,
            timeline: empty_timeline(),
            planned: false,
            timeline_changed: true,
            last_plan: None,
            dirty: true,
            history: PlayHistory::new(),
            slots: Default::default(),
            next_deck: 0,
            next_tag: 1,
            next_seek: 1,
            broken: BTreeMap::new(),
            items: BTreeMap::new(),
            alarms,
            ticks: 0,
            view: Arc::new(Mutex::new(view)),
            pending: Vec::new(),
            hold: false,
            carts,
            carts_dirty: true,
            state_dirty: false,
            plans: 0,
            tick_overruns: 0,
            dead_air: DeadAir::default(),
            next_bed: 0,
            icecast,
            icecast_set_up,
            on_air,
            webrtc: None,
            webrtc_tap,
            output_meters: meters,
            webrtc_bind: None,
            brand_sent: None,
            logo_watch: None,
            logo_before: None,
            webrtc_stopping: None,
            webrtc_now: None,
            webrtc_played: std::collections::VecDeque::new(),
            title: String::new(),
            save_due: false,
            last_prune: None,
            last_save: None,
            analysis: Analysis::default(),
            log,
            work: Workers::new(),
            ended: BTreeMap::new(),
            hard_seen: Vec::new(),
            blocked: Vec::new(),
            last_station: None,
            cancelled: Vec::new(),
            aired_fixed: Vec::new(),
            superseded: Vec::new(),
            planned_rules: Vec::new(),
            out_clock: opened.clock,
            aired_one_offs: std::collections::BTreeSet::new(),
        };
        if core.cfg.webrtc.is_some() {
            core.seed_played();
        }
        Ok(core)
    }

    pub(crate) fn view(&self) -> Arc<Mutex<PlayoutView>> {
        Arc::clone(&self.view)
    }

    pub(crate) fn meter_reader(&self) -> MeterReader {
        self.mixer.meter_reader()
    }

    pub(crate) fn is_offline(&self) -> bool {
        matches!(self.driver, Driver::Offline { .. })
    }

    pub(crate) fn subscribe(&mut self, tx: std::sync::mpsc::SyncSender<RaisedAlarm>) {
        self.alarms.subscribe(tx);
    }

    pub(crate) fn render(&mut self, frames: usize) -> Result<Vec<f32>, PlayoutError> {
        if !self.is_offline() {
            return Err(PlayoutError::NotOffline);
        }
        self.tick();
        self.settle_offline();
        let Driver::Offline { mixer, taps, cue } = &mut self.driver else {
            return Err(PlayoutError::NotOffline);
        };
        let (out, monitor) = radio_audio::render_offline_buses(mixer, frames);
        feed_tap(cue, &monitor);
        feed_taps(taps, &out);
        Ok(out)
    }

    /// Offline only: renders the tail that the lookahead holds, and feeds the taps.
    pub(crate) fn drain(&mut self) -> Result<Vec<f32>, PlayoutError> {
        let Driver::Offline { mixer, taps, .. } = &mut self.driver else {
            return Err(PlayoutError::NotOffline);
        };
        let out = radio_audio::drain_offline(mixer);
        feed_taps(taps, &out);
        Ok(out)
    }

    /// Answers a command with the CSV that `read` gives, off the playout thread.
    fn csv_query(
        &self,
        reply: Reply,
        read: impl FnOnce() -> Result<String, radio_engine::EngineError> + Send + 'static,
    ) {
        self.work.run_query(move || {
            let result = read().map(|csv| radio_engine::Outcome {
                data: Some(serde_json::Value::String(csv)),
                ..Default::default()
            });
            let _ = reply.send(result);
            Done::Query
        });
    }

    pub(crate) fn dispatch(&mut self, cmd: Command, reply: Reply) {
        fn has_script(c: &Command) -> bool {
            match c {
                Command::PlaylistSetRulesScript { .. } => true,
                Command::StationBatch { commands } | Command::StationSystem { commands } => {
                    commands.iter().any(has_script)
                }
                _ => false,
            }
        }
        if has_script(&cmd) {
            let names = self
                .engine
                .state()
                .station
                .playlists
                .iter()
                .map(|p| (p.name.clone(), p.id))
                .collect::<Vec<_>>();
            self.work.run_slow(move || {
                Done::Prepared(radio_engine::prepare_scripts(cmd, &names), reply)
            });
            return;
        }
        match cmd {
            Command::HistoryList { since, limit } => {
                let log = self.log.clone();
                self.work.run_query(move || {
                    let result = log
                        .history(since, limit)
                        .and_then(|rows| {
                            serde_json::to_value(rows)
                                .map_err(|e| radio_engine::EngineError::BadArgument(e.to_string()))
                        })
                        .map(|data| radio_engine::Outcome {
                            data: Some(data),
                            ..Default::default()
                        });
                    let _ = reply.send(result);
                    Done::Query
                });
                return;
            }
            Command::HistoryExportCsv { since, until } => {
                let log = self.log.clone();
                self.csv_query(reply, move || log.export_csv(since, until));
                return;
            }
            Command::HistoryPlaysCsv {
                since,
                until,
                playlist,
            } => {
                let log = self.log.clone();
                self.csv_query(reply, move || {
                    log.plays_csv(since, until, playlist.as_deref())
                });
                return;
            }
            Command::HistoryAiringsCsv {
                since,
                until,
                playlist,
            } => {
                let log = self.log.clone();
                let zone = self.engine.state().station.tz.clone();
                self.csv_query(reply, move || {
                    log.airings_csv(since, until, playlist.as_deref(), &zone)
                });
                return;
            }
            _ => {}
        }
        match self.engine.dispatch(cmd) {
            Ok(outcome) => {
                let checks = self.drain_engine();
                self.answer(outcome, checks, reply);
            }
            Err(e) => {
                let _ = reply.send(Err(e));
            }
        }
    }

    /// Takes the engine events. It turns intents into audio and returns their checks.
    pub(crate) fn drain_engine(&mut self) -> Vec<Check> {
        let mut checks = Vec::new();
        while let Ok(ev) = self.engine_rx.try_recv() {
            match ev {
                EngineEvent::StateChanged => {
                    self.dirty = true;
                    self.carts_dirty = true;
                    self.state_dirty = true;
                }
                // Faders and cart settings: the UI sees them, and no plan runs for them.
                EngineEvent::SettingsChanged => self.state_dirty = true,
                // The UI reads the save error from the log. It needs a new snapshot only.
                EngineEvent::PersistenceFailed { .. } => self.state_dirty = true,
                EngineEvent::AudioIntent(intent) => {
                    let now = self.clock.now();
                    checks.push(self.intent(intent, now));
                }
            }
        }
        checks
    }

    fn output_event(&mut self, event: crate::device::State, now: Timestamp) {
        match event {
            crate::device::State::Lost(name) => {
                self.local = LocalOutput::Lost { name: name.clone() };
                self.alarms.raise(now, Alarm::DeviceLost { name });
            }
            crate::device::State::Running(name, clock) => {
                self.local = LocalOutput::Device { name };
                self.out_clock = Some(clock);
                self.alarms
                    .clear(now, |a| matches!(a, Alarm::DeviceLost { .. }));
            }
            crate::device::State::Paced(clock) => self.out_clock = Some(clock),
        }
    }

    /// One pass of the control loop.
    pub(crate) fn tick(&mut self) {
        let tick_start = std::time::Instant::now();
        let now = self.clock.now();
        let device_events: Vec<_> = match &self.driver {
            Driver::Device { out } => out.events.try_iter().collect(),
            _ => Vec::new(),
        };
        for event in device_events {
            self.output_event(event, now);
        }

        if let Some(c) = &self.out_clock {
            self.map.latency = c.latency();
        }
        if let Some(step) = self.map.observe(now, self.mixer.now())
            && self.ticks > 0
            && !matches!(self.local, LocalOutput::Lost { .. })
        {
            let by_ms = i64::try_from(step.as_millis()).unwrap_or(i64::MAX);
            self.alarms.raise(now, Alarm::ClockStep { by_ms });
        }
        let cue_name = self
            .engine
            .state()
            .settings(radio_engine::AUDIO_PART)
            .and_then(|s| s.get(radio_engine::MONITOR_KEY))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        if self.ticks == 0 || cue_name != self.cue_name {
            let _ = self.cue.select.send(cue_name.clone());
            self.cue_name = cue_name;
            self.sync_strips();
        }
        for reason in self.cue.errors.try_iter() {
            self.alarms.raise(now, Alarm::DeviceLost { name: reason });
        }
        self.sync_air();
        self.sync_webrtc();
        self.sync_brand(now);
        self.ticks += 1;
        if self.ticks == 1 {
            self.seed_history(now);
            self.resume(now);
            self.sync_strips();
        }
        let _ = self.drain_engine();
        self.take_done(now);
        self.refresh_gains();
        if std::mem::take(&mut self.carts_dirty) || self.ticks == 1 {
            self.sync_carts();
        }
        let events: Vec<AudioEvent> = self.mixer.events().collect();
        for ev in events {
            self.on_audio(ev, now);
        }
        let due = self
            .last_plan
            .is_none_or(|t| now < t || since(t, now) >= PLAN_EVERY);
        if self.dirty || due {
            self.replan(now);
        }
        // Hard events claim a deck first, so a queued item cannot take it back.
        self.ensure_hard(now);
        self.make_room_for_soft();
        self.load_ahead(now);
        self.cue_starts(now);
        self.cue_ends(now);
        self.check_streams(now);
        self.check_dead_air(now);
        self.poll_states();
        self.stop_took_effect();
        self.sync_engine_decks();
        self.expire_ended(now);
        self.sweep_one_offs(now);
        if tick_start.elapsed() > self.cfg.command_lead / 2 {
            self.tick_overruns += 1;
            self.alarms.raise_once(now, Alarm::ControlOverrun);
        }
        self.publish(now);
        self.heartbeat(now);
        self.save_state(now);
        self.expire_replies();
    }

    fn on_audio(&mut self, ev: AudioEvent, now: Timestamp) {
        match ev {
            AudioEvent::Loaded { deck, item_tag } => {
                if let Some(s) = &mut self.slots[deck.index()]
                    && s.phase == Phase::Loading
                    && item_tag == Some(s.tag)
                {
                    s.phase = Phase::Ready;
                }
            }
            AudioEvent::LoadRejected { deck, item_tag } => {
                let ours = self.slots[deck.index()]
                    .as_ref()
                    .is_some_and(|s| item_tag == Some(s.tag));
                if ours {
                    self.slots[deck.index()] = None;
                    self.dirty = true;
                }
            }
            AudioEvent::Fading { deck, item_tag } => {
                if let Some(s) = &mut self.slots[deck.index()]
                    && item_tag == Some(s.tag)
                {
                    s.fading = true;
                }
            }
            AudioEvent::Started {
                deck,
                at_sample,
                item_tag,
            } => self.on_started(deck, at_sample, item_tag),
            AudioEvent::Seeked {
                deck,
                item_tag,
                id,
                at_sample,
            } => self.on_seeked(deck, item_tag, id, at_sample),
            AudioEvent::DecodeFailed { deck, item_tag } => {
                self.on_decode_failed(deck.index(), item_tag, now);
            }
            AudioEvent::DeckEnded(deck) => self.finish(deck.index(), now),
            AudioEvent::Underrun(deck) => {
                let file = self.slots[deck.index()]
                    .as_ref()
                    .is_some_and(|s| matches!(s.item.source, Source::File { .. }));
                if file {
                    let deck = deck.index() as u8;
                    self.alarms.raise(now, Alarm::DeckUnderrun { deck });
                }
            }
            AudioEvent::Silence { .. } => self.on_silence(now),
            AudioEvent::SilenceEnded { .. } => self.on_sound(now),
        }
    }

    /// The file on deck `i` stopped decoding before its end. The item did not air in
    /// full: its row ends as interrupted, the plan leaves it out for a while, and the
    /// next item does not wait for the rest of it.
    fn on_decode_failed(&mut self, i: usize, tag: Option<u64>, now: Timestamp) {
        let Some(s) = &mut self.slots[i] else {
            return;
        };
        if tag != Some(s.tag) {
            return;
        }
        s.cut = true;
        let (item, role) = (s.item.clone(), s.role);
        let reason = "the file stopped decoding before its end".to_string();
        if role == DeckRole::Plan {
            self.skip_broken(&item, reason, now);
            if self.hands_over_alone(i) {
                let _ = self.start_next(Some(i));
            }
        } else {
            let alarm = Alarm::ItemSkipped {
                item: item.id,
                title: item.title,
                reason,
            };
            self.alarms.raise(now, alarm);
        }
    }

    /// Tells if playout must start the next item itself after the item on deck `i`
    /// failed: the station plays by itself, and no other item is on air or cued to take
    /// over, as one is during a crossfade.
    fn hands_over_alone(&self, i: usize) -> bool {
        let taken_over = (0..4).filter(|&j| j != i).any(|j| {
            self.slots[j].as_ref().is_some_and(|s| {
                matches!(s.role, DeckRole::Plan | DeckRole::Operator) && !s.not_started()
            })
        });
        let auto = self.engine.state().mode == radio_engine::Mode::Auto;
        auto && !self.hold && !self.dead_air.emergency && !taken_over
    }

    /// The mixer moved the item on the deck to the position of seek `id`. The rest of the
    /// item plays from there, so its start, its planned end and the items after it move,
    /// as after a pause.
    fn on_seeked(&mut self, deck: DeckId, tag: Option<u64>, id: u64, at_sample: u64) {
        let wall = self.map.wall_at(at_sample);
        let Some(s) = &mut self.slots[deck.index()] else {
            return;
        };
        let Some((_, to)) = s.seek.filter(|(want, _)| tag == Some(s.tag) && *want == id) else {
            return;
        };
        s.seek = None;
        s.offset = to;
        if s.started.is_some() {
            s.started = Some(wall);
            if s.paused_at.is_some() {
                s.paused_at = Some(wall);
            }
            // A hard event keeps its target as its start.
            if !s.is_hard() {
                s.entry.start = sub(wall, to);
            }
        } else {
            s.seeked = true;
        }
        self.dirty = true;
        self.save_due = true;
    }

    fn on_started(&mut self, deck: DeckId, at_sample: u64, tag: Option<u64>) {
        let wall = self.map.wall_at(at_sample);
        let Some(s) = &mut self.slots[deck.index()] else {
            return;
        };
        if tag != Some(s.tag) {
            return;
        }
        s.phase = Phase::Playing;
        s.started = Some(wall);
        s.aired = Some(wall);
        let fallback = s.role == DeckRole::Emergency;
        if let Some(fixed) = &s.entry.fixed {
            let by = since(fixed.target, wall);
            if wall > fixed.target && by > fixed.tolerance {
                self.alarms.raise(
                    wall,
                    Alarm::FixedLate {
                        item: s.item.id,
                        target: fixed.target,
                        by,
                    },
                );
            }
        }
        if s.entry.fixed.is_some() {
            self.aired_fixed.push(s.entry.clone());
        }
        let id = s.item.id;
        // A hard event keeps its target as its start, so later plans still know it.
        if !s.is_hard() {
            s.entry.start = sub(wall, s.offset);
        }
        if fallback {
            self.dead_air_file_started();
        }
        self.dirty = true;
        self.save_due = true;
        // A play-next item leaves the queue when it starts.
        self.engine.dequeue(id);
        if self.slots[deck.index()]
            .as_ref()
            .is_some_and(|s| s.row.is_none())
        {
            self.log_start(deck.index(), wall);
        }
        let titled = self.slots[deck.index()]
            .as_ref()
            .filter(|s| matches!(s.role, DeckRole::Plan | DeckRole::Operator))
            .map(|s| s.item.clone());
        if let Some(item) = titled {
            self.send_title(&item);
        }
    }

    fn log_start(&mut self, i: usize, wall: Timestamp) {
        let Some(s) = &self.slots[i] else { return };
        let station = &self.engine.state().station;
        let playlist = station
            .playlists
            .iter()
            .find(|p| p.id == s.entry.playlist)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let origin = match (&s.item.source, s.role, &s.entry.origin) {
            (Source::LiveInput { .. }, _, _) => "live",
            (Source::Stream { .. }, _, _) => "stream",
            (_, DeckRole::Emergency, _) => "emergency",
            (_, DeckRole::Bed, _) => "bed",
            (_, _, Origin::Main) => "music",
            (_, _, Origin::Rule { .. }) => "rule",
            (_, _, Origin::Manual) => "manual",
            (_, _, Origin::Fill) => "fill",
        };
        let rule = match &s.entry.origin {
            Origin::Rule { playlist, rule } => Some(format!("{}#{rule}", playlist.0)),
            _ => None,
        };
        let entry = LogEntry {
            start: wall,
            playlist,
            title: s.item.title.clone(),
            artist: s.item.artist.clone(),
            origin: origin.to_string(),
            rule,
        };
        let played = Played {
            item: s.item.id,
            playlist: s.entry.playlist,
            at: wall,
            artist: s.item.artist.clone(),
            title: s.item.title.clone(),
        };
        if s.role != DeckRole::Bed {
            self.history.push(played);
        }
        let (tag, done) = (s.tag, self.work.done_tx.clone());
        self.log.log_start_later(&entry, move |result| {
            let result = result.map_err(|e| e.to_string());
            let _ = done.send(Done::Row { tag, result });
        });
    }

    /// The deck is empty: complete its log row and free it.
    pub(crate) fn finish(&mut self, i: usize, now: Timestamp) {
        let Some(s) = self.slots[i].take() else {
            return;
        };
        if let Some((deck, tag)) = s.stream.as_ref().and_then(|st| st.bed) {
            self.end_bed(deck, tag);
        }
        let status = if s.cut {
            LogStatus::Interrupted
        } else {
            LogStatus::Aired
        };
        if s.started.is_some() {
            self.note_aired_one_off(s.item.id);
        }
        match s.row {
            Some(row) => self.log.log_end_later(row, now, status),
            None if s.started.is_some() => {
                self.ended.insert(s.tag, (now, status));
            }
            None => {}
        }
        self.dirty = true;
        self.save_due = true;
    }

    /// Moves the starts of the queued entries behind the item before them.
    fn chain(&mut self, now: Timestamp) {
        let mut order: Vec<usize> = (0..4)
            .filter(|&i| self.slots[i].as_ref().is_some_and(Slot::pinned))
            .collect();
        // A seek forward puts the start of an entry before the start of the item that
        // still ends under it, so the items on air go by when they went on air.
        order.sort_by_key(|&i| {
            self.slots[i]
                .as_ref()
                .map(|s| s.aired.unwrap_or(s.entry.start))
        });
        let mut prev: Option<(Timestamp, Item, Duration)> = None;
        for i in order {
            let Some(s) = &mut self.slots[i] else {
                continue;
            };
            if s.not_started() {
                if let Some((start, item, trim)) = &prev
                    && !s.is_hard()
                {
                    let gap = radio_timing::advance(item, Some(&s.item)).saturating_sub(*trim);
                    let handover = add(*start, gap);
                    // A soft event never starts early. It waits for the item before it.
                    s.entry.start = match s.entry.fixed {
                        Some(_) => s.entry.start.max(handover),
                        None => handover,
                    };
                }
                // A hard event keeps its target, even when it starts late.
                if (s.offset.is_zero() || s.seeked) && !s.is_hard() {
                    s.entry.start = s.entry.start.max(now);
                }
            }
            // The next item follows the real start: a late hard event plays in full.
            let start = match s.started {
                Some(_) => s.real_start(),
                None if s.is_hard() => s.entry.start.max(now),
                None => s.real_start(),
            };
            prev = Some((start, s.item.clone(), s.entry.trim));
        }
    }

    fn replan(&mut self, now: Timestamp) {
        self.plans += 1;
        self.chain(now);
        self.prune_history(now);
        let mut station = self.engine.state().station.clone();
        self.apply_analysis(&mut station);
        self.broken.retain(|_, at| since(*at, now) < RETRY_BROKEN);
        let missing = self.engine.state().missing.clone();
        for p in &mut station.playlists {
            p.items
                .retain(|i| !self.broken.contains_key(&i.id) && !missing.contains(&i.id));
        }
        self.items = station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| (i.id, i.clone())))
            .collect();
        // Playout does not pin a queued hard event far from its target. The plan must
        // still produce it, or it goes. Near its target, playout pins it so it stays.
        let lock = add(now, HARD_LOCK);
        let free_hard = |s: &Slot| s.is_hard() && s.not_started() && s.entry.start > lock;
        // Items to play next go right after the item on air. The queued plan items that
        // wait on decks give way to them, unless they are hard events.
        let queued = self.play_next_entries(now, &station);
        // Until the play-next items are on decks, nothing of the plan waits ahead of them.
        if !queued.is_empty() {
            self.unload_waiting_plan_items();
        }
        let mut pinned: Vec<Entry> = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.pinned() && !free_hard(s))
            .map(|s| s.entry.clone())
            .collect();
        pinned.extend(queued);
        let input = PlanInput {
            station: &station,
            history: &self.history,
            now,
            horizon: self.cfg.horizon,
            pinned: &pinned,
        };
        let mut opts = self.cfg.plan;
        opts.resume_rotation = true;
        let edited = self.last_station.as_ref() != Some(&self.engine.state().station);
        // After a station edit, the previous plan keeps only the events and rules that the
        // station still has, so no stale fixed event survives. Its frozen windows stay.
        let filtered = (self.planned && edited)
            .then(|| still_planned(&self.timeline, self.last_station.as_ref(), &station));
        let prev = match &filtered {
            Some(f) => Some(f),
            None => self.planned.then_some(&self.timeline),
        };
        let mut tl = plan(&input, &opts, prev);
        self.aired_fixed.retain(|e| {
            e.fixed.as_ref().is_some_and(|f| {
                now < f.target || since(f.target, now) < Duration::from_secs(86400)
            })
        });
        let aired: Vec<_> = self
            .aired_fixed
            .iter()
            .filter(|e| {
                !self
                    .slots
                    .iter()
                    .flatten()
                    .any(|s| same_occurrence(&s.entry, e))
            })
            .map(|e| Superseded {
                entry: e.clone(),
                stand_in: e.clone(),
            })
            .collect();
        drop_superseded(&mut tl, &aired);
        self.expire_superseded(now);
        drop_superseded(&mut tl, &self.superseded);
        // A late fixed event keeps its target for alarms, but its row starts where it
        // really went on air. The following flow must follow that audible interval.
        let mut previous: Option<Entry> = None;
        for entry in &mut tl.entries {
            if let Some(slot) = self.slots.iter().flatten().find(|s| {
                s.started.is_some() && s.entry.fixed.is_some() && same_event(&s.entry, entry)
            }) {
                entry.start = slot.real_start();
            } else if entry.fixed.is_none()
                && let Some(prev) = &previous
            {
                let step = self
                    .items
                    .get(&prev.item)
                    .map_or(prev.len, |item| {
                        radio_timing::advance(item, self.items.get(&entry.item))
                    })
                    .saturating_sub(prev.trim);
                entry.start = entry.start.max(add(prev.start, step));
            }
            previous = Some(entry.clone());
        }
        // A waiting soft event never starts before the plan says. The plan puts it after the
        // locked entries in its way, and those keep their times.
        for s in self.slots.iter_mut().flatten() {
            if s.not_started() && !s.is_hard() && s.entry.fixed.is_some() {
                let planned = tl.entries.iter().find(|e| same_event(e, &s.entry));
                if let Some(e) = planned {
                    s.entry.start = s.entry.start.max(e.start);
                }
            }
        }
        let old = std::mem::replace(&mut self.timeline, tl);
        if edited {
            if self.planned {
                self.warn_locked_hard(&station, &old, now);
                let superseded = std::mem::take(&mut self.superseded);
                drop_superseded(&mut self.timeline, &superseded);
                self.superseded = superseded;
            }
            self.last_station = Some(self.engine.state().station.clone());
        }
        if old != self.timeline {
            self.timeline_changed = true;
            self.save_due = true;
        }
        self.measure_ahead(now);
        self.note_planned_rules(&station, now);
        self.prune_hard(now);
        self.planned = true;
        self.dirty = false;
        self.last_plan = Some(now);
    }

    /// Pinned entries for the play-next queue of the engine, one after the other from the end
    /// of the item on air. Items on a deck already are left out.
    fn play_next_entries(&self, now: Timestamp, station: &radio_model::Station) -> Vec<Entry> {
        let queue = &self.engine.state().queue;
        if queue.is_empty() {
            return Vec::new();
        }
        // The item on air ends at its deck's planned end, or else at its end in the plan.
        let on_deck = self
            .slots
            .iter()
            .flatten()
            .filter(|s| !s.not_started())
            .map(Slot::planned_end)
            .max();
        let in_plan = self
            .timeline
            .entries
            .iter()
            .filter(|e| e.start <= now && now < add(e.start, e.len))
            .map(|e| add(e.start, e.len.saturating_sub(e.trim)))
            .max();
        let mut at = on_deck.max(in_plan).unwrap_or(now).max(now);
        let mut out = Vec::new();
        for id in queue {
            if self.slots.iter().flatten().any(|s| s.item.id == *id) {
                continue;
            }
            let found = station
                .playlists
                .iter()
                .find_map(|p| p.items.iter().find(|i| i.id == *id).map(|i| (p.id, i)));
            let Some((playlist, item)) = found else {
                continue;
            };
            let len = radio_timing::item_len(item);
            out.push(Entry {
                playlist,
                item: *id,
                start: at,
                len,
                origin: Origin::Manual,
                locked: true,
                fixed: None,
                trim: Duration::ZERO,
            });
            at = add(at, len);
        }
        out
    }

    /// Takes the plan items that wait on decks off them, so a new plan can move them after
    /// the play-next items. Hard events and operator items stay.
    fn unload_waiting_plan_items(&mut self) {
        self.unload_where(|s| !s.is_hard());
    }

    /// Makes room for a soft event that the plan puts before items on decks. It takes
    /// those items off their decks. The event then loads first, and they load after it.
    /// Else the event never loads, and its time passes.
    fn make_room_for_soft(&mut self) {
        let on_deck: Vec<Entry> = self
            .slots
            .iter()
            .flatten()
            .map(|s| s.entry.clone())
            .collect();
        let Some(event) = self.timeline.entries.iter().position(|e| {
            e.fixed.as_ref().is_some_and(|f| f.mode == FixMode::Soft)
                && !on_deck.iter().any(|d| same_event(d, e))
                && !self.aired_fixed.iter().any(|a| same_occurrence(a, e))
        }) else {
            return;
        };
        let after: Vec<ItemId> = self
            .slots
            .iter()
            .flatten()
            .map(|s| s.item.id)
            .filter(|id| {
                self.timeline
                    .entries
                    .iter()
                    .position(|e| e.item == *id && e.fixed.is_none())
                    .is_some_and(|k| k > event)
            })
            .collect();
        self.unload_where(|s| s.entry.fixed.is_none() && after.contains(&s.item.id));
    }

    /// Takes off their decks the plan items that wait there and match `which`. Items on
    /// the play-next queue stay.
    fn unload_where(&mut self, which: impl Fn(&Slot) -> bool) {
        let queue = self.engine.state().queue.clone();
        for i in 0..4 {
            let waiting = self.slots[i].as_ref().is_some_and(|s| {
                s.role == DeckRole::Plan
                    && s.not_started()
                    && which(s)
                    && !queue.contains(&s.item.id)
            });
            if waiting && let Some(s) = self.slots[i].take() {
                let cmd = Cmd::new(self.mixer.now(), CommandKind::Stop(DeckId::ALL[i]));
                let _ = self.mixer.send(cmd.for_item(s.tag));
            }
        }
    }

    /// Keeps the play history to the last 48 h, checked once an hour, so plans stay cheap.
    fn prune_history(&mut self, now: Timestamp) {
        const KEEP: Duration = Duration::from_secs(48 * 3600);
        const EVERY: Duration = Duration::from_secs(3600);
        if self.last_prune.is_some_and(|t| since(t, now) < EVERY) {
            return;
        }
        self.last_prune = Some(now);
        let mut kept = PlayHistory::new();
        for p in self.history.iter_since(sub(now, KEEP)) {
            kept.push(p.clone());
        }
        self.history = kept;
    }

    /// The next empty deck in the rotation, among the decks the station uses. A deck
    /// past that number takes no new item; what it holds plays out.
    pub(crate) fn free_deck(&self) -> Option<usize> {
        let n = self.engine.state().deck_count();
        (0..n).map(|k| (self.next_deck + k) % n).find(|&i| {
            self.slots[i].is_none() && self.mixer.deck_state(DeckId::ALL[i]) == DeckState::Empty
        })
    }

    /// A deck for a bed or a fallback file. These play under or in place of the plan, so
    /// they may take a deck past the number in use when the others are full.
    pub(crate) fn spare_deck(&self) -> Option<usize> {
        self.free_deck().or_else(|| self.empty_deck_from(0))
    }

    /// A deck for a hard event. It waits for its time, so it takes a deck past the number
    /// in use first, and the decks in use stay with the rotation.
    fn timed_deck(&self) -> Option<usize> {
        self.empty_deck_from(self.engine.state().deck_count())
            .or_else(|| self.free_deck())
    }

    fn empty_deck_from(&self, first: usize) -> Option<usize> {
        (first..4).find(|&i| {
            self.slots[i].is_none() && self.mixer.deck_state(DeckId::ALL[i]) == DeckState::Empty
        })
    }

    /// Loads the next planned entries that start within the preload time.
    fn load_ahead(&mut self, now: Timestamp) {
        // Hard events load on their own (see `ensure_hard`) and do not count here. A soft
        // event keeps its own time, so the items before it still load.
        let plan_slots = || self.slots.iter().flatten().filter(|s| s.pinned());
        let last = plan_slots()
            .filter(|s| s.entry.fixed.is_none())
            .map(|s| s.entry.start)
            .max();
        let queued = plan_slots()
            .filter(|s| s.not_started() && !s.is_hard())
            .count();
        // One deck in use stays for the item on air, so a start never waits for a free deck.
        let in_use = MAX_QUEUED.min(self.engine.state().deck_count() - 1);
        let soft_waits =
            plan_slots().any(|s| s.not_started() && !s.is_hard() && s.entry.fixed.is_some());
        let assigned = self.assigned();
        // An item that waits on a deck already never loads on a second deck. Its next
        // airing waits for that deck, and so does everything after it: loading past it
        // would drop that airing from the order.
        let waits = |plan: bool| -> Vec<ItemId> {
            self.slots
                .iter()
                .flatten()
                .filter(|s| s.not_started() && (s.role == DeckRole::Plan) == plan)
                .map(|s| s.item.id)
                .collect()
        };
        // An item that the operator holds on a deck is theirs: the plan loads around it.
        let (waiting, held) = (waits(true), waits(false));
        let next: Vec<Entry> = self
            .timeline
            .entries
            .iter()
            .filter(|e| !assigned.contains(&(e.item, e.start)))
            .filter(|e| !held.contains(&e.item))
            .filter(|e| !e.fixed.as_ref().is_some_and(|f| f.mode == FixMode::Hard))
            .filter(|e| {
                !self
                    .blocked
                    .iter()
                    .any(|b| b.item == e.item && b.start == e.start && now < b.until)
            })
            .filter(|e| last.is_none_or(|l| e.start > l))
            .take_while(|e| !waiting.contains(&e.item))
            .take(MAX_QUEUED.saturating_sub(queued))
            .cloned()
            .collect();
        // A soft event leaves the plan when its time passes and it is on no deck, and the
        // plan needs the loaded items to be the next ones, in order. So while a soft event
        // is near or waits on a deck, the station loads ahead as it does with four decks,
        // on the decks past the number in use too.
        let soon = add(now, self.cfg.preload);
        let soft_due = next
            .iter()
            .any(|e| e.fixed.as_ref().is_some_and(|f| f.target <= soon));
        let spare = soft_waits || soft_due;
        let mut queued = queued;
        for entry in next {
            if queued >= in_use && !spare {
                break;
            }
            let Some(item) = self.items.get(&entry.item).cloned() else {
                continue;
            };
            if matches!(item.source, Source::Stream { .. })
                && entry.start > add(now, self.cfg.stream_preroll)
            {
                break;
            }
            let deck = if spare {
                self.spare_deck()
            } else {
                self.free_deck()
            };
            let Some(i) = deck else {
                break;
            };
            if let Err(reason) = self.load(i, entry, item.clone(), DeckRole::Plan, Duration::ZERO) {
                self.skip_broken(&item, reason, now);
                break;
            }
            queued += 1;
        }
    }

    /// The (item, start) of each pinned entry on a deck.
    pub(crate) fn assigned(&self) -> Vec<(ItemId, Timestamp)> {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.pinned())
            .map(|s| (s.entry.item, s.entry.start))
            .collect()
    }

    /// Raises the alarm for an item that failed to load, and plans without it.
    pub(crate) fn skip_broken(&mut self, item: &Item, reason: String, now: Timestamp) {
        self.alarms.raise(
            now,
            Alarm::ItemSkipped {
                item: item.id,
                title: item.title.clone(),
                reason,
            },
        );
        self.broken.insert(item.id, now);
        self.dirty = true;
    }

    /// Loads `item` on deck `i`, `offset` after its cue-in.
    pub(crate) fn load(
        &mut self,
        i: usize,
        entry: Entry,
        item: Item,
        role: DeckRole,
        offset: Duration,
    ) -> Result<u64, String> {
        let tag = self.next_tag;
        self.next_tag += 1;
        let deck = DeckId::ALL[i];
        let (cue_in, fade_in) = item
            .timing
            .as_ref()
            .map_or((Duration::ZERO, Duration::ZERO), |t| (t.cue_in, t.fade_in));
        let options = LoadOptions {
            live_bus: matches!(item.source, Source::LiveInput { .. }),
            cue_in: cue_in.saturating_add(offset),
            fade_in: if offset.is_zero() {
                fade_in
            } else {
                Duration::ZERO
            },
            item_tag: Some(tag),
            gain_db: 0.0,
        };
        let mut stream = None;
        let mut phase = Phase::Loading;
        let mut gain = None;
        match &item.source {
            Source::File { path } => {
                let (path, cue_in) = (path.clone(), options.cue_in);
                let open = self.cfg.open_file;
                self.work.run(move || Done::Opened {
                    tag,
                    result: open(std::path::Path::new(&path), cue_in).map_err(|e| e.to_string()),
                    tagged: radio_library::read_loudness_tags(&path),
                });
                phase = Phase::Opening;
                gain = Some(self.item_gain(item.id, None));
                self.ensure_measured(&item);
            }
            Source::Stream { url, keep_alive } => {
                stream = Some(self.load_stream(deck, url, *keep_alive, options)?);
            }
            Source::LiveInput { device, channels } => {
                let (device, channels, open) = (device.clone(), *channels, self.cfg.open_input);
                self.work.run(move || {
                    let (producer, consumer) = rtrb::RingBuffer::new(48_000 * 2);
                    Done::InputOpened {
                        tag,
                        result: open(&device, channels, producer)
                            .map(|capture| (capture, consumer)),
                    }
                });
                phase = Phase::Opening;
            }
        }
        let live_end = matches!(item.source, Source::LiveInput { .. })
            .then(|| add(entry.start, entry.len.saturating_sub(entry.trim)));
        self.slots[i] = Some(Slot {
            live_end,
            tag,
            entry,
            item,
            role,
            phase,
            started: None,
            aired: None,
            paused_at: None,
            end_sent: false,
            cut: false,
            row: None,
            stream,
            capture: None,
            offset,
            seek: None,
            seek_failed: None,
            reopening: false,
            seeked: false,
            fading: false,
            asap: false,
            options,
            gain,
            tagged: None,
        });
        self.next_deck = (i + 1) % self.engine.state().deck_count();
        Ok(tag)
    }

    /// Sends `Play` for each queued entry that starts within the command lead.
    fn cue_starts(&mut self, now: Timestamp) {
        let soon = add(now, self.cfg.command_lead);
        for i in 0..4 {
            let Some(s) = &self.slots[i] else { continue };
            // A deck whose ring is not full yet would start on silence: it starts late.
            let ready = self.mixer.deck_state(DeckId::ALL[i]) == DeckState::Ready;
            if s.asap && s.not_started() && ready {
                let at = self.mixer.now();
                self.play_at(i, at);
                continue;
            }
            if s.role != DeckRole::Plan || !s.not_started() || s.entry.start > soon {
                continue;
            }
            if !ready || !self.may_start(s) {
                continue;
            }
            let at = self.map.sample_at(s.entry.start).max(self.mixer.now());
            self.play_at(i, at);
        }
    }

    /// Sends `Play` for deck `i` at frame `at`.
    pub(crate) fn play_at(&mut self, i: usize, at: u64) {
        self.play_fading_at(i, at, Duration::ZERO);
    }

    /// Sends `Play` for deck `i` at frame `at`, with a fade-in over `fade_in`.
    pub(crate) fn play_fading_at(&mut self, i: usize, at: u64, fade_in: Duration) {
        let Some(s) = &mut self.slots[i] else { return };
        if s.phase == Phase::Opening {
            s.asap = true;
            return;
        }
        let deck = DeckId::ALL[i];
        let kind = if fade_in.is_zero() {
            CommandKind::Play(deck)
        } else {
            CommandKind::PlayFadeIn {
                deck,
                over: fade_in,
            }
        };
        if self.mixer.send(Cmd::new(at, kind).for_item(s.tag)).is_ok() {
            s.phase = Phase::Cued(at);
        }
    }

    /// The first hard event in the timeline after `start` and before `end`, other than
    /// the event `own` itself.
    fn hard_between(&self, own: &Entry, start: Timestamp, end: Timestamp) -> Option<Timestamp> {
        self.timeline
            .entries
            .iter()
            .filter(|e| !same_event(e, own))
            .filter_map(|e| e.fixed.as_ref())
            .filter(|f| f.mode == FixMode::Hard)
            .map(|f| f.target)
            .find(|t| *t > start && *t < end)
    }

    /// Sends the fade at the planned end of each started entry when that end is near.
    fn cue_ends(&mut self, now: Timestamp) {
        let soon = add(now, self.cfg.command_lead);
        for i in 0..4 {
            let Some(s) = &self.slots[i] else { continue };
            let live = matches!(s.item.source, Source::LiveInput { .. });
            if s.not_started() || s.end_sent || (s.phase == Phase::Paused && !live) {
                continue;
            }
            // Until the mixer reports the start, a cued item starts at its cue frame. A
            // hard event that is late or resumed starts after its target.
            let start = match (s.started, s.phase) {
                (None, Phase::Cued(at)) => sub(self.map.wall_at(at), s.offset),
                _ => s.real_start(),
            };
            let planned = matches!(s.role, DeckRole::Plan | DeckRole::Operator);
            // Beds and fallback files have no planned end; only a hard event ends them.
            let mut end = if planned {
                add(start, s.entry.len.saturating_sub(s.entry.trim))
            } else {
                Timestamp::MAX
            };
            if matches!(s.item.source, Source::LiveInput { .. }) {
                end = s.planned_end();
            }
            let hard = if live {
                self.timeline
                    .entries
                    .iter()
                    .filter(|e| e.fixed.is_some() && !same_event(&s.entry, e))
                    .map(|e| e.start)
                    .filter(|t| *t > start && *t < end)
                    .min()
            } else {
                // After a seek forward the item starts, by its position, before it went on
                // air. A hard event from before that is over, and does not cut it.
                let from = s.aired.map_or(start, |aired| aired.max(start));
                self.hard_between(&s.entry, from, end)
            };
            if !planned && hard.is_none() {
                continue;
            }
            if let Some(t) = hard {
                end = t;
            }
            let own = s
                .item
                .timing
                .as_ref()
                .map_or(Duration::ZERO, |t| t.fade_out);
            let early = hard.is_some() || !s.entry.trim.is_zero();
            let fade = if early && own.is_zero() {
                CUT_FADE
            } else {
                own
            };
            let fade = fade.min(since(start, end));
            let fade_at = sub(end, fade);
            if fade_at > soon {
                continue;
            }
            let floor = match s.phase {
                Phase::Cued(at) => at,
                _ => 0,
            };
            let at = self.map.sample_at(fade_at).max(floor);
            self.fade_at(i, at, fade, hard.is_some());
        }
    }

    /// Sends a `FadeOut` of length `over` for deck `i` at frame `at`.
    pub(crate) fn fade_at(&mut self, i: usize, at: u64, over: Duration, cut: bool) {
        let Some(s) = &mut self.slots[i] else { return };
        let deck = DeckId::ALL[i];
        let cmd = Cmd::new(at, CommandKind::FadeOut { deck, over }).for_item(s.tag);
        if self.mixer.send(cmd).is_ok() {
            s.end_sent = true;
            s.cut |= cut;
        }
    }

    fn publish(&mut self, now: Timestamp) {
        let waiting = self.slots.iter().flatten().any(|s| {
            s.role == DeckRole::Plan
                && s.not_started()
                && s.entry.start <= now
                && !self.may_start(s)
        });
        let decks: [DeckView; 4] = std::array::from_fn(|i| match &self.slots[i] {
            None => DeckView::default(),
            Some(s) => DeckView {
                item: Some(s.item.id),
                title: s.item.title.clone(),
                phase: match s.phase {
                    Phase::Opening | Phase::Loading => DeckPhase::Loading,
                    Phase::Ready => DeckPhase::Ready,
                    Phase::Cued(_) => DeckPhase::Cued,
                    Phase::Playing => DeckPhase::Playing,
                    Phase::Paused => DeckPhase::Paused,
                },
                role: Some(s.role),
                start: Some(s.started.unwrap_or(s.entry.start)),
                end: Some(s.planned_end()),
                position: s.position(now).0,
                running: s.position(now).1,
                len: s.planned_len(),
                seekable: s.seekable(),
                gain: s.gain,
            },
        });
        let Ok(mut view) = self.view.lock() else {
            return;
        };
        view.monitor_open = self.cue.open.load(std::sync::atomic::Ordering::Acquire);
        view.now = now;
        view.mode = self.engine.state().mode;
        view.stop_after_current = self.hold;
        view.emergency = self.dead_air.emergency;
        view.waiting_for_next = waiting;
        view.decks = decks;
        view.outputs.local = self.local.clone();
        view.alarms = self.alarms.recent.iter().cloned().collect();
        view.alarms_dropped = self.alarms.dropped;
        view.stream_states = self.stream_states();
        // Without settings the strip says Icecast is not set up, not what the waiting
        // source reports.
        view.outputs.icecast = self
            .icecast
            .as_ref()
            .filter(|_| self.icecast_set_up)
            .map(IcecastHandle::status);
        view.outputs.on_air = self.on_air;
        view.outputs.webrtc = self.webrtc.as_ref().map(WebRtcHandle::status);
        if self.timeline_changed {
            view.timeline = self.timeline.clone();
            self.timeline_changed = false;
        }
        view.plans = self.plans;
        view.tick_overruns = self.tick_overruns;
        if std::mem::take(&mut self.state_dirty) {
            view.state = Arc::new(self.engine.state().clone());
            view.revision += 1;
        }
    }

    /// Ends the open log rows as interrupted, and writes the state to disk.
    pub(crate) fn shutdown(&mut self) {
        let now = self.clock.now();
        for i in 0..4 {
            if let Some(s) = &mut self.slots[i] {
                s.cut = true;
            }
            self.finish(i, now);
        }
        // Rows whose id arrives now still get their end.
        let _ = self.engine.flush();
        self.take_done(now);
        let _ = self.engine.flush();
    }

    /// Offline only: waits until no deck is still opening or filling, for 2 s at most. Then
    /// a run does not depend on the speed of the disk. A real output never waits.
    fn settle_offline(&mut self) {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            let busy = (0..4).any(|i| {
                self.slots[i].as_ref().is_some_and(|s| {
                    s.phase == Phase::Opening
                        || s.reopening
                        || (s.not_started()
                            && self.mixer.deck_state(DeckId::ALL[i]) == DeckState::Loading)
                })
            });
            if !busy {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
            let now = self.clock.now();
            self.take_done(now);
        }
    }

    /// Takes the results of the workers and of the log writer.
    pub(crate) fn take_done(&mut self, now: Timestamp) {
        for i in 0..4 {
            let error = self.slots[i]
                .as_mut()
                .and_then(|s| s.capture.as_mut())
                .and_then(|c| c.error());
            if let Some(reason) = error {
                let mut on_air = false;
                if let Some(s) = &self.slots[i] {
                    on_air = s.role == DeckRole::Plan && !s.not_started();
                    let item = s.item.clone();
                    self.skip_broken(&item, reason, now);
                }
                if let Some(s) = &mut self.slots[i] {
                    s.cut = true;
                    let _ = self.mixer.send(
                        Cmd::new(self.mixer.now(), CommandKind::Stop(DeckId::ALL[i]))
                            .for_item(s.tag),
                    );
                }
                // `Stop` sends no `DeckEnded`, so the deck is freed here.
                self.finish(i, now);
                // Nothing hands over to the next item now, so it starts here.
                let idle = self.slots.iter().flatten().all(Slot::not_started);
                if (on_air || idle) && self.hands_over_alone(i) {
                    let _ = self.start_next(None);
                }
            }
        }
        while let Ok(done) = self.work.done_rx.try_recv() {
            match done {
                Done::InputOpened { tag, result } => {
                    if let Some(i) = self
                        .slots
                        .iter()
                        .position(|s| s.as_ref().is_some_and(|s| s.tag == tag))
                    {
                        let result = result.and_then(|(capture, ring)| {
                            let Some(s) = self.slots[i].as_mut() else {
                                return Ok(());
                            };
                            self.mixer
                                .load_ring(DeckId::ALL[i], ring, s.options)
                                .map_err(|e| e.to_string())?;
                            s.capture = Some(capture);
                            s.phase = Phase::Loading;
                            Ok(())
                        });
                        if let Err(reason) = result
                            && let Some(s) = self.slots[i].take()
                        {
                            self.skip_broken(&s.item, reason, now);
                        }
                    }
                }
                Done::Query => {}
                Done::Prepared(result, reply) => match result {
                    Ok(cmd) => self.dispatch(cmd, reply),
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                },
                Done::Opened {
                    tag,
                    result,
                    tagged,
                } => self.on_opened(tag, result, tagged, now),
                Done::Reopened { tag, id, result } => self.on_reopened(tag, id, result, now),
                Done::Row { tag, result } => match result {
                    Ok(row) => {
                        let slot = self.slots.iter_mut().flatten().find(|s| s.tag == tag);
                        match slot {
                            Some(s) => {
                                s.row = Some(row);
                                // The saved state must know the row, for a resume.
                                self.save_due = true;
                            }
                            None => {
                                if let Some((end, status)) = self.ended.remove(&tag) {
                                    self.log.log_end_later(row, end, status);
                                }
                            }
                        }
                    }
                    Err(reason) => {
                        self.ended.remove(&tag);
                        self.alarms.raise(now, Alarm::LogFailed { reason });
                    }
                },
                Done::Folder(files) => self.on_folder(files, now),
                Done::Analysed(id, timing) => self.on_analysed(id, timing),
                Done::Measured(id, measured) => self.on_measured(id, measured),
                Done::Cart(i, item, result) => self.on_cart(i, item, result, now),
            }
        }
    }

    /// A worker opened the file of the load with `tag` at the position of seek `id`. The
    /// decoder goes to the mixer, unless a newer seek or another item took its place.
    fn on_reopened(&mut self, tag: u64, id: u64, result: Result<Decoder, String>, now: Timestamp) {
        let Some(i) = (0..4).find(|&i| {
            self.slots[i]
                .as_ref()
                .is_some_and(|s| s.tag == tag && s.seek.is_some_and(|(want, _)| want == id))
        }) else {
            return;
        };
        let deck = DeckId::ALL[i];
        if let Some(s) = &mut self.slots[i] {
            s.reopening = false;
        }
        let sent = result.and_then(|dec| {
            self.mixer
                .seek_decoder(deck, dec, Some(tag), id)
                .map_err(|e| e.to_string())
        });
        if let Err(reason) = sent {
            if let Some(s) = &mut self.slots[i] {
                s.seek = None;
                s.seek_failed = Some(id);
            }
            let name = ['A', 'B', 'C', 'D'][i];
            let what = format!("Deck {name} keeps its position, the file did not open: {reason}");
            self.alarms.raise(now, Alarm::Unsupported { what });
        }
    }

    /// A worker opened (or failed to open) the file of the load with `tag`.
    fn on_opened(
        &mut self,
        tag: u64,
        result: Result<Decoder, String>,
        tagged: Option<radio_model::Loudness>,
        now: Timestamp,
    ) {
        let Some(i) = (0..4).find(|&i| {
            self.slots[i]
                .as_ref()
                .is_some_and(|s| s.tag == tag && s.phase == Phase::Opening)
        }) else {
            return;
        };
        let deck = DeckId::ALL[i];
        // The gain goes with the load, so the item plays at it from its first sample.
        let id = self.slots[i].as_ref().map(|s| s.item.id);
        if let Some(id) = id {
            let gain = self.item_gain(id, tagged);
            if let Some(s) = &mut self.slots[i] {
                s.tagged = tagged;
                s.options.gain_db = gain.db;
                s.gain = Some(gain);
            }
        }
        let result = result.and_then(|dec| {
            let options = self.slots[i]
                .as_ref()
                .map(|s| s.options)
                .unwrap_or_default();
            self.mixer
                .load_decoder(deck, dec, options)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => {
                if let Some(s) = &mut self.slots[i] {
                    s.phase = Phase::Loading;
                }
            }
            Err(reason) => {
                let Some(s) = self.slots[i].take() else {
                    return;
                };
                match s.role {
                    DeckRole::Plan => self.skip_broken(&s.item, reason, now),
                    DeckRole::Emergency => self.dead_air_file_failed(now),
                    DeckRole::Operator | DeckRole::Bed => {
                        let alarm = Alarm::ItemSkipped {
                            item: s.item.id,
                            title: s.item.title,
                            reason,
                        };
                        self.alarms.raise(now, alarm);
                    }
                }
                self.dirty = true;
            }
        }
    }

    /// Clears the engine flag once stop-after-current has stopped the station.
    fn stop_took_effect(&mut self) {
        if !self.hold || !self.engine.state().stop_after_current {
            return;
        }
        let on_air = self
            .slots
            .iter()
            .flatten()
            .any(|s| matches!(s.role, DeckRole::Plan | DeckRole::Operator) && !s.not_started());
        if !on_air {
            self.engine.clear_stop_after_current();
        }
    }

    /// Tells the engine which item each deck holds, so `deck.*` commands reach the
    /// decks that playout loads.
    fn sync_engine_decks(&mut self) {
        for i in 0..4 {
            // Only station items: the fallback files have made-up ids.
            let item = self.slots[i]
                .as_ref()
                .map(|s| s.item.id)
                .filter(|id| self.items.contains_key(id));
            if self.engine.state().decks[i] != item {
                self.engine.set_deck(i as u8, item);
            }
            let live = self.slots[i].as_ref().is_some_and(|s| !s.not_started());
            self.engine.set_live(i as u8, live);
        }
    }

    /// Forgets hard events that the plan no longer has: a moved or deleted event. A queued
    /// one leaves its deck. A past event stays while its item is still in the station,
    /// because the planner never shows past targets.
    fn prune_hard(&mut self, now: Timestamp) {
        let lock = add(now, HARD_LOCK);
        let timeline = &self.timeline.entries;
        let items = &self.items;
        // Near or after its target, only the removal of its item cancels a hard event.
        self.hard_seen.retain(|h| {
            if h.start <= lock {
                items.contains_key(&h.item)
            } else {
                timeline.iter().any(|e| same_event(e, h))
            }
        });
        for i in 0..4 {
            let gone = self.slots[i].as_ref().is_some_and(|s| {
                let queued_hard = s.role == DeckRole::Plan && s.is_hard() && s.not_started();
                let removed = !items.contains_key(&s.item.id);
                let moved =
                    s.entry.start > lock && !timeline.iter().any(|e| same_event(e, &s.entry));
                queued_hard && (removed || moved)
            });
            if gone && let Some(s) = self.slots[i].take() {
                let cmd = Cmd::new(self.mixer.now(), CommandKind::Stop(DeckId::ALL[i]));
                let _ = self.mixer.send(cmd.for_item(s.tag));
                self.dirty = true;
            }
        }
    }

    /// An operator cancelled this hard event: it does not load again.
    pub(crate) fn cancel_hard(&mut self, entry: &Entry) {
        self.hard_seen.retain(|h| !same_event(h, entry));
        self.cancelled.push(entry.clone());
    }

    /// After a station change, warns about each hard event near its target that the
    /// change moves or deletes. Such an event still airs, and the change applies after it:
    /// the occurrence it moved to does not air (see `superseded`). If the operator removed
    /// the event's item, the event is cancelled instead.
    fn warn_locked_hard(&mut self, station: &radio_model::Station, old: &Timeline, now: Timestamp) {
        let lock = add(now, HARD_LOCK);
        let in_window = |e: &Entry| e.start > now && e.start <= lock;
        let mut near: Vec<Entry> = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.is_hard() && s.not_started() && in_window(&s.entry))
            .map(|s| s.entry.clone())
            .collect();
        for h in self.hard_seen.iter().filter(|h| in_window(h)) {
            if !near.iter().any(|n| same_event(n, h)) {
                near.push(h.clone());
            }
        }
        if near.is_empty() {
            return;
        }
        let check = self.timeline.clone();
        for n in near {
            let unchanged = match n.origin {
                Origin::Rule { playlist, rule } => self
                    .last_station
                    .as_ref()
                    .and_then(|s| rules_of(s, playlist))
                    .zip(rules_of(station, playlist))
                    .is_some_and(|(a, b)| {
                        a.get(rule).zip(b.get(rule)).is_some_and(|(a, b)| {
                            a.trigger == b.trigger && a.placement == b.placement
                        })
                    }),
                _ => true,
            };
            let present = station
                .playlists
                .iter()
                .any(|p| p.enabled && p.items.iter().any(|i| i.id == n.item));
            if unchanged && present {
                continue;
            }
            // Removing the item is an operator cancel: `prune_hard` takes it off its deck.
            // A broken file is not a removal, so this looks at the station itself.
            let removed = !self
                .engine
                .state()
                .station
                .playlists
                .iter()
                .any(|p| p.items.iter().any(|i| i.id == n.item));
            if removed {
                let alarm = Alarm::HardEventCancelled {
                    item: n.item,
                    target: n.start,
                };
                self.alarms.raise_once(now, alarm);
                continue;
            }
            let alarm = Alarm::HardEventLocked {
                item: n.item,
                target: n.start,
            };
            self.alarms.raise_once(now, alarm);
            // The occurrence of the same rule that the edit moved this one to: new in this
            // plan, and the nearest to the old target.
            let target = |e: &Entry| e.fixed.as_ref().map(|f| f.target);
            let moved_to = check
                .entries
                .iter()
                .filter(|e| e.origin == n.origin && is_hard_entry(e))
                .filter(|e| target(e) != target(&n))
                .filter(|e| {
                    !old.entries
                        .iter()
                        .any(|o| o.origin == e.origin && target(o) == target(e))
                })
                .min_by_key(|e| e.start.duration_since(n.start).unsigned_abs())
                .cloned();
            // When the edit renumbered the rules, the same rule index is another rule:
            // its occurrence is no move of this event, and it airs.
            let old = self
                .planned_rules
                .iter()
                .find(|(e, _)| same_event(e, &n))
                .map(|(_, r)| r.clone())
                .or_else(|| {
                    let Origin::Rule { playlist, .. } = n.origin else {
                        return None;
                    };
                    self.last_station
                        .as_ref()
                        .and_then(|s| rules_of(s, playlist))
                });
            if renumbered(old, station, &n.origin) {
                continue;
            }
            if let Some(m) = moved_to
                && !self
                    .superseded
                    .iter()
                    .any(|x| same_occurrence(&x.entry, &m))
            {
                self.superseded.push(Superseded {
                    entry: m,
                    stand_in: n,
                });
            }
        }
    }

    /// Keeps the rule list of each hard event that the plan has for the first time.
    fn note_planned_rules(&mut self, station: &radio_model::Station, now: Timestamp) {
        const KEEP: Duration = Duration::from_secs(3600);
        self.planned_rules
            .retain(|(e, _)| e.start > now || since(e.start, now) < KEEP);
        for e in self.timeline.entries.iter().filter(|e| is_hard_entry(e)) {
            let Origin::Rule { playlist, .. } = e.origin else {
                continue;
            };
            if self.planned_rules.iter().any(|(k, _)| same_event(k, e)) {
                continue;
            }
            if let Some(rules) = rules_of(station, playlist) {
                self.planned_rules.push((e.clone(), rules));
            }
        }
    }

    /// Forgets superseded occurrences whose target passed. If the locked event that stood
    /// in for one did not air, the occurrence was suppressed without airing: that raises
    /// `HardEventMissed` for it.
    fn expire_superseded(&mut self, now: Timestamp) {
        let (past, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.superseded)
            .into_iter()
            .partition(|x| x.entry.start.max(x.stand_in.start) <= now);
        self.superseded = kept;
        for x in past {
            let from = sub(x.stand_in.start, HARD_LOCK);
            let aired = self
                .history
                .iter_since(from)
                .any(|p| p.item == x.stand_in.item);
            if !aired {
                let alarm = Alarm::HardEventMissed {
                    item: x.entry.item,
                    target: x.entry.start,
                };
                self.alarms.raise(now, alarm);
            }
        }
    }

    /// Ends the wait for row ids that never came, after 60 s, with an alarm.
    fn expire_ended(&mut self, now: Timestamp) {
        const WAIT: Duration = Duration::from_secs(60);
        let old: Vec<u64> = self
            .ended
            .iter()
            .filter(|(_, (end, _))| since(*end, now) >= WAIT)
            .map(|(tag, _)| *tag)
            .collect();
        for tag in old {
            self.ended.remove(&tag);
            let reason = "the log returned no row id within 60 s".to_string();
            self.alarms.raise(now, Alarm::LogFailed { reason });
        }
    }

    /// Makes sure each hard event within the preload time is on a deck. When no deck is
    /// free, it takes off a queued item that has not started. A late event still loads
    /// while it is within its tolerance. After that, playout raises `HardEventMissed`.
    fn ensure_hard(&mut self, now: Timestamp) {
        let horizon = add(now, self.cfg.preload);
        for e in self.timeline.entries.iter().filter(|e| {
            e.fixed.as_ref().is_some_and(|f| f.mode == FixMode::Hard)
                && e.start > now
                && e.start <= horizon
        }) {
            if !self.hard_seen.iter().any(|h| same_event(h, e)) {
                self.hard_seen.push(e.clone());
            }
        }
        // An event on a deck needs no load: match by event, not by start time.
        let on_deck: Vec<Entry> = self
            .slots
            .iter()
            .flatten()
            .map(|s| s.entry.clone())
            .collect();
        self.blocked.retain(|b| now < b.until);
        self.cancelled
            .retain(|c| since(c.start, now) < Duration::from_secs(24 * 3600));
        // The plan still has an event that the operator cancelled. It is not waited for,
        // so it is not missed either.
        self.hard_seen.retain(|h| {
            !on_deck.iter().any(|e| same_event(e, h))
                && !self.aired_fixed.iter().any(|e| same_occurrence(e, h))
                && !self.cancelled.iter().any(|c| same_event(c, h))
        });
        let mut missed = Vec::new();
        let mut due = Vec::new();
        for h in &self.hard_seen {
            let tolerance = h.fixed.as_ref().map_or(Duration::ZERO, |f| f.tolerance);
            if h.start <= now && since(h.start, now) > tolerance {
                missed.push(h.clone());
            } else {
                due.push(h.clone());
            }
        }
        for m in missed {
            self.hard_seen.retain(|h| !same_event(h, &m));
            let alarm = Alarm::HardEventMissed {
                item: m.item,
                target: m.start,
            };
            self.alarms.raise(now, alarm);
        }
        for entry in due {
            if self.aired_fixed.iter().any(|e| same_occurrence(e, &entry)) {
                continue;
            }
            let Some(item) = self.items.get(&entry.item).cloned() else {
                continue;
            };
            if self.cancelled.iter().any(|c| same_event(c, &entry)) {
                continue;
            }
            let Some(i) = self.timed_deck() else {
                if !self.evict_for_hard(entry.start, now) {
                    self.alarms
                        .raise_once(now, Alarm::HardEventNotLoaded { item: item.id });
                }
                return;
            };
            let late = entry.start <= now;
            if let Err(reason) = self.load(i, entry, item.clone(), DeckRole::Plan, Duration::ZERO) {
                self.skip_broken(&item, reason, now);
            } else if late && let Some(s) = &mut self.slots[i] {
                // The target passed but the tolerance holds: start as soon as possible.
                s.asap = true;
            }
        }
    }

    /// Takes the latest queued item that has not started off its deck, plan items
    /// first. That item cannot load again before the hard event at `target`. It
    /// returns false when there is no such item.
    fn evict_for_hard(&mut self, target: Timestamp, now: Timestamp) -> bool {
        let victim = (0..4)
            .filter_map(|i| self.slots[i].as_ref().map(|s| (i, s)))
            .filter(|(_, s)| s.not_started() && !s.is_hard())
            .filter(|(_, s)| matches!(s.role, DeckRole::Plan | DeckRole::Operator))
            .max_by_key(|(_, s)| (s.role == DeckRole::Plan, s.entry.start))
            .map(|(i, _)| i);
        let Some(i) = victim else { return false };
        if let Some(s) = self.slots[i].take() {
            let cmd = Cmd::new(self.mixer.now(), CommandKind::Stop(DeckId::ALL[i]));
            let _ = self.mixer.send(cmd.for_item(s.tag));
            self.blocked.push(Blocked {
                item: s.item.id,
                start: s.entry.start,
                until: target,
            });
            if s.role == DeckRole::Operator {
                let alarm = Alarm::OperatorItemEvicted {
                    deck: i as u8,
                    item: s.item.id,
                };
                self.alarms.raise(now, alarm);
            }
        }
        self.dirty = true;
        true
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;

    #[test]
    fn lost_output_is_visible_until_recovery() {
        let now: Timestamp = "2026-11-21T13:00:00Z".parse().unwrap();
        let cfg = PlayoutConfig::new(
            std::sync::Arc::new(crate::ManualClock::new(now)),
            OutputChoice::Offline,
        );
        let mut core = Core::new(Engine::open_in_memory().unwrap(), cfg).unwrap();
        core.output_event(crate::device::State::Lost("Test card".into()), now);
        core.publish(now);
        let view = core.view.lock().unwrap();
        assert!(matches!(view.outputs.local, LocalOutput::Lost { .. }));
        assert!(
            view.alarms
                .iter()
                .any(|a| matches!(a.alarm, Alarm::DeviceLost { .. }) && a.cleared.is_none())
        );
        drop(view);
        core.output_event(
            crate::device::State::Running("Default card".into(), SampleClock::new()),
            now,
        );
        assert!(matches!(core.local, LocalOutput::Device { .. }));
        assert!(core.alarms.recent.iter().all(|a| a.cleared.is_some()));
    }

    #[test]
    fn actual_fixed_start_checks_tolerance() {
        for (delay, expected) in [(0, false), (2, true)] {
            let now: Timestamp = "2026-11-21T13:00:00Z".parse().unwrap();
            let cfg = PlayoutConfig::new(
                std::sync::Arc::new(crate::ManualClock::new(now)),
                OutputChoice::Offline,
            );
            let mut core = Core::new(Engine::open_in_memory().unwrap(), cfg).unwrap();
            core.map.observe(now, 0);
            let item = radio_testkit::track(10, "Signal", "Test artist", 1);
            let entry = Entry {
                playlist: radio_model::PlaylistId(1),
                item: item.id,
                start: now,
                len: Duration::from_secs(1),
                origin: Origin::Manual,
                locked: true,
                fixed: Some(radio_scheduler::Fixed {
                    target: now,
                    mode: FixMode::Hard,
                    priority: 0,
                    frozen: false,
                    tolerance: Duration::from_secs(1),
                }),
                trim: Duration::ZERO,
            };
            let tag = core
                .load(0, entry, item, DeckRole::Plan, Duration::ZERO)
                .unwrap();
            core.on_started(DeckId::A, delay * u64::from(SAMPLE_RATE), Some(tag));
            assert_eq!(
                core.alarms
                    .recent
                    .iter()
                    .any(|a| matches!(a.alarm, Alarm::FixedLate { .. })),
                expected
            );
        }
    }
}
