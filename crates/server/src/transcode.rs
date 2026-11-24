//! Re-encoding a station's pushed stream into its outputs, while someone listens.
//!
//! One thread per live source decodes the pushed mount through a tap and encodes each
//! output's mount from the same blocks. While no output has a listener it holds no tap and
//! no decoder, and only looks again every [`IDLE`].

use crate::config::Output;
use crate::decode::{BLOCK, DecodeError, Decoded, SourceDecoder};
use crate::encode::OutputEncoder;
use crate::mount::{ListenerCap, Mount};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// How often an idle transcoder looks for a listener, a stop or the source's end.
const IDLE: Duration = Duration::from_millis(50);

/// Why a live station's outputs are off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The source is not 48 kHz stereo.
    Unsupported { rate: u32, channels: usize },
    /// The source's bytes are not a stream the decoders read.
    Unreadable,
}

/// A station's output mounts, and what its transcoder reports about them.
#[derive(Debug)]
pub struct Outputs {
    outputs: Vec<(Output, Arc<Mount>)>,
    /// Per output, in config order: whether it failed for the current source.
    failed: Vec<AtomicBool>,
    refusal: Mutex<Option<Refusal>>,
    decoded: AtomicU64,
}

impl Outputs {
    /// A station's output mounts, sharing the station's listener limit.
    pub fn new(configs: &[Output], cap: &Arc<ListenerCap>) -> Arc<Outputs> {
        Arc::new(Outputs {
            outputs: configs
                .iter()
                .map(|output| (output.clone(), Mount::new(Arc::clone(cap))))
                .collect(),
            failed: configs.iter().map(|_| AtomicBool::new(false)).collect(),
            refusal: Mutex::new(None),
            decoded: AtomicU64::new(0),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty()
    }

    /// The mount of the output named `name` in the config.
    pub fn mount(&self, name: &str) -> Option<&Arc<Mount>> {
        self.outputs
            .iter()
            .find(|(output, _)| output.mount == name)
            .map(|(_, mount)| mount)
    }

    /// In config order.
    pub fn each(&self) -> impl Iterator<Item = (&Output, &Arc<Mount>)> {
        self.outputs.iter().map(|(output, mount)| (output, mount))
    }

    /// Set while the live source can't be transcoded; cleared when it ends.
    pub fn refusal(&self) -> Option<Refusal> {
        *self.refusal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the output named `name` failed for the current source: its mount could not
    /// be begun, its encoder could not be made or stopped working, or the transcoder could
    /// not run. It stays off until the source ends; the others go on. Cleared by
    /// [`Transcoder::finish`].
    pub fn failed(&self, name: &str) -> bool {
        self.outputs
            .iter()
            .zip(&self.failed)
            .any(|((output, _), failed)| output.mount == name && failed.load(Ordering::Acquire))
    }

    fn fail(&self, index: usize) {
        if let Some(failed) = self.failed.get(index) {
            failed.store(true, Ordering::Release);
        }
    }

    fn fail_all(&self) {
        for failed in &self.failed {
            failed.store(true, Ordering::Release);
        }
    }

    /// Blocks decoded since the server started; for tests of "idle costs nothing".
    pub fn decoded_blocks(&self) -> u64 {
        self.decoded.load(Ordering::Relaxed)
    }

    fn set_refusal(&self, refusal: Option<Refusal>) {
        *self.refusal.lock().unwrap_or_else(PoisonError::into_inner) = refusal;
    }
}

/// A station's transcoder thread, for one live source.
#[derive(Debug)]
pub struct Transcoder {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    outputs: Arc<Outputs>,
}

/// Starts the station's transcoder thread. Call right after the pushed mount's `begin`
/// succeeded.
pub fn spawn(pushed: Arc<Mount>, outputs: Arc<Outputs>) -> Transcoder {
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let stop = Arc::clone(&stop);
        let outputs = Arc::clone(&outputs);
        std::thread::Builder::new()
            .name("server-transcoder".into())
            .spawn(move || run(&pushed, &outputs, &stop))
            .ok()
    };
    // A thread that could not start leaves every output off, and says so.
    if thread.is_none() {
        outputs.fail_all();
    }
    Transcoder {
        stop,
        thread,
        outputs,
    }
}

impl Transcoder {
    /// Stops and joins the thread; every output mount has ended when this returns.
    /// Call before the pushed mount's guard is dropped, so a new source never finds
    /// the old outputs still live.
    pub fn finish(mut self) {
        self.end();
    }

    fn end(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // A thread that panicked has dropped its encoders, so its outputs are over too.
            let _ = thread.join();
        }
        self.outputs.set_refusal(None);
        for failed in &self.outputs.failed {
            failed.store(false, Ordering::Release);
        }
    }
}

impl Drop for Transcoder {
    fn drop(&mut self) {
        self.end();
    }
}

/// Why decoding the pushed stream stopped.
enum Halt {
    /// The source ended, or `stop` was set.
    Ended,
    Refused(Refusal),
}

impl From<DecodeError> for Halt {
    fn from(error: DecodeError) -> Halt {
        match error {
            DecodeError::Unsupported { rate, channels } => {
                Halt::Refused(Refusal::Unsupported { rate, channels })
            }
            DecodeError::Unreadable(_) => Halt::Refused(Refusal::Unreadable),
            DecodeError::Ended | DecodeError::Overrun => Halt::Ended,
        }
    }
}

/// Marks every output failed if the thread panics, so a listener is told the outputs are
/// off instead of to try again in a moment. Dropped after the encoders, whose mounts have
/// ended by then.
struct FailOnPanic<'a>(&'a Outputs);

