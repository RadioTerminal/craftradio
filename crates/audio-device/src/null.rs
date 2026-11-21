use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{AudioSource, DeviceError, DeviceEvent, SampleClock};

const BLOCK: Duration = Duration::from_millis(10);

/// A software "sound card": a thread paced by [`Instant`] renders 10 ms blocks of an
/// [`AudioSource`] at the requested rate and discards the result. Used when there is no
/// sound card (headless, Icecast only).
pub struct NullOutput {
    clock: SampleClock,
    sample_rate: u32,
    channels: u16,
    stop: Arc<AtomicBool>,
    events_rx: Receiver<DeviceEvent>,
    thread: Option<JoinHandle<()>>,
    source_rx: Receiver<Box<dyn AudioSource>>,
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
        let (events_tx, events_rx): (Sender<DeviceEvent>, _) = channel();
        let (source_tx, source_rx) = channel();
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
                    // After a stall, skip what is older than 200 ms instead of bursting; the
                    // skipped frames still count on the clock so it stays pinned to real time.
                    let max_behind = (rate / 5).max(block_frames);
                    if due > rendered + max_behind {
                        let skipped = due - rendered - max_behind;
                        rendered += skipped;
                        thread_clock.advance(skipped as usize);
                    }
                    if rendered + block_frames > due {
                        std::thread::sleep(BLOCK / 4);
                        continue;
                    }
                    buf.fill(0.0);
                    let ok = catch_unwind(AssertUnwindSafe(|| source.render(&mut buf, ch)));
                    if ok.is_err() {
                        let _ = events_tx.send(DeviceEvent::Lost {
                            name: "null".to_string(),
                        });
                        break;
                    }
                    rendered += block_frames;
                    thread_clock.advance(block_frames as usize);
                }
                let _ = source_tx.send(source);
            })
            .map_err(|e| DeviceError::Stream(format!("null output thread: {e}")))?;
        Ok(NullOutput {
            clock,
            sample_rate,
            channels,
            stop,
            events_rx,
            thread: Some(thread),
            source_rx,
        })
    }

    /// Stop the timer and return the source for a replacement output.
    pub fn into_source(mut self) -> Result<Box<dyn AudioSource>, DeviceError> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.source_rx
            .try_recv()
            .map_err(|_| DeviceError::SourceLost)
    }

    /// The master audio clock: frames rendered so far.
    pub fn clock(&self) -> SampleClock {
        self.clock.clone()
    }

    /// Rate being simulated.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Events from the output. `Lost { name: "null" }` means `render` panicked and the
    /// thread stopped.
    pub fn events(&self) -> &Receiver<DeviceEvent> {
        &self.events_rx
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
