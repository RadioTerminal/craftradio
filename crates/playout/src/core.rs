//! The control thread: plan, load, start, end and log. Everything here runs on one thread.

use crate::alarm::{Alarm, Alarms, RaisedAlarm};
use crate::clock::{Clock, ClockMap};
use crate::ops::{Carts, Check, Pending};
use crate::output::{ProgramSource, feed_tap};
use crate::view::{DeckPhase, DeckRole, DeckView, LocalOutput, OutputStatus, PlayoutView};
use crate::{OutputChoice, PlayoutConfig, PlayoutError, Reply};
use jiff::Timestamp;
use radio_audio::{
    AudioEvent, CHANNELS, Command as Cmd, CommandKind, DeckId, DeckState, LiveInput, LoadOptions,
    MeterReader, Mixer, MixerConfig, MixerHandle, SAMPLE_RATE, render_offline,
};
use radio_audio_device::{NullOutput, Output};
use radio_engine::{
    AsPlayed, Command, Engine, EngineError, EngineEvent, LogEntry, LogStatus, RowId,
};
use radio_model::{FixMode, Item, ItemId, Source};
use radio_scheduler::{Entry, Origin, PlanInput, PlayHistory, Played, Timeline, plan};
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Playout plans again at least this often.
const PLAN_EVERY: Duration = Duration::from_secs(1);
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
}

impl Slot {
    pub(crate) fn not_started(&self) -> bool {
        matches!(self.phase, Phase::Loading | Phase::Ready)
    }

