//! The mixer: four decks, carts, live input, PFL and Program out.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::deck::{DeckShared, DeckSource};
use crate::meter::{MeterBank, SilenceChange, SilenceState};
use crate::{
    AudioError, AudioEvent, CHANNELS, Clip, Command, CommandKind, DeckFeeder, DeckId, DeckState,
    Decoder, LoadOptions, MeterSnapshot, SAMPLE_RATE, SilenceDetector, Strip,
};

/// The number of cart voices that can play at the same time.
pub const CART_VOICES: usize = 16;

const COMMAND_CAPACITY: usize = 256;
const CONTROL_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 256;
/// The length of a fader ramp: 5 ms.
const GAIN_RAMP: u64 = SAMPLE_RATE as u64 / 200;
/// The limiter ceiling: −1 dBFS.
const CEILING: f32 = 0.891_250_9;
/// The limiter is linear below this level (about −3.5 dBFS).
const KNEE: f32 = CEILING * 0.75;

/// The settings of a mixer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MixerConfig {
    /// The number of cartwall slots.
    pub cart_slots: usize,
    /// The length of the ring of each deck.
    pub deck_buffer: Duration,
    /// The length of the ring of the live input.
    pub live_buffer: Duration,
    /// The silence detector on Program.
    pub silence: SilenceDetector,
    /// The hold time of the peak meters.
    pub peak_hold: Duration,
}

impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            cart_slots: 48,
            deck_buffer: Duration::from_secs(4),
            live_buffer: Duration::from_millis(500),
            silence: SilenceDetector::default(),
            peak_hold: Duration::from_millis(1500),
        }
    }
}

fn duration_frames(d: Duration) -> u64 {
    let frames = d.as_nanos() * u128::from(SAMPLE_RATE) / 1_000_000_000;
    u64::try_from(frames).unwrap_or(u64::MAX)
}

fn ring_samples(d: Duration) -> usize {
    usize::try_from(duration_frames(d))
        .unwrap_or(usize::MAX / 4)
        .max(1024)
        * CHANNELS
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// A soft clip with a ceiling at −1 dBFS. It does not change levels below the knee.
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let range = CEILING - KNEE;
    let y = KNEE + range * ((a - KNEE) / range).tanh();
    y.copysign(x)
}

/// Items that go from the handle to the render thread, outside the sample clock.
enum Control {
    Load { deck: DeckId, source: DeckSource },
    SetCart { slot: usize, clip: Option<Clip> },
}

/// Heap items that the render thread gives back, so that the handle frees them.
enum Retired {
    Deck(DeckSource),
    Clip(Clip),
}

/// Atomics that the render thread and the handle share.
struct Shared {
    now: AtomicU64,
    pfl: AtomicU8,
}

/// A linear gain ramp.
#[derive(Copy, Clone, Debug)]
struct Ramp {
    current: f32,
    target: f32,
    step: f32,
}

impl Ramp {
    const UNITY: Ramp = Ramp {
        current: 1.0,
        target: 1.0,
        step: 0.0,
    };

    fn set(&mut self, target: f32, frames: u64) {
        self.target = target;
        self.step = (target - self.current) / frames.max(1) as f32;
    }

    fn next(&mut self) -> f32 {
        if self.current != self.target {
            let next = self.current + self.step;
            let passed = (self.step > 0.0 && next >= self.target)
                || (self.step <= 0.0 && next <= self.target);
            self.current = if passed { self.target } else { next };
        }
        self.current
    }
}

/// The ducker of decks and live input, with a linear ramp in dB.
#[derive(Copy, Clone, Debug)]
struct Duck {
    current_db: f32,
    target_db: f32,
    step_db: f32,
    release: u64,
    gain: f32,
}

impl Duck {
    const OFF: Duck = Duck {
        current_db: 0.0,
        target_db: 0.0,
        step_db: 0.0,
        release: GAIN_RAMP,
        gain: 1.0,
    };

    fn to(&mut self, target_db: f32, frames: u64) {
        self.target_db = target_db;
        self.step_db = (target_db - self.current_db).abs() / frames.max(1) as f32;
    }

    fn next(&mut self) -> f32 {
        if self.current_db != self.target_db {
            let diff = self.target_db - self.current_db;
            self.current_db = if diff.abs() <= self.step_db {
                self.target_db
            } else {
                self.current_db + self.step_db.copysign(diff)
            };
            self.gain = db_to_gain(self.current_db);
        }
        self.gain
    }
}

