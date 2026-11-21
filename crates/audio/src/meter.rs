//! Level meters, and the snapshot that the UI reads.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

use crate::{SAMPLE_RATE, Strip};

/// The lowest level that a meter shows, in dBFS. Silence reads as this value.
pub const METER_FLOOR_DB: f32 = -120.0;

/// The time constant of the RMS meter, in seconds.
const RMS_WINDOW_S: f32 = 0.05;

/// Ballistic values below this level become 0, so they never become denormals.
const FLUSH: f32 = 1e-20;

/// The levels of one strip, per channel (left, right), in dBFS.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StripMeter {
    /// The true sample peak of the last render block, with no fall: a loud block shows at once
    /// and a quiet one right after it shows the lower level.
    pub peak_db: [f32; 2],
    /// The RMS level over about 50 ms.
    pub rms_db: [f32; 2],
    /// The highest peak in the hold time.
    pub peak_hold_db: [f32; 2],
}

impl Default for StripMeter {
    fn default() -> Self {
        Self {
            peak_db: [METER_FLOOR_DB; 2],
            rms_db: [METER_FLOOR_DB; 2],
            peak_hold_db: [METER_FLOOR_DB; 2],
        }
    }
}

/// The levels of all strips after a render block.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct MeterSnapshot {
    /// The absolute frame count at the end of the block.
    pub at_sample: u64,
    /// The strip levels, in [`Strip::ALL`] order.
    pub strips: [StripMeter; Strip::COUNT],
}

impl MeterSnapshot {
    /// Returns the levels of one strip.
    pub fn strip(&self, strip: Strip) -> &StripMeter {
        &self.strips[strip.index()]
    }
}

fn to_db(lin: f32) -> f32 {
    if lin <= 1e-6 {
        METER_FLOOR_DB
    } else {
        (20.0 * lin.log10()).max(METER_FLOOR_DB)
    }
}

fn flush(x: f32) -> f32 {
    if x.abs() < FLUSH { 0.0 } else { x }
}

/// The ballistics of one channel. It runs on the render thread.
#[derive(Copy, Clone, Debug, Default)]
struct Channel {
    /// The largest absolute sample since the last snapshot.
    block_peak: f32,
    mean_square: f32,
    hold: f32,
    hold_age: u64,
}

/// The meters of all strips. It runs on the render thread and does not allocate.
#[derive(Debug)]
pub(crate) struct MeterBank {
    channels: [[Channel; 2]; Strip::COUNT],
    rms_coef: f32,
    hold_frames: u64,
}

impl MeterBank {
    pub(crate) fn new(hold: std::time::Duration) -> Self {
        let rate = SAMPLE_RATE as f32;
        Self {
            channels: [[Channel::default(); 2]; Strip::COUNT],
            rms_coef: 1.0 - (-1.0 / (RMS_WINDOW_S * rate)).exp(),
            hold_frames: (hold.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64,
        }
    }

    pub(crate) fn add(&mut self, strip: usize, frame: [f32; 2]) {
        let Some(chans) = self.channels.get_mut(strip) else {
            return;
        };
        for (ch, x) in chans.iter_mut().zip(frame) {
            let a = x.abs();
            ch.block_peak = ch.block_peak.max(a);
            ch.mean_square = flush(ch.mean_square + self.rms_coef * (x * x - ch.mean_square));
            if a >= ch.hold {
                ch.hold = a;
                ch.hold_age = 0;
            } else {
                ch.hold_age += 1;
                if ch.hold_age > self.hold_frames {
                    ch.hold = a;
                    ch.hold_age = 0;
                }
            }
            ch.hold = flush(ch.hold);
        }
    }

    /// The levels now. The block peaks start again from zero.
    pub(crate) fn snapshot(&mut self, at_sample: u64) -> MeterSnapshot {
        let mut snap = MeterSnapshot {
            at_sample,
            ..MeterSnapshot::default()
        };
        for (meter, chans) in snap.strips.iter_mut().zip(&mut self.channels) {
            for (i, ch) in chans.iter_mut().enumerate() {
                meter.peak_db[i] = to_db(ch.block_peak);
                ch.block_peak = 0.0;
                meter.rms_db[i] = to_db(ch.mean_square.max(0.0).sqrt());
                meter.peak_hold_db[i] = to_db(ch.hold);
            }
        }
        snap
    }
}

/// The number of f32 values in a snapshot.
const FIELDS: usize = Strip::COUNT * 6;

/// A sequence lock over atomics, for one writer and any number of readers. The writer
/// never waits. A reader tries again when it sees a write in progress.
pub(crate) struct MeterBoard {
    seq: AtomicU64,
    at_sample: AtomicU64,
    values: [AtomicU32; FIELDS],
}

impl MeterBoard {
    pub(crate) fn new() -> Self {
        let board = Self {
            seq: AtomicU64::new(0),
            at_sample: AtomicU64::new(0),
            values: std::array::from_fn(|_| AtomicU32::new(METER_FLOOR_DB.to_bits())),
        };
        board.publish(&MeterSnapshot::default());
        board
    }

