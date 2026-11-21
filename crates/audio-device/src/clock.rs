use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Count of frames an output has rendered: the master audio clock.
///
/// Cloning gives another handle to the same counter. The engine pins it to wall-clock time.
#[derive(Debug, Clone, Default)]
pub struct SampleClock {
    frames: Arc<AtomicU64>,
    latency_ns: Arc<AtomicU64>,
}

impl SampleClock {
    /// A clock at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames rendered so far.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Acquire)
    }

    /// The time from a render to the moment its first frame sounds, as the device last
    /// reported it. It is zero for the null output.
    pub fn latency(&self) -> Duration {
        Duration::from_nanos(self.latency_ns.load(Ordering::Relaxed))
    }

    /// Called by the output after each rendered block (lock-free, allocation-free).
    pub(crate) fn advance(&self, frames: usize) {
        self.frames.fetch_add(frames as u64, Ordering::Release);
    }

    /// Called by the output callback (lock-free, allocation-free).
    pub(crate) fn set_latency(&self, latency: Duration) {
        let ns = u64::try_from(latency.as_nanos()).unwrap_or(u64::MAX);
        self.latency_ns.store(ns, Ordering::Relaxed);
    }
}
