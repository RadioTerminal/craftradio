//! Wall-clock time and its mapping onto the mixer's sample clock (spec §6.3).

use jiff::{SignedDuration, Timestamp};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The wall clock that owns `fixed_at` times. Production uses [`SystemClock`]; tests inject
/// a [`ManualClock`] and move it themselves.
pub trait Clock: Send + Sync + 'static {
    /// The current wall-clock time.
    fn now(&self) -> Timestamp;
}

/// The system clock (kept accurate by NTP).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock that only moves when told to. Clones share the same time.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<Mutex<Timestamp>>);

impl ManualClock {
    /// A clock stopped at `at`.
    pub fn new(at: Timestamp) -> Self {
        Self(Arc::new(Mutex::new(at)))
    }

    /// Moves the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        if let Ok(mut t) = self.0.lock() {
            *t = t.checked_add(by).unwrap_or(*t);
        }
    }

    /// Sets the clock, forwards or backwards (an NTP step, say).
    pub fn set(&self, at: Timestamp) {
        if let Ok(mut t) = self.0.lock() {
            *t = at;
        }
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        self.0.lock().map_or(Timestamp::UNIX_EPOCH, |t| *t)
    }
}

/// How far back the fit looks.
const WINDOW_NS: i128 = 10_000_000_000;
/// A point this far off the fit is a clock step, not drift: the map re-anchors.
const JUMP_SAMPLES: f64 = 0.25 * RATE;
/// The fitted rate stays within this many parts per million of nominal.
const MAX_PPM: f64 = 500.0;
const RATE: f64 = radio_audio::SAMPLE_RATE as f64;
const NOMINAL: f64 = RATE / 1e9;

/// The measured relation between wall-clock time and mixer frames. It is a least-squares
/// line over the last 10 s of observations. It starts again when the two clocks step apart.
#[derive(Debug, Default)]
pub(crate) struct ClockMap {
    points: VecDeque<(i128, u64)>,
    /// Frames per nanosecond, and the line's centre (wall ns, frame).
    slope: f64,
    centre: (f64, f64),
    /// The time from rendering a frame to hearing it, from the output device.
    pub(crate) latency: Duration,
}

impl ClockMap {
    /// Adds an observation: at wall time `wall` the mixer was at frame `sample`. Returns the
    /// size of the step when the map had to re-anchor.
    pub(crate) fn observe(&mut self, wall: Timestamp, sample: u64) -> Option<SignedDuration> {
        let x = wall.as_nanosecond();
        let mut jump = None;
        if !self.points.is_empty() {
            let off = sample as f64 - self.predict(x);
            if off.abs() > JUMP_SAMPLES {
                self.points.clear();
                jump = Some(SignedDuration::from_secs_f64(off / RATE));
            }
        }
        if self.points.back().is_some_and(|p| p.0 >= x) {
            // Wall time stood still or went back a little; keep the newer point only.
            self.points.pop_back();
        }
        self.points.push_back((x, sample));
        while self.points.front().is_some_and(|p| x - p.0 > WINDOW_NS) {
            self.points.pop_front();
        }
        self.fit();
        jump
    }

    fn fit(&mut self) {
        let n = self.points.len() as f64;
        let Some(&(x0, y0)) = self.points.front() else {
            return;
        };
        let rel = |p: &(i128, u64)| ((p.0 - x0) as f64, p.1 as f64 - y0 as f64);
        let (sx, sy) = self
            .points
            .iter()
            .map(rel)
            .fold((0.0, 0.0), |a, (x, y)| (a.0 + x, a.1 + y));
        let (mx, my) = (sx / n, sy / n);
        let (sxy, sxx) = self.points.iter().map(rel).fold((0.0, 0.0), |a, (x, y)| {
            (a.0 + (x - mx) * (y - my), a.1 + (x - mx) * (x - mx))
        });
        let lo = NOMINAL * (1.0 - MAX_PPM / 1e6);
        let hi = NOMINAL * (1.0 + MAX_PPM / 1e6);
        self.slope = if sxx > 0.0 {
            (sxy / sxx).clamp(lo, hi)
        } else {
            NOMINAL
        };
        self.centre = (x0 as f64 + mx, y0 as f64 + my);
    }

    fn predict(&self, x: i128) -> f64 {
        let slope = if self.slope > 0.0 {
            self.slope
        } else {
            NOMINAL
        };
        self.centre.1 + (x as f64 - self.centre.0) * slope
    }

    /// The mixer frame that sounds at wall time `wall`. The mixer renders it one output
    /// latency before that time.
    pub(crate) fn sample_at(&self, wall: Timestamp) -> u64 {
        let x = wall.as_nanosecond() - self.latency.as_nanos() as i128;
        let y = self.predict(x).round();
        if y <= 0.0 { 0 } else { y as u64 }
    }

    /// The wall time at which mixer frame `sample` sounds.
    pub(crate) fn wall_at(&self, sample: u64) -> Timestamp {
        let slope = if self.slope > 0.0 {
            self.slope
        } else {
            NOMINAL
        };
        let x = self.centre.0
            + (sample as f64 - self.centre.1) / slope
            + self.latency.as_nanos() as f64;
        Timestamp::from_nanosecond(x as i128).unwrap_or(Timestamp::UNIX_EPOCH)
    }
}
