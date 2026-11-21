use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Count of frames an output has rendered: the master audio clock.
///
/// Cloning gives another handle to the same counter. The engine pins it to wall-clock time.
#[derive(Debug, Clone, Default)]
pub struct SampleClock(Arc<AtomicU64>);

impl SampleClock {
    /// A clock at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames rendered so far.
    pub fn frames(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// Called by the output after each rendered block (lock-free, allocation-free).
    pub(crate) fn advance(&self, frames: usize) {
        self.0.fetch_add(frames as u64, Ordering::Release);
    }
}