impl Drop for FailOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.fail_all();
        }
    }
}

/// The thread: checks the source, begins the outputs and feeds them while they are heard.
fn run(pushed: &Arc<Mount>, outputs: &Outputs, stop: &Arc<AtomicBool>) {
    if outputs.is_empty() {
        return;
    }
    let _panic = FailOnPanic(outputs);
    // A source that can't be decoded never makes an output live.
    match open_source(pushed, stop) {
        Ok(_) => {}
        Err(Halt::Ended) => return,
        Err(Halt::Refused(refusal)) => return refuse(pushed, outputs, stop, refusal),
    }
    let name = pushed.stats().name;
    let mut encoders: Vec<Option<OutputEncoder>> = outputs
        .each()
        .enumerate()
        .map(|(index, (output, mount))| {
            let encoder = mount
                .begin(output.format.into(), name.clone(), Some(output.kbps))
                .and_then(|sink| OutputEncoder::new(output.format, output.kbps, sink).ok());
            if encoder.is_none() {
                outputs.fail(index);
            }
            encoder
        })
        .collect();
    let ended = transcode(pushed, outputs, &mut encoders, stop);
    // Ends every output mount and its listeners.
    drop(encoders);
    match ended {
        Err(refusal) => refuse(pushed, outputs, stop, refusal),
        // The outputs ended while the source goes on: none of them comes back for it.
        Ok(()) if !stop.load(Ordering::Acquire) && pushed.stats().live => outputs.fail_all(),
        Ok(()) => {}
    }
}