    /// Writes a snapshot. Only the render thread calls this.
    pub(crate) fn publish(&self, snap: &MeterSnapshot) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.at_sample.store(snap.at_sample, Ordering::Relaxed);
        let fields = snap
            .strips
            .iter()
            .flat_map(|m| m.peak_db.into_iter().chain(m.rms_db).chain(m.peak_hold_db));
        for (slot, v) in self.values.iter().zip(fields) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    fn try_read(&self) -> Option<MeterSnapshot> {
        let before = self.seq.load(Ordering::Acquire);
        if before % 2 == 1 {
            return None;
        }
        let mut snap = MeterSnapshot {
            at_sample: self.at_sample.load(Ordering::Relaxed),
            ..MeterSnapshot::default()
        };
        let mut values = self
            .values
            .iter()
            .map(|v| f32::from_bits(v.load(Ordering::Relaxed)));
        for m in &mut snap.strips {
            for field in [&mut m.peak_db, &mut m.rms_db, &mut m.peak_hold_db] {
                for v in field.iter_mut() {
                    *v = values.next().unwrap_or(METER_FLOOR_DB);
                }
            }
        }
        fence(Ordering::Acquire);
        (self.seq.load(Ordering::Relaxed) == before).then_some(snap)
    }
}

/// A reader of the mixer meters. Clone it for each thread that shows meters, for
/// example the UI thread. A read never blocks the render thread.
#[derive(Clone)]
pub struct MeterReader {
    board: Arc<MeterBoard>,
    last: MeterSnapshot,
}

impl MeterReader {
    pub(crate) fn new(board: Arc<MeterBoard>) -> Self {
        Self {
            board,
            last: MeterSnapshot::default(),
        }
    }

    /// Returns the meters from the last render block. If the render thread writes all the
    /// time, it returns the last complete snapshot that this reader saw.
    pub fn read(&mut self) -> MeterSnapshot {
        for _ in 0..64 {
            if let Some(snap) = self.board.try_read() {
                self.last = snap;
                return snap;
            }
            std::hint::spin_loop();
        }
        self.last
    }
}

/// The silence detector settings for Program.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SilenceDetector {
    /// Program is silent while its peak stays below this level.
    pub threshold_db: f32,
    /// The detector raises `Silence` after this much silence.
    pub after: std::time::Duration,
}

impl Default for SilenceDetector {
    fn default() -> Self {
        Self {
            threshold_db: -50.0,
            after: std::time::Duration::from_secs(15),
        }
    }
}

/// The result of one detector window.
pub(crate) enum SilenceChange {
    Started { since_sample: u64 },
    Ended { at_sample: u64 },
}

/// The silence detector state. It looks at Program in windows of 10 ms.
pub(crate) struct SilenceState {
    threshold: f32,
    after: u64,
    window_peak: f32,
    window_len: u64,
    window_start: u64,
    run_start: Option<u64>,
    raised: bool,
}

impl SilenceState {
    const WINDOW: u64 = SAMPLE_RATE as u64 / 100;

    pub(crate) fn new(config: SilenceDetector) -> Self {
        Self {
            threshold: 10f32.powf(config.threshold_db / 20.0),
            after: (config.after.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64,
            window_peak: 0.0,
            window_len: 0,
            window_start: 0,
            run_start: None,
            raised: false,
        }
    }

    /// Adds one Program frame at absolute frame `sample`.
    pub(crate) fn add(&mut self, sample: u64, frame: [f32; 2]) -> Option<SilenceChange> {
        if self.window_len == 0 {
            self.window_start = sample;
        }
        self.window_peak = self.window_peak.max(frame[0].abs()).max(frame[1].abs());
        self.window_len += 1;
        if self.window_len < Self::WINDOW {
            return None;
        }
        let end = self.window_start + self.window_len;
        let silent = self.window_peak < self.threshold;
        let start = self.window_start;
        self.window_peak = 0.0;
        self.window_len = 0;
        if silent {
            let run_start = *self.run_start.get_or_insert(start);
            if !self.raised && end - run_start >= self.after {
                self.raised = true;
                return Some(SilenceChange::Started {
                    since_sample: run_start,
                });
            }
            None
        } else {
            self.run_start = None;
            if self.raised {
                self.raised = false;
                return Some(SilenceChange::Ended { at_sample: start });
            }
            None
        }
    }
}
