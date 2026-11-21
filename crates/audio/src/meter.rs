//! Level meters, and the snapshot that the UI reads.

use crate::{SAMPLE_RATE, Strip};

/// The lowest level that a meter shows, in dBFS. Silence reads as this value.
pub const METER_FLOOR_DB: f32 = -120.0;

/// The levels of one strip, per channel (left, right), in dBFS.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StripMeter {
    /// The sample peak, with a fall of 20 dB/s.
    pub peak_db: [f32; 2],
    /// The RMS level over about 300 ms.
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

/// The ballistics of one channel. It runs on the render thread.
#[derive(Copy, Clone, Debug, Default)]
struct Channel {
    peak: f32,
    mean_square: f32,
    hold: f32,
    hold_age: u64,
}

/// The meters of all strips. It runs on the render thread and does not allocate.
#[derive(Debug)]
pub(crate) struct MeterBank {
    channels: [[Channel; 2]; Strip::COUNT],
    peak_fall: f32,
    rms_coef: f32,
    hold_frames: u64,
}

impl MeterBank {
    pub(crate) fn new(hold: std::time::Duration) -> Self {
        let rate = SAMPLE_RATE as f32;
        Self {
            channels: [[Channel::default(); 2]; Strip::COUNT],
            // 20 dB per second.
            peak_fall: 10f32.powf(-1.0 / rate),
            rms_coef: 1.0 - (-1.0 / (0.3 * rate)).exp(),
            hold_frames: (hold.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64,
        }
    }

    pub(crate) fn add(&mut self, strip: usize, frame: [f32; 2]) {
        let Some(chans) = self.channels.get_mut(strip) else {
            return;
        };
        for (ch, x) in chans.iter_mut().zip(frame) {
            let a = x.abs();
            ch.peak = a.max(ch.peak * self.peak_fall);
            ch.mean_square += self.rms_coef * (x * x - ch.mean_square);
            if a >= ch.hold {
                ch.hold = a;
                ch.hold_age = 0;
            } else {
                ch.hold_age += 1;
                if ch.hold_age > self.hold_frames {
                    ch.hold = ch.peak;
                    ch.hold_age = 0;
                }
            }
        }
    }

    pub(crate) fn snapshot(&self, at_sample: u64) -> MeterSnapshot {
        let mut snap = MeterSnapshot {
            at_sample,
            ..MeterSnapshot::default()
        };
        for (meter, chans) in snap.strips.iter_mut().zip(&self.channels) {
            for (i, ch) in chans.iter().enumerate() {
                meter.peak_db[i] = to_db(ch.peak);
                meter.rms_db[i] = to_db(ch.mean_square.max(0.0).sqrt());
                meter.peak_hold_db[i] = to_db(ch.hold);
            }
        }
        snap
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