/// Feeds the outputs until the source ends, `stop` is set or the source turns out not to
/// be transcodable.
fn transcode(
    pushed: &Arc<Mount>,
    outputs: &Outputs,
    encoders: &mut [Option<OutputEncoder>],
    stop: &Arc<AtomicBool>,
) -> Result<(), Refusal> {
    // Whether each output has begun its current stretch of audio.
    let mut started = vec![false; encoders.len()];
    // The decoder and the samples it gave that are not yet a whole block.
    let mut active: Option<(SourceDecoder, Vec<f32>)> = None;
    let mut seen = (String::new(), String::new());
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let now = pushed.stats();
        if !now.live {
            return Ok(());
        }
        let artist = Some(now.artist.as_str()).filter(|artist| !artist.is_empty());
        let listening: Vec<bool> = outputs
            .each()
            .zip(encoders.iter())
            .map(|((_, mount), encoder)| encoder.is_some() && mount.stats().listeners > 0)
            .collect();
        pause(
            outputs,
            encoders,
            &mut started,
            &listening,
            &now.title,
            artist,
        );
        if !listening.contains(&true) {
            // What was decoded before the pause must not play after it.
            active = None;
            std::thread::sleep(IDLE);
            continue;
        }
        if active.is_none() {
            match open_source(pushed, stop) {
                Ok(opened) => active = Some(opened),
                Err(Halt::Ended) => return Ok(()),
                Err(Halt::Refused(refusal)) => return Err(refusal),
            }
            started.fill(false);
        }
        let Some((decoder, samples)) = active.as_mut() else {
            continue;
        };

        let retitled = (&now.title, &now.artist) != (&seen.0, &seen.1);
        for (index, ((encoder, started), &listening)) in encoders
            .iter_mut()
            .zip(started.iter_mut())
            .zip(&listening)
            .enumerate()
        {
            let Some(output) = encoder.as_mut() else {
                continue;
            };
            let done = if !listening {
                Ok(())
            } else if !*started {
                *started = true;
                output.start(&now.title, artist)
            } else if retitled {
                output.retitle(&now.title, artist)
            } else {
                Ok(())
            };
            if done.is_err() {
                // This output ends; the others go on.
                outputs.fail(index);
                *encoder = None;
                *started = false;
            }
        }
        seen = (now.title, now.artist);

        let mut overrun = false;
        while samples.len() < BLOCK {
            match decoder.next(samples) {
                Ok(Decoded::Audio) => {}
                Ok(Decoded::Overrun) => {
                    overrun = true;
                    break;
                }
                Ok(Decoded::Ended) => return Ok(()),
                Err(error) => {
                    return match Halt::from(error) {
                        Halt::Ended => Ok(()),
                        Halt::Refused(refusal) => Err(refusal),
                    };
                }
            }
        }
        if overrun {
            // A new tap, and a new stretch for every output: the skipped audio is gone.
            active = None;
            continue;
        }
        let Some(block) = samples.get(..BLOCK) else {
            continue;
        };
        outputs.decoded.fetch_add(1, Ordering::Relaxed);
        for (index, (encoder, started)) in encoders.iter_mut().zip(started.iter_mut()).enumerate() {
            if let Some(output) = encoder.as_mut().filter(|_| *started)
                && output.encode(block).is_err()
            {
                outputs.fail(index);
                *encoder = None;
                *started = false;
            }
        }
        samples.drain(..BLOCK);
    }
}

/// Each output that has no listener now has a fresh stretch of audio ready with the
/// source's tags: one that was playing ends its stretch and starts a new one at once,
/// while nobody hears the end of the old one, and one that was already paused starts a
/// new one when the title changes. So whoever joins before it gains a listener again gets
/// one stretch with the right title, and nothing from before the pause. Starting a stretch
/// that is already fresh with the same tags does nothing. An output that fails to ends,
/// and is marked failed.
fn pause(
    outputs: &Outputs,
    encoders: &mut [Option<OutputEncoder>],
    started: &mut [bool],
    listening: &[bool],
    title: &str,
    artist: Option<&str>,
) {
    for (index, ((encoder, started), &listening)) in
        encoders.iter_mut().zip(started).zip(listening).enumerate()
    {
        if listening {
            continue;
        }
        *started = false;
        if encoder
            .as_mut()
            .is_some_and(|output| output.start(title, artist).is_err())
        {
            outputs.fail(index);
            *encoder = None;
        }
    }
}

/// Taps the pushed mount and decodes until the first audio, so the format is known for
/// sure; a tap that falls behind first is replaced. Gives the decoder and that audio.
fn open_source(
    pushed: &Arc<Mount>,
    stop: &Arc<AtomicBool>,
) -> Result<(SourceDecoder, Vec<f32>), Halt> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Err(Halt::Ended);
        }
        let tap = pushed.tap().ok_or(Halt::Ended)?;
        let mut decoder = match SourceDecoder::open(tap, Arc::clone(stop)) {
            Ok(decoder) => decoder,
            Err(DecodeError::Overrun) => continue,
            Err(error) => return Err(error.into()),
        };
        let mut samples = Vec::new();
        match decoder.next(&mut samples) {
            Ok(Decoded::Audio) => return Ok((decoder, samples)),
            Ok(Decoded::Overrun) => continue,
            Ok(Decoded::Ended) => return Err(Halt::Ended),
            Err(error) => return Err(error.into()),
        }
    }
}

/// Reports why the outputs are off until the source ends or `stop` is set.
fn refuse(pushed: &Mount, outputs: &Outputs, stop: &AtomicBool, refusal: Refusal) {
    outputs.set_refusal(Some(refusal));
    while !stop.load(Ordering::Acquire) && pushed.stats().live {
        std::thread::sleep(IDLE);
    }
    outputs.set_refusal(None);
}
