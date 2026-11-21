//! Deck state and the decoder thread that fills the ring of a deck.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use rtrb::{Consumer, Producer};

use crate::{AudioError, CHANNELS, Decoder};

/// The state of a deck.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeckState {
    /// The deck has no item.
    Empty,
    /// The decoder thread fills the ring of the deck.
    Loading,
    /// The ring is full, or holds the whole file. `Play` starts the deck.
    Ready,
    /// The deck sends audio to Program.
    Playing,
    /// The deck holds its position and sends silence.
    Paused,
}

impl DeckState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Loading,
            2 => Self::Ready,
            3 => Self::Playing,
            4 => Self::Paused,
            _ => Self::Empty,
        }
    }

    pub(crate) fn as_u8(self) -> u8 {
        match self {
            Self::Empty => 0,
            Self::Loading => 1,
            Self::Ready => 2,
            Self::Playing => 3,
            Self::Paused => 4,
        }
    }
}

/// How a deck plays a new item.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadOptions {
    /// The point in the file where playback starts.
    pub cue_in: Duration,
    /// The length of the fade-in from silence at the start. Zero means no fade.
    pub fade_in: Duration,
}

/// State that the feeder, the render thread and the handle share for one loaded item.
#[derive(Debug)]
pub(crate) struct DeckShared {
    state: AtomicU8,
    /// The feeder pushed the last frame of the file, or stopped on an error.
    pub(crate) eof: AtomicBool,
    /// The decoder failed in the middle of the file.
    pub(crate) failed: AtomicBool,
    /// The ring of the playing deck is empty before the end of the file.
    pub(crate) underrun: AtomicBool,
}

impl DeckShared {
    pub(crate) fn new() -> Self {
        Self {
            state: AtomicU8::new(DeckState::Loading.as_u8()),
            eof: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            underrun: AtomicBool::new(false),
        }
    }

    pub(crate) fn state(&self) -> DeckState {
        DeckState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub(crate) fn set_state(&self, state: DeckState) {
        self.state.store(state.as_u8(), Ordering::Release);
    }

    /// Changes `Loading` to `Ready`. It keeps any state that the mixer set.
    fn mark_ready(&self) {
        let _ = self.state.compare_exchange(
            DeckState::Loading.as_u8(),
            DeckState::Ready.as_u8(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// The render side of a loaded item: the ring consumer and its shared state.
pub(crate) struct DeckSource {
    pub(crate) consumer: Consumer<f32>,
    pub(crate) shared: Arc<DeckShared>,
    pub(crate) fade_in: u64,
}

/// The decoder thread of one loaded item. It fills the ring of the deck from the file. It
/// stops at the end of the file, or when the mixer lets go of the ring.
pub struct DeckFeeder {
    thread: Option<JoinHandle<()>>,
}

impl DeckFeeder {
    pub(crate) fn spawn(
        decoder: Decoder,
        producer: Producer<f32>,
        shared: Arc<DeckShared>,
    ) -> Result<Self, AudioError> {
        let thread = std::thread::Builder::new()
            .name("radio-deck-feeder".into())
            .spawn(move || feed(decoder, producer, &shared))
            .map_err(AudioError::Thread)?;
        Ok(Self {
            thread: Some(thread),
        })
    }

    /// Returns `true` when the thread has stopped.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

fn feed(mut decoder: Decoder, mut producer: Producer<f32>, shared: &DeckShared) {
    let mut buf: Vec<f32> = Vec::with_capacity(16 * 1024);
    let mut pos = 0;
    let mut more = true;
    loop {
        if producer.is_abandoned() {
            return;
        }
        if pos >= buf.len() {
            buf.clear();
            pos = 0;
            if !more {
                shared.eof.store(true, Ordering::Release);
                shared.mark_ready();
                return;
            }
            match decoder.read(&mut buf) {
                Ok(m) => more = m,
                Err(_) => {
                    shared.failed.store(true, Ordering::Release);
                    buf.clear();
                    more = false;
                }
            }
            continue;
        }
        let free = producer.slots() / CHANNELS * CHANNELS;
        let n = free.min(buf.len() - pos);
        if n == 0 {
            shared.mark_ready();
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        if let Some(chunk) = buf.get(pos..pos + n)
            && producer.push_entire_slice(chunk).is_ok()
        {
            pos += n;
        }
    }
}
