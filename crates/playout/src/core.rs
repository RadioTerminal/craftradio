//! The control thread: plan, load, start, end and log. Everything here runs on one thread.

use crate::alarm::{Alarm, Alarms, RaisedAlarm};
use crate::clock::{Clock, ClockMap};
use crate::deadair::{Analysis, DeadAir};
use crate::ops::{Carts, Check, Pending};
use crate::output::{ProgramSource, feed_tap};
use crate::streams::StreamSlot;
use crate::view::{DeckPhase, DeckRole, DeckView, LocalOutput, OutputStatus, PlayoutView};
use crate::work::{Done, Workers};
use crate::{OutputChoice, PlayoutConfig, PlayoutError, Reply};
use jiff::Timestamp;
use radio_audio::Decoder;
use radio_audio::{
    AudioEvent, CHANNELS, Command as Cmd, CommandKind, DeckId, DeckState, LiveInput, LoadOptions,
    MeterReader, Mixer, MixerConfig, MixerHandle, SAMPLE_RATE, render_offline,
};
use radio_audio_device::{NullOutput, Output, SampleClock};
use radio_engine::{Command, Engine, EngineEvent, LogEntry, LogHandle, LogStatus, RowId};
use radio_model::{FixMode, Item, ItemId, Source};
use radio_scheduler::{Entry, Origin, PlanInput, PlayHistory, Played, Timeline, plan};
use radio_stream::{IcecastClient, IcecastHandle};
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
fn still_planned(prev: &Timeline, fresh: &Timeline) -> Timeline {
    let mut kept = prev.clone();
    kept.entries.retain(|e| match (&e.fixed, &e.origin) {
        (Some(_), _) => fresh.entries.iter().any(|f| same_event(f, e)),
        (None, Origin::Rule { .. }) => fresh.entries.iter().any(|f| f.origin == e.origin),
        _ => true,
    });
    kept.fits.retain(|fit| {
        fresh
            .fits
            .iter()
            .any(|f| f.playlist == fit.playlist && f.target == fit.target)
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
    old: Option<&radio_model::Station>,
    new: &radio_model::Station,
    origin: &Origin,
) -> bool {
    let Origin::Rule { playlist, rule } = origin else {
        return true;
    };
    let rules = |s: &radio_model::Station| {
        s.playlists
            .iter()
            .find(|p| p.id == *playlist)
            .map(|p| p.rules.clone())
    };
    match (old.and_then(rules), rules(new)) {
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
/// The most planned entries that wait on decks. The other decks stay free for operators.
const MAX_QUEUED: usize = 2;
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
    /// Playout sent the fade at the planned end.
    pub(crate) end_sent: bool,
    /// Something ended the item before its planned end.
    pub(crate) cut: bool,
    pub(crate) row: Option<RowId>,
    pub(crate) stream: Option<StreamSlot>,
    /// The load starts this far after the cue-in (a resume).
    pub(crate) offset: Duration,
    /// The mixer reported that a fade-out began.
    pub(crate) fading: bool,
    /// Start as soon as the deck is ready, whatever the plan says.
    pub(crate) asap: bool,
    pub(crate) options: LoadOptions,
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

    pub(crate) fn is_hard(&self) -> bool {
        self.entry
            .fixed
            .as_ref()
            .is_some_and(|f| f.mode == FixMode::Hard)
    }

    /// Where the item really started at its cue-in, once it is on air. A hard event keeps
    /// its target in `entry.start`, so a late one starts after that.
    pub(crate) fn real_start(&self) -> Timestamp {
        self.started
            .map_or(self.entry.start, |w| sub(w, self.offset))
    }

    /// The planned end: the real start plus the length, less the trim of the planner.
    pub(crate) fn planned_end(&self) -> Timestamp {
        add(
            self.real_start(),
            self.entry.len.saturating_sub(self.entry.trim),
        )
    }
}

enum Driver {
    Offline {
        mixer: Box<Mixer>,
        tap: Option<rtrb::Producer<f32>>,
    },
    Null {
        _out: NullOutput,
    },
    Device {
        _out: Output,
    },
}

struct OpenedOutput {
    /// The clock of a real or null output, for its latency.
    clock: Option<SampleClock>,
    driver: Driver,
    handle: MixerHandle,
    live: LiveInput,
    local: LocalOutput,
    tap: Option<rtrb::Consumer<f32>>,
    fallback: Option<String>,
}

type Tap = (Option<rtrb::Producer<f32>>, Option<rtrb::Consumer<f32>>);

fn tap_ring(samples: Option<usize>) -> Tap {
    samples
        .map(rtrb::RingBuffer::<f32>::new)
        .map_or((None, None), |(p, c)| (Some(p), Some(c)))
}

/// Opens the output that `choice` asks for. If a device fails or does not run at 48
/// kHz, the null output runs instead. It returns the reason too.
fn open_output(
    choice: &OutputChoice,
    config: MixerConfig,
    tap: Option<usize>,
) -> Result<OpenedOutput, PlayoutError> {
    let (mixer, handle, live) = Mixer::new(config);
    let (tap_tx, tap_rx) = tap_ring(tap);
    let mut fallback = None;
    let (mixer, handle, live, tap_tx, tap_rx) = match choice {
        OutputChoice::Offline => {
            return Ok(OpenedOutput {
                clock: None,
                driver: Driver::Offline {
                    mixer: Box::new(mixer),
                    tap: tap_tx,
                },
                handle,
                live,
                local: LocalOutput::Offline,
                tap: tap_rx,
                fallback,
            });
        }
        OutputChoice::Device { name } => {
            let source = Box::new(ProgramSource::new(mixer, tap_tx));
            match Output::open(name.as_deref(), SAMPLE_RATE, source) {
                Ok(out) if out.sample_rate() == SAMPLE_RATE => {
                    let local = LocalOutput::Device {
                        name: out.device_name().to_string(),
                    };
                    return Ok(OpenedOutput {
                        clock: Some(out.clock()),
                        driver: Driver::Device { _out: out },
                        handle,
                        live,
                        local,
                        tap: tap_rx,
                        fallback,
                    });
                }
                Ok(out) => {
                    fallback = Some(format!(
                        "{} runs at {} Hz, not {SAMPLE_RATE} Hz",
                        out.device_name(),
                        out.sample_rate()
                    ));
                }
                Err(e) => fallback = Some(e.to_string()),
            }
            let (mixer, handle, live) = Mixer::new(config);
            let (tap_tx, tap_rx) = tap_ring(tap);
            (mixer, handle, live, tap_tx, tap_rx)
        }
        OutputChoice::Null => (mixer, handle, live, tap_tx, tap_rx),
    };
    let source = Box::new(ProgramSource::new(mixer, tap_tx));
    let null = NullOutput::start(SAMPLE_RATE, CHANNELS as u16, source)
        .map_err(|e| PlayoutError::Output(e.to_string()))?;
    Ok(OpenedOutput {
        clock: Some(null.clock()),
        driver: Driver::Null { _out: null },
        handle,
        live,
        local: LocalOutput::Null,
        tap: tap_rx,
        fallback,
    })
}

pub(crate) struct Core {
    pub(crate) engine: Engine,
    engine_rx: Receiver<EngineEvent>,
    pub(crate) mixer: MixerHandle,
    driver: Driver,
    _live: LiveInput,
    local: LocalOutput,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) map: ClockMap,
    pub(crate) cfg: PlayoutConfig,
    pub(crate) timeline: Timeline,
    planned: bool,
    timeline_changed: bool,
    last_plan: Option<Timestamp>,
    pub(crate) dirty: bool,
    pub(crate) history: PlayHistory,
    pub(crate) slots: [Option<Slot>; 4],
    next_deck: usize,
    next_tag: u64,
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
    pub(crate) dead_air: DeadAir,
    pub(crate) next_bed: usize,
    pub(crate) icecast: Option<IcecastHandle>,
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
    /// Occurrences that a late edit moved a locked hard event to. The locked event airs at
    /// its old time instead, so these do not air. Kept until their target passes.
    superseded: Vec<Superseded>,
    out_clock: Option<SampleClock>,
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
        // Four seconds of Program between the mixer and the encoder.
        let tap = cfg
            .icecast
            .as_ref()
            .map(|_| 4 * SAMPLE_RATE as usize * CHANNELS);
        let mut opened = open_output(&cfg.output, cfg.mixer, tap)?;
        let icecast = cfg
            .icecast
            .clone()
            .zip(opened.tap.take())
            .map(|(ice, rx)| IcecastClient::start(ice, rx));
        let now = cfg.clock.now();
        let mut alarms = Alarms::new(cfg.webhook.clone());
        if let Some(reason) = opened.fallback {
            alarms.raise(now, Alarm::OutputFallback { reason });
        }
        let view = PlayoutView {
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
            },
            stream_states: Vec::new(),
            alarms: Vec::new(),
            alarms_dropped: 0,
            state: Arc::new(engine.state().clone()),
            revision: 0,
        };
        Ok(Core {
            engine,
            engine_rx,
            mixer: opened.handle,
            driver: opened.driver,
            _live: opened.live,
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
            dead_air: DeadAir::default(),
            next_bed: 0,
            icecast,
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
            superseded: Vec::new(),
            out_clock: opened.clock,
        })
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
        let Driver::Offline { mixer, tap } = &mut self.driver else {
            return Err(PlayoutError::NotOffline);
        };
        let out = render_offline(mixer, frames);
        feed_tap(tap, &out);
        Ok(out)
    }

    pub(crate) fn dispatch(&mut self, cmd: Command, reply: Reply) {
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
    fn drain_engine(&mut self) -> Vec<Check> {
        let mut checks = Vec::new();
        while let Ok(ev) = self.engine_rx.try_recv() {
            match ev {
                EngineEvent::StateChanged => {
                    self.dirty = true;
                    self.carts_dirty = true;
                    self.state_dirty = true;
                }
                EngineEvent::AudioIntent(intent) => {
                    let now = self.clock.now();
                    checks.push(self.intent(intent, now));
                }
            }
        }
        checks
    }

    /// One pass of the control loop.
    pub(crate) fn tick(&mut self) {
        let now = self.clock.now();
        if let Some(c) = &self.out_clock {
            self.map.latency = c.latency();
        }
        if let Some(step) = self.map.observe(now, self.mixer.now())
            && self.ticks > 0
        {
            let by_ms = i64::try_from(step.as_millis()).unwrap_or(i64::MAX);
            self.alarms.raise(now, Alarm::ClockStep { by_ms });
        }
        self.ticks += 1;
        if self.ticks == 1 {
            self.resume(now);
        }
        let _ = self.drain_engine();
        self.take_done(now);
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
        self.load_ahead(now);
        self.cue_starts(now);
        self.cue_ends(now);
        self.check_streams(now);
        self.check_dead_air(now);
        self.poll_states();
        self.stop_took_effect();
        self.sync_engine_decks();
        self.expire_ended(now);
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
        // A hard event keeps its target as its start, so later plans still know it.
        if !s.is_hard() {
            s.entry.start = sub(wall, s.offset);
        }
        self.dirty = true;
        self.save_due = true;
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
        order.sort_by_key(|&i| self.slots[i].as_ref().map(|s| s.entry.start));
        let mut prev: Option<(Timestamp, Item, Duration)> = None;
        for i in order {
            let Some(s) = &mut self.slots[i] else {
                continue;
            };
            if s.not_started() {
                if let Some((start, item, trim)) = &prev
                    && s.entry.fixed.is_none()
                {
                    let gap = radio_timing::advance(item, Some(&s.item)).saturating_sub(*trim);
                    s.entry.start = add(*start, gap);
                }
                // A hard event keeps its target, even when it starts late.
                if s.offset.is_zero() && !s.is_hard() {
                    s.entry.start = s.entry.start.max(now);
                }
            }
            // The next item follows the real start: a late hard event plays in full.
            let start = match s.started {
                Some(_) => s.real_start(),
                None if s.is_hard() => s.entry.start.max(now),
                None => s.entry.start,
            };
            prev = Some((start, s.item.clone(), s.entry.trim));
        }
    }

    fn replan(&mut self, now: Timestamp) {
        self.chain(now);
        self.prune_history(now);
        let mut station = self.engine.state().station.clone();
        self.apply_analysis(&mut station);
        self.broken.retain(|_, at| since(*at, now) < RETRY_BROKEN);
        for p in &mut station.playlists {
            p.items.retain(|i| !self.broken.contains_key(&i.id));
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
        let pinned: Vec<Entry> = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.pinned() && !free_hard(s))
            .map(|s| s.entry.clone())
            .collect();
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
        let filtered = (self.planned && edited).then(|| {
            let fresh = plan(&input, &opts, None);
            still_planned(&self.timeline, &fresh)
        });
        let prev = match &filtered {
            Some(f) => Some(f),
            None => self.planned.then_some(&self.timeline),
        };
        let mut tl = plan(&input, &opts, prev);
        self.expire_superseded(now);
        drop_superseded(&mut tl, &self.superseded);
        let old = std::mem::replace(&mut self.timeline, tl);
        if edited {
            if self.planned {
                self.warn_locked_hard(&station, &pinned, &old, now);
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
        self.prune_hard(now);
        self.planned = true;
        self.dirty = false;
        self.last_plan = Some(now);
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

    pub(crate) fn free_deck(&self) -> Option<usize> {
        (0..4).map(|k| (self.next_deck + k) % 4).find(|&i| {
            self.slots[i].is_none() && self.mixer.deck_state(DeckId::ALL[i]) == DeckState::Empty
        })
    }

    /// Loads the next planned entries that start within the preload time.
    fn load_ahead(&mut self, now: Timestamp) {
        // Hard events load on their own (see `ensure_hard`) and do not count here.
        let plan_slots = || self.slots.iter().flatten().filter(|s| s.pinned());
        let last = plan_slots()
            .filter(|s| !s.is_hard())
            .map(|s| s.entry.start)
            .max();
        let queued = plan_slots()
            .filter(|s| s.not_started() && !s.is_hard())
            .count();
        let assigned = self.assigned();
        let horizon = add(now, self.cfg.preload);
        let next: Vec<Entry> = self
            .timeline
            .entries
            .iter()
            .filter(|e| !assigned.contains(&(e.item, e.start)))
            .filter(|e| !e.fixed.as_ref().is_some_and(|f| f.mode == FixMode::Hard))
            .filter(|e| {
                !self
                    .blocked
                    .iter()
                    .any(|b| b.item == e.item && b.start == e.start && now < b.until)
            })
            .filter(|e| last.is_none_or(|l| e.start > l))
            .take_while(|e| e.start <= horizon)
            .take(MAX_QUEUED.saturating_sub(queued))
            .cloned()
            .collect();
        for entry in next {
            let Some(item) = self.items.get(&entry.item).cloned() else {
                continue;
            };
            if matches!(item.source, Source::Stream { .. })
                && entry.start > add(now, self.cfg.stream_preroll)
            {
                break;
            }
            let Some(i) = self.free_deck() else {
                break;
            };
            if let Err(reason) = self.load(i, entry, item.clone(), DeckRole::Plan, Duration::ZERO) {
                self.skip_broken(&item, reason, now);
                break;
            }
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
            cue_in: cue_in.saturating_add(offset),
            fade_in: if offset.is_zero() {
                fade_in
            } else {
                Duration::ZERO
            },
            item_tag: Some(tag),
        };
        let mut stream = None;
        let mut phase = Phase::Loading;
        match &item.source {
            Source::File { path } => {
                let (path, cue_in) = (path.clone(), options.cue_in);
                self.work.run(move || Done::Opened {
                    tag,
                    result: Decoder::open_at(&path, cue_in).map_err(|e| e.to_string()),
                });
                phase = Phase::Opening;
            }
            Source::Stream { url, keep_alive } => {
                stream = Some(self.load_stream(deck, url, *keep_alive, options)?);
            }
            Source::LiveInput { .. } => return Err("live input is not supported yet".into()),
        }
        self.slots[i] = Some(Slot {
            tag,
            entry,
            item,
            role,
            phase,
            started: None,
            end_sent: false,
            cut: false,
            row: None,
            stream,
            offset,
            fading: false,
            asap: false,
            options,
        });
        self.next_deck = (i + 1) % 4;
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
        let Some(s) = &mut self.slots[i] else { return };
        if s.phase == Phase::Opening {
            s.asap = true;
            return;
        }
        let cmd = Cmd::new(at, CommandKind::Play(DeckId::ALL[i])).for_item(s.tag);
        if self.mixer.send(cmd).is_ok() {
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
            if s.not_started() || s.end_sent || s.phase == Phase::Paused {
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
            let hard = self.hard_between(&s.entry, start, end);
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
            },
        });
        let Ok(mut view) = self.view.lock() else {
            return;
        };
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
        view.outputs.icecast = self.icecast.as_ref().map(IcecastHandle::status);
        if self.timeline_changed {
            view.timeline = self.timeline.clone();
            self.timeline_changed = false;
        }
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
        while let Ok(done) = self.work.done_rx.try_recv() {
            match done {
                Done::Opened { tag, result } => self.on_opened(tag, result, now),
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
                Done::Cart(i, item, result) => self.on_cart(i, item, result, now),
            }
        }
    }

    /// A worker opened (or failed to open) the file of the load with `tag`.
    fn on_opened(&mut self, tag: u64, result: Result<Decoder, String>, now: Timestamp) {
        let Some(i) = (0..4).find(|&i| {
            self.slots[i]
                .as_ref()
                .is_some_and(|s| s.tag == tag && s.phase == Phase::Opening)
        }) else {
            return;
        };
        let deck = DeckId::ALL[i];
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
                    DeckRole::Emergency => self.dead_air_file_failed(),
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
    fn warn_locked_hard(
        &mut self,
        station: &radio_model::Station,
        pinned: &[Entry],
        old: &Timeline,
        now: Timestamp,
    ) {
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
        // Plan once more without these events, to see if the station still has them.
        let others: Vec<Entry> = pinned
            .iter()
            .filter(|p| !near.iter().any(|n| same_event(n, p)))
            .cloned()
            .collect();
        let input = PlanInput {
            station,
            history: &self.history,
            now,
            horizon: self.cfg.horizon,
            pinned: &others,
        };
        let mut opts = self.cfg.plan;
        opts.resume_rotation = true;
        let check = plan(&input, &opts, None);
        for n in near {
            if check.entries.iter().any(|e| same_event(e, &n)) {
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
            if renumbered(self.last_station.as_ref(), station, &n.origin) {
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

    /// Forgets superseded occurrences whose target passed. If the locked event that stood
    /// in for one did not air, the occurrence was suppressed without airing: that raises
    /// `HardEventMissed` for it.
    fn expire_superseded(&mut self, now: Timestamp) {
        let (past, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.superseded)
            .into_iter()
            .partition(|x| x.entry.start <= now);
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
        self.hard_seen
            .retain(|h| !on_deck.iter().any(|e| same_event(e, h)));
        self.blocked.retain(|b| now < b.until);
        self.cancelled
            .retain(|c| since(c.start, now) < Duration::from_secs(24 * 3600));
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
            let Some(item) = self.items.get(&entry.item).cloned() else {
                continue;
            };
            if self.cancelled.iter().any(|c| same_event(c, &entry)) {
                continue;
            }
            let Some(i) = self.free_deck() else {
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
