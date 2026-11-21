use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{AudioSource, DeviceError, SampleClock};

const BLOCK: Duration = Duration::from_millis(10);

/// A software "sound card": a thread paced by [`Instant`] renders 10 ms blocks of an
/// [`AudioSource`] at the requested rate and discards the result. Used when there is no
/// sound card (headless, Icecast only).
pub struct NullOutput {
    clock: SampleClock,
    sample_rate: u32,
    channels: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NullOutput {
    /// Start rendering `source` at `sample_rate` with `channels` interleaved channels.
    pub fn start(
        sample_rate: u32,
        channels: u16,
        mut source: Box<dyn AudioSource>,
    ) -> Result<NullOutput, DeviceError> {
        let ch = usize::from(channels.max(1));
        let rate = u64::from(sample_rate.max(1));
        let clock = SampleClock::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_clock, thread_stop) = (clock.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("null-output".into())
            .spawn(move || {
                let start = Instant::now();
                let block_frames = (rate / 100).max(1);
                let mut buf = vec![0.0f32; block_frames as usize * ch];
                let mut rendered: u64 = 0;
                while !thread_stop.load(Ordering::Acquire) {
                    // Frames due by now, from the wall clock: no cumulative rounding drift.
                    let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                    let due = nanos.saturating_mul(rate) / 1_000_000_000;
                    if rendered + block_frames > due {
                        std::thread::sleep(BLOCK / 4);
                        continue;
                    }
                    buf.fill(0.0);
                    source.render(&mut buf, ch);
                    rendered += block_frames;
                    thread_clock.advance(block_frames as usize);
                }
            })
            .map_err(|e| DeviceError::Stream(format!("null output thread: {e}")))?;
        Ok(NullOutput {
            clock,
            sample_rate,
            channels,
            stop,
            thread: Some(thread),
        })
    }

    /// The master audio clock: frames rendered so far.
    pub fn clock(&self) -> SampleClock {
        self.clock.clone()
    }

    /// Rate being simulated.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channel count.
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

impl Drop for NullOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