/// One deck on the render thread.
#[derive(Default)]
struct DeckSlot {
    source: Option<DeckSource>,
    playing: bool,
    paused: bool,
    played: u64,
    /// The remaining and the total frames of a fade-out.
    fade: Option<(u64, u64)>,
    underrun: bool,
}

/// One cart voice.
#[derive(Copy, Clone, Debug, Default)]
struct Voice {
    slot: usize,
    pos: usize,
    active: bool,
}

/// What happened to a deck in one frame.
enum DeckStep {
    Frame([f32; 2]),
    Silent,
    Ended,
    Underrun,
}

/// The playout mixer with decks A–D, a cart bus, a live bus, a PFL bus and Program
/// out. Real-time invariant: [`Mixer::render`] runs on the audio thread. It does no heap
/// allocation or deallocation, takes no locks, makes no system calls and writes no
/// logs. The constructor allocates all buffers. The render thread talks to other
/// threads only through lock-free rtrb rings, atomics and a triple buffer. It gives heap
/// items back to the [`MixerHandle`], and the handle frees them.
pub struct Mixer {
    commands: Consumer<Command>,
    pending: Vec<Command>,
    next_due: u64,
    control: Consumer<Control>,
    retired: Producer<Retired>,
    events: Producer<AudioEvent>,
    meters_out: triple_buffer::Input<MeterSnapshot>,
    live: Consumer<f32>,
    decks: [DeckSlot; 4],
    gains: [Ramp; Strip::COUNT],
    carts: Vec<Option<Clip>>,
    voices: [Voice; CART_VOICES],
    duck: Duck,
    meters: MeterBank,
    silence: SilenceState,
    shared: Arc<Shared>,
    sample: u64,
}