    /// The planned end: the start plus the length, less the trim of the planner.
    pub(crate) fn planned_end(&self) -> Timestamp {
        add(
            self.entry.start,
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
        let carts = Carts::new(cfg.mixer.cart_slots);
        let opened = open_output(&cfg.output, cfg.mixer, None)?;
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
        };
        drop(opened.tap);
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

    pub(crate) fn subscribe(&mut self, tx: Sender<RaisedAlarm>) {
        self.alarms.subscribe(tx);
    }

    pub(crate) fn history(
        &self,
        since: Timestamp,
        limit: u32,
    ) -> Result<Vec<AsPlayed>, EngineError> {
        self.engine.history(since, limit)
    }

    pub(crate) fn render(&mut self, frames: usize) -> Result<Vec<f32>, PlayoutError> {
        if !self.is_offline() {
            return Err(PlayoutError::NotOffline);
        }
        self.tick();
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
        if let Some(step) = self.map.observe(now, self.mixer.now())
            && self.ticks > 0
        {
            let by_ms = i64::try_from(step.as_millis()).unwrap_or(i64::MAX);
            self.alarms.raise(now, Alarm::ClockStep { by_ms });
        }
        self.ticks += 1;
        let _ = self.drain_engine();
        if std::mem::take(&mut self.carts_dirty) || self.ticks == 1 {
            self.sync_carts(now);
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
        self.load_ahead(now);
        self.cue_starts(now);
        self.cue_ends(now);
        self.poll_states();
        self.publish(now);
        self.expire_replies();
    }

    fn on_audio(&mut self, ev: AudioEvent, now: Timestamp) {
        match ev {
            AudioEvent::Loaded(deck) => {
                if let Some(s) = &mut self.slots[deck.index()]
                    && s.phase == Phase::Loading
                {
                    s.phase = Phase::Ready;
                }
            }
            AudioEvent::LoadRejected(deck) => {
                self.slots[deck.index()] = None;
                self.dirty = true;
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
            AudioEvent::Silence { .. } | AudioEvent::SilenceEnded { .. } => {}
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
        s.entry.start = wall;
        self.dirty = true;
        self.log_start(deck.index(), wall);
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
        match self.engine.log_start(&entry) {
            Ok(row) => {
                if let Some(s) = &mut self.slots[i] {
                    s.row = Some(row);
                }
            }
            Err(e) => {
                let reason = e.to_string();
                self.alarms.raise(wall, Alarm::LogFailed { reason });
            }
        }
    }

    /// The deck is empty: complete its log row and free it.
    pub(crate) fn finish(&mut self, i: usize, now: Timestamp) {
        let Some(s) = self.slots[i].take() else {
            return;
        };
        if let Some(row) = s.row {
            let status = if s.cut {
                LogStatus::Interrupted
            } else {
                LogStatus::Aired
            };
            if let Err(e) = self.engine.log_end(row, now, status) {
                let reason = e.to_string();
                self.alarms.raise(now, Alarm::LogFailed { reason });
            }
        }
        self.dirty = true;
    }

    /// Moves the starts of the queued entries behind the item before them.
    fn chain(&mut self, now: Timestamp) {
        let mut order: Vec<usize> = (0..4)
            .filter(|&i| {
                self.slots[i]
                    .as_ref()
                    .is_some_and(|s| s.role == DeckRole::Plan)
            })
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
                s.entry.start = s.entry.start.max(now);
            }
            prev = Some((s.entry.start, s.item.clone(), s.entry.trim));
        }
    }

    fn replan(&mut self, now: Timestamp) {
        self.chain(now);
        let mut station = self.engine.state().station.clone();
        self.broken.retain(|_, at| since(*at, now) < RETRY_BROKEN);
        for p in &mut station.playlists {
            p.items.retain(|i| !self.broken.contains_key(&i.id));
        }
        self.items = station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| (i.id, i.clone())))
            .collect();
        let pinned: Vec<Entry> = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.role == DeckRole::Plan)
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
        let prev = self.planned.then_some(&self.timeline);
        let tl = plan(&input, &opts, prev);
        if tl != self.timeline {
            self.timeline_changed = true;
        }
        self.timeline = tl;
        self.planned = true;
        self.dirty = false;
        self.last_plan = Some(now);
    }

    fn free_deck(&self) -> Option<usize> {
        (0..4).map(|k| (self.next_deck + k) % 4).find(|&i| {
            self.slots[i].is_none() && self.mixer.deck_state(DeckId::ALL[i]) == DeckState::Empty
        })
    }

    /// Loads the next planned entries that start within the preload time.
    fn load_ahead(&mut self, now: Timestamp) {
        let plan_slots = || {
            self.slots
                .iter()
                .flatten()
                .filter(|s| s.role == DeckRole::Plan)
        };
        let last = plan_slots().map(|s| s.entry.start).max();
        let assigned: Vec<(ItemId, Timestamp)> = plan_slots()
            .map(|s| (s.entry.item, s.entry.start))
            .collect();
        let horizon = add(now, self.cfg.preload);
        let next: Vec<Entry> = self
            .timeline
            .entries
            .iter()
            .filter(|e| !assigned.contains(&(e.item, e.start)))
            .filter(|e| last.is_none_or(|l| e.start > l))
            .take_while(|e| e.start <= horizon)
            .take(4)
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
            cue_in: cue_in + offset,
            fade_in: if offset.is_zero() {
                fade_in
            } else {
                Duration::ZERO
            },
            item_tag: Some(tag),
        };
        match &item.source {
            Source::File { path } => self
                .mixer
                .load(deck, path, options)
                .map_err(|e| e.to_string())?,
            Source::Stream { .. } => return Err("internet streams are not supported yet".into()),
            Source::LiveInput { .. } => return Err("live input is not supported yet".into()),
        }
        self.slots[i] = Some(Slot {
            tag,
            entry,
            item,
            role,
            phase: Phase::Loading,
            started: None,
            end_sent: false,
            cut: false,
            row: None,
        });
        self.next_deck = (i + 1) % 4;
        Ok(tag)
    }

    /// Sends `Play` for each queued entry that starts within the command lead.
    fn cue_starts(&mut self, now: Timestamp) {
        let soon = add(now, self.cfg.command_lead);
        for i in 0..4 {
            let Some(s) = &self.slots[i] else { continue };
            if s.role != DeckRole::Plan || !s.not_started() || s.entry.start > soon {
                continue;
            }
            if !self.may_start(s) {
                continue;
            }
            let at = self.map.sample_at(s.entry.start).max(self.mixer.now());
            self.play_at(i, at);
        }
    }

    /// Sends `Play` for deck `i` at frame `at`.
    pub(crate) fn play_at(&mut self, i: usize, at: u64) {
        let Some(s) = &mut self.slots[i] else { return };
        let cmd = Cmd::new(at, CommandKind::Play(DeckId::ALL[i])).for_item(s.tag);
        if self.mixer.send(cmd).is_ok() {
            s.phase = Phase::Cued(at);
        }
    }

    /// The first hard event in the timeline after `start` and before `end`.
    fn hard_between(&self, start: Timestamp, end: Timestamp) -> Option<Timestamp> {
        self.timeline
            .entries
            .iter()
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
            let ends = matches!(s.role, DeckRole::Plan | DeckRole::Operator);
            if !ends || s.not_started() || s.end_sent || s.phase == Phase::Paused {
                continue;
            }
            let start = s.entry.start;
            let mut end = s.planned_end();
            let hard = self.hard_between(start, end);
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
                    Phase::Loading => DeckPhase::Loading,
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
        view.waiting_for_next = waiting;
        view.decks = decks;
        view.outputs.local = self.local.clone();
        view.alarms = self.alarms.recent.iter().cloned().collect();
        if self.timeline_changed {
            view.timeline = self.timeline.clone();
            self.timeline_changed = false;
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
        let _ = self.engine.flush();
    }
}