impl Mixer {
    /// Makes a mixer, the handle that controls it, and the producer of its live input.
    pub fn new(config: MixerConfig) -> (Mixer, MixerHandle, LiveInput) {
        let (cmd_tx, cmd_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (ctl_tx, ctl_rx) = RingBuffer::new(CONTROL_CAPACITY);
        // The bound holds every heap item in the mixer and in the control ring.
        let (ret_tx, ret_rx) = RingBuffer::new(CONTROL_CAPACITY + 4 + config.cart_slots + 1);
        let (ev_tx, ev_rx) = RingBuffer::new(EVENT_CAPACITY);
        let (live_tx, live_rx) = RingBuffer::new(ring_samples(config.live_buffer));
        let (meter_in, meter_out) = triple_buffer::triple_buffer(&MeterSnapshot::default());
        let shared = Arc::new(Shared {
            now: AtomicU64::new(0),
            pfl: AtomicU8::new(0),
        });
        let mixer = Mixer {
            commands: cmd_rx,
            pending: Vec::with_capacity(COMMAND_CAPACITY),
            next_due: u64::MAX,
            control: ctl_rx,
            retired: ret_tx,
            events: ev_tx,
            meters_out: meter_in,
            live: live_rx,
            decks: Default::default(),
            gains: [Ramp::UNITY; Strip::COUNT],
            carts: vec![None; config.cart_slots],
            voices: [Voice::default(); CART_VOICES],
            duck: Duck::OFF,
            meters: MeterBank::new(config.peak_hold),
            silence: SilenceState::new(config.silence),
            shared: Arc::clone(&shared),
            sample: 0,
        };
        let handle = MixerHandle {
            commands: cmd_tx,
            control: ctl_tx,
            retired: ret_rx,
            events: ev_rx,
            meters: meter_out,
            decks: Default::default(),
            feeders: Default::default(),
            shared,
            deck_buffer: ring_samples(config.deck_buffer),
            cart_slots: config.cart_slots,
        };
        (mixer, handle, LiveInput { producer: live_tx })
    }

    /// Returns the absolute count of rendered frames.
    pub fn now(&self) -> u64 {
        self.sample
    }

    /// Renders Program and PFL as stereo interleaved f32 at 48 kHz. The frame count is
    /// `program.len() / 2`. PFL gets the same frames as far as it has room, then silence.
    /// This method is real-time safe, see [`Mixer`].
    pub fn render(&mut self, program: &mut [f32], pfl: &mut [f32]) {
        self.take_control();
        self.take_commands();
        let pfl_mask = self.shared.pfl.load(Ordering::Relaxed);
        let (cue_frames, cue_rest) = pfl.as_chunks_mut::<CHANNELS>();
        let (out_frames, out_rest) = program.as_chunks_mut::<CHANNELS>();
        let mut cue_iter = cue_frames.iter_mut();
        for out in out_frames.iter_mut() {
            if self.sample >= self.next_due {
                self.apply_due();
            }
            let (y, cue) = self.frame(pfl_mask);
            *out = y;
            if let Some(c) = cue_iter.next() {
                *c = cue;
            }
        }
        out_rest.fill(0.0);
        for c in cue_iter {
            *c = [0.0; CHANNELS];
        }
        cue_rest.fill(0.0);
        self.meters_out.write(self.meters.snapshot(self.sample));
        self.shared.now.store(self.sample, Ordering::Release);
    }

    /// Waits until each loaded deck has `samples` in its ring, or reached its end.
    fn wait_for_decks(&self, samples: usize, timeout: Duration) {
        let start = Instant::now();
        loop {
            let starving = self.decks.iter().any(|slot| {
                slot.source.as_ref().is_some_and(|src| {
                    !src.shared.eof.load(Ordering::Acquire) && src.consumer.slots() < samples
                })
            });
            if !starving || start.elapsed() > timeout {
                return;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    fn take_control(&mut self) {
        while let Ok(item) = self.control.pop() {
            match item {
                Control::Load { deck, source } => {
                    let slot = &mut self.decks[deck.index()];
                    if slot.playing {
                        self.retire(Retired::Deck(source));
                        continue;
                    }
                    let old = slot.source.replace(source);
                    *slot = DeckSlot {
                        source: slot.source.take(),
                        ..DeckSlot::default()
                    };
                    if let Some(old) = old {
                        self.retire(Retired::Deck(old));
                    }
                }
                Control::SetCart { slot, clip } => match self.carts.get_mut(slot) {
                    Some(entry) => {
                        let old = std::mem::replace(entry, clip);
                        for v in self.voices.iter_mut().filter(|v| v.slot == slot) {
                            v.active = false;
                        }
                        if let Some(old) = old {
                            self.retire(Retired::Clip(old));
                        }
                    }
                    None => {
                        if let Some(clip) = clip {
                            self.retire(Retired::Clip(clip));
                        }
                    }
                },
            }
        }
    }

    fn take_commands(&mut self) {
        while self.pending.len() < COMMAND_CAPACITY {
            let Ok(cmd) = self.commands.pop() else { break };
            self.next_due = self.next_due.min(cmd.at_sample);
            self.pending.push(cmd);
        }
    }

    /// Applies every due command, in the order that the handle sent them.
    fn apply_due(&mut self) {
        let mut next = u64::MAX;
        let mut i = 0;
        while let Some(cmd) = self.pending.get(i).copied() {
            if cmd.at_sample <= self.sample {
                self.pending.remove(i);
                self.apply(cmd.kind);
            } else {
                next = next.min(cmd.at_sample);
                i += 1;
            }
        }
        self.next_due = next;
    }

    fn apply(&mut self, kind: CommandKind) {
        match kind {
            CommandKind::Play(deck) => {
                let slot = &mut self.decks[deck.index()];
                if let Some(src) = &slot.source
                    && (!slot.playing || slot.paused)
                {
                    slot.playing = true;
                    slot.paused = false;
                    src.shared.set_state(DeckState::Playing);
                }
            }
            CommandKind::Pause(deck) => {
                let slot = &mut self.decks[deck.index()];
                if let Some(src) = &slot.source
                    && slot.playing
                    && !slot.paused
                {
                    slot.paused = true;
                    src.shared.set_state(DeckState::Paused);
                }
            }
            CommandKind::Stop(deck) => self.unload(deck),
            CommandKind::FadeOut { deck, over } => {
                let slot = &mut self.decks[deck.index()];
                if slot.source.is_none() {
                    return;
                }
                let frames = duration_frames(over);
                if slot.playing && !slot.paused && frames > 0 {
                    slot.fade = Some((frames, frames));
                } else {
                    self.end(deck);
                }
            }
            CommandKind::SetGain { strip, db } => {
                self.gains[strip.index()].set(db_to_gain(db), GAIN_RAMP);
            }
            CommandKind::FireCart { slot } => {
                if !matches!(self.carts.get(slot), Some(Some(_))) {
                    return;
                }
                let free = self.voices.iter().position(|v| !v.active);
                let index = free.unwrap_or_else(|| {
                    let oldest = self.voices.iter().enumerate().max_by_key(|(_, v)| v.pos);
                    oldest.map_or(0, |(i, _)| i)
                });
                if let Some(v) = self.voices.get_mut(index) {
                    *v = Voice {
                        slot,
                        pos: 0,
                        active: true,
                    };
                }
            }
            CommandKind::StopCarts => {
                for v in &mut self.voices {
                    v.active = false;
                }
            }
            CommandKind::Duck {
                db,
                attack,
                release,
            } => {
                self.duck.release = duration_frames(release);
                self.duck.to(db.min(0.0), duration_frames(attack));
            }
            CommandKind::Unduck => {
                let release = self.duck.release;
                self.duck.to(0.0, release);
            }
        }
    }

    /// Unloads a deck at once.
    fn unload(&mut self, deck: DeckId) {
        let slot = &mut self.decks[deck.index()];
        let old = slot.source.take();
        *slot = DeckSlot::default();
        if let Some(src) = old {
            src.shared.set_state(DeckState::Empty);
            src.shared.underrun.store(false, Ordering::Release);
            self.retire(Retired::Deck(src));
        }
    }

    /// Unloads a deck and tells the handle that its item ended.
    fn end(&mut self, deck: DeckId) {
        self.unload(deck);
        self.emit(AudioEvent::DeckEnded(deck));
    }

    fn retire(&mut self, item: Retired) {
        // The ring has room for every item that can exist, so this push cannot fail.
        if let Err(PushError::Full(item)) = self.retired.push(item) {
            drop(item);
        }
    }

    fn emit(&mut self, event: AudioEvent) {
        // A full event ring drops the event. The render thread never waits.
        let _ = self.events.push(event);
    }

    fn deck_step(&mut self, index: usize) -> DeckStep {
        let Some(slot) = self.decks.get_mut(index) else {
            return DeckStep::Silent;
        };
        let Some(src) = slot.source.as_mut() else {
            return DeckStep::Silent;
        };
        if !slot.playing || slot.paused {
            return DeckStep::Silent;
        }
        if src.consumer.slots() < CHANNELS {
            if src.shared.eof.load(Ordering::Acquire) && src.consumer.slots() < CHANNELS {
                return DeckStep::Ended;
            }
            if slot.underrun {
                return DeckStep::Silent;
            }
            slot.underrun = true;
            src.shared.underrun.store(true, Ordering::Release);
            return DeckStep::Underrun;
        }
        let l = src.consumer.pop().unwrap_or(0.0);
        let r = src.consumer.pop().unwrap_or(0.0);
        if slot.underrun {
            slot.underrun = false;
            src.shared.underrun.store(false, Ordering::Release);
        }
        let mut gain = 1.0;
        if src.fade_in > slot.played {
            gain = slot.played as f32 / src.fade_in as f32;
        }
        if let Some((remaining, total)) = slot.fade.as_mut() {
            if *remaining == 0 {
                return DeckStep::Ended;
            }
            gain *= *remaining as f32 / *total as f32;
            *remaining -= 1;
        }
        slot.played += 1;
        DeckStep::Frame([l * gain, r * gain])
    }

    /// Mixes one frame. It returns the Program frame and the PFL frame.
    fn frame(&mut self, pfl_mask: u8) -> ([f32; 2], [f32; 2]) {
        let mut bus = [0.0f32; 2];
        let mut cue = [0.0f32; 2];
        for deck in DeckId::ALL {
            let i = deck.index();
            let pre = match self.deck_step(i) {
                DeckStep::Frame(f) => f,
                DeckStep::Silent => [0.0; 2],
                DeckStep::Ended => {
                    self.end(deck);
                    [0.0; 2]
                }
                DeckStep::Underrun => {
                    self.emit(AudioEvent::Underrun(deck));
                    [0.0; 2]
                }
            };
            self.strip(i, pre, pfl_mask, &mut bus, &mut cue);
        }

        let live = if self.live.slots() >= CHANNELS {
            [
                self.live.pop().unwrap_or(0.0),
                self.live.pop().unwrap_or(0.0),
            ]
        } else {
            [0.0; 2]
        };
        self.strip(Strip::Live.index(), live, pfl_mask, &mut bus, &mut cue);

        let mut carts = [0.0f32; 2];
        for v in self.voices.iter_mut().filter(|v| v.active) {
            let clip = self.carts.get(v.slot).and_then(Option::as_ref);
            let at = v.pos * CHANNELS;
            match clip.and_then(|c| c.samples().get(at..at + CHANNELS)) {
                Some(f) => {
                    carts[0] += f[0];
                    carts[1] += f[1];
                    v.pos += 1;
                }
                None => v.active = false,
            }
        }
        let mut cart_bus = [0.0f32; 2];
        self.strip(
            Strip::Cart.index(),
            carts,
            pfl_mask,
            &mut cart_bus,
            &mut cue,
        );

        let duck = self.duck.next();
        let out_gain = self.gains[Strip::Out.index()].next();
        let y = [
            limit((bus[0] * duck + cart_bus[0]) * out_gain),
            limit((bus[1] * duck + cart_bus[1]) * out_gain),
        ];
        self.meters.add(Strip::Out.index(), y);
        match self.silence.add(self.sample, y) {
            Some(SilenceChange::Started { since_sample }) => {
                self.emit(AudioEvent::Silence { since_sample });
            }
            Some(SilenceChange::Ended { at_sample }) => {
                self.emit(AudioEvent::SilenceEnded { at_sample });
            }
            None => {}
        }
        self.sample += 1;
        (y, cue)
    }

    /// Applies the fader of a strip, meters it, and adds it to a bus and to PFL.
    fn strip(&mut self, i: usize, pre: [f32; 2], mask: u8, bus: &mut [f32; 2], cue: &mut [f32; 2]) {
        let g = self.gains.get_mut(i).map_or(1.0, Ramp::next);
        let post = [pre[0] * g, pre[1] * g];
        self.meters.add(i, post);
        bus[0] += post[0];
        bus[1] += post[1];
        if mask & (1 << i) != 0 {
            cue[0] += pre[0];
            cue[1] += pre[1];
        }
    }
}

/// Renders `frames` frames of Program as fast as the decoders allow, for tests and the
/// testkit. It waits for the deck feeders between blocks, so decks do not underrun.
pub fn render_offline(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
    const BLOCK: usize = 256;
    let mut out = vec![0.0; frames * CHANNELS];
    for chunk in out.chunks_mut(BLOCK * CHANNELS) {
        mixer.wait_for_decks(chunk.len(), Duration::from_secs(2));
        mixer.render(chunk, &mut []);
    }
    out
}

/// The control side of a [`Mixer`], for the engine and UI threads. Each method with
/// `&mut self` also frees the heap items that the mixer gave back.
pub struct MixerHandle {
    commands: Producer<Command>,
    control: Producer<Control>,
    retired: Consumer<Retired>,
    events: Consumer<AudioEvent>,
    meters: triple_buffer::Output<MeterSnapshot>,
    decks: [Option<Arc<DeckShared>>; 4],
    feeders: [Option<DeckFeeder>; 4],
    shared: Arc<Shared>,
    deck_buffer: usize,
    cart_slots: usize,
}

impl MixerHandle {
    /// Sends a command. The mixer applies it at `at_sample`.
    pub fn send(&mut self, command: Command) -> Result<(), AudioError> {
        self.collect();
        self.commands
            .push(command)
            .map_err(|_| AudioError::QueueFull)
    }

    /// Opens a file and loads it onto an empty or stopped deck. The file opens on this
    /// thread, so a missing or broken file fails here. A decoder thread then fills the ring
    /// of the deck, and the deck goes from `Loading` to `Ready`.
    pub fn load(
        &mut self,
        deck: DeckId,
        path: impl AsRef<Path>,
        options: LoadOptions,
    ) -> Result<(), AudioError> {
        self.collect();
        if matches!(
            self.deck_state(deck),
            DeckState::Playing | DeckState::Paused
        ) {
            return Err(AudioError::DeckBusy(deck));
        }
        let decoder = Decoder::open_at(path, options.cue_in)?;
        let (producer, consumer) = RingBuffer::new(self.deck_buffer);
        let shared = Arc::new(DeckShared::new());
        let feeder = DeckFeeder::spawn(decoder, producer, Arc::clone(&shared))?;
        let source = DeckSource {
            consumer,
            shared: Arc::clone(&shared),
            fade_in: duration_frames(options.fade_in),
        };
        self.control
            .push(Control::Load { deck, source })
            .map_err(|_| AudioError::QueueFull)?;
        self.decks[deck.index()] = Some(shared);
        self.feeders[deck.index()] = Some(feeder);
        Ok(())
    }

    /// Puts a clip into a cartwall slot, or clears the slot with `None`.
    pub fn set_cart(&mut self, slot: usize, clip: Option<Clip>) -> Result<(), AudioError> {
        if slot >= self.cart_slots {
            return Err(AudioError::NoSuchCartSlot(slot));
        }
        self.collect();
        self.control
            .push(Control::SetCart { slot, clip })
            .map_err(|_| AudioError::QueueFull)
    }

    /// Routes a strip to PFL, or takes it off. Program out has no PFL.
    pub fn set_pfl(&self, strip: Strip, on: bool) {
        if strip == Strip::Out {
            return;
        }
        let bit = 1u8 << strip.index();
        if on {
            self.shared.pfl.fetch_or(bit, Ordering::Relaxed);
        } else {
            self.shared.pfl.fetch_and(!bit, Ordering::Relaxed);
        }
    }

    /// Returns the state of a deck.
    pub fn deck_state(&self, deck: DeckId) -> DeckState {
        self.decks[deck.index()]
            .as_ref()
            .map_or(DeckState::Empty, |s| s.state())
    }

    /// Returns `true` while the ring of a playing deck is empty before the end of its file.
    pub fn underrun(&self, deck: DeckId) -> bool {
        self.decks[deck.index()]
            .as_ref()
            .is_some_and(|s| s.underrun.load(Ordering::Acquire))
    }

    /// Returns `true` if the decoder of the deck stopped on an error before the file end.
    pub fn decode_failed(&self, deck: DeckId) -> bool {
        self.decks[deck.index()]
            .as_ref()
            .is_some_and(|s| s.failed.load(Ordering::Acquire))
    }

    /// Returns the absolute count of frames that the mixer rendered.
    pub fn now(&self) -> u64 {
        self.shared.now.load(Ordering::Acquire)
    }

    /// Returns the meters from the last render block.
    pub fn meters(&mut self) -> MeterSnapshot {
        *self.meters.read()
    }

    /// Takes the events that the mixer sent since the last call.
    pub fn events(&mut self) -> impl Iterator<Item = AudioEvent> + '_ {
        self.collect();
        std::iter::from_fn(move || self.events.pop().ok())
    }

    /// Frees the heap items that the mixer gave back.
    fn collect(&mut self) {
        while let Ok(item) = self.retired.pop() {
            match item {
                Retired::Deck(source) => drop(source),
                Retired::Clip(clip) => drop(clip),
            }
        }
        for feeder in &mut self.feeders {
            if feeder.as_ref().is_some_and(DeckFeeder::is_finished) {
                *feeder = None;
            }
        }
    }
}

/// The producer side of the live input ring, for the audio-device crate.
pub struct LiveInput {
    producer: Producer<f32>,
}

impl LiveInput {
    /// Pushes stereo interleaved frames at 48 kHz. It returns the number of frames that fit.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        let room = self.producer.slots() / CHANNELS * CHANNELS;
        let n = (interleaved.len() / CHANNELS * CHANNELS).min(room);
        match interleaved.get(..n) {
            Some(frames) if self.producer.push_entire_slice(frames).is_ok() => n / CHANNELS,
            _ => 0,
        }
    }
}

/// A [`Mixer`] with a scratch buffer, for an output device with any channel count.
pub struct DeviceRenderer {
    mixer: Mixer,
    program: Vec<f32>,
    max_frames: usize,
}

impl DeviceRenderer {
    /// Wraps a mixer. The constructor allocates scratch for `max_frames` frames per pass.
    pub fn new(mixer: Mixer, max_frames: usize) -> Self {
        let max_frames = max_frames.max(1);
        Self {
            mixer,
            program: vec![0.0; max_frames * CHANNELS],
            max_frames,
        }
    }

    /// Renders Program into `out`, interleaved with `channels` channels. Mono gets the mean
    /// of left and right, and channels after the second get silence. It is real-time safe.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        for chunk in out.chunks_mut(self.max_frames * channels) {
            let frames = chunk.len() / channels;
            let Some(program) = self.program.get_mut(..frames * CHANNELS) else {
                return;
            };
            self.mixer.render(program, &mut []);
            let mut dst = chunk.chunks_exact_mut(channels);
            for (d, s) in (&mut dst).zip(program.as_chunks::<CHANNELS>().0) {
                if channels == 1 {
                    d[0] = (s[0] + s[1]) * 0.5;
                } else {
                    d.fill(0.0);
                    d[0] = s[0];
                    d[1] = s[1];
                }
            }
            dst.into_remainder().fill(0.0);
        }
    }

    /// Returns the mixer.
    pub fn mixer_mut(&mut self) -> &mut Mixer {
        &mut self.mixer
    }

    /// Takes the mixer back.
    pub fn into_inner(self) -> Mixer {
        self.mixer
    }
}
