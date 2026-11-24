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
        // A thread that could not start leaves the outputs off: their mounts never go live.
        std::thread::Builder::new()
            .name("server-transcoder".into())
            .spawn(move || run(&pushed, &outputs, &stop))
            .ok()
    };
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

/// The thread: checks the source, begins the outputs and feeds them while they are heard.
fn run(pushed: &Arc<Mount>, outputs: &Outputs, stop: &Arc<AtomicBool>) {
    if outputs.is_empty() {
        return;
    }
    // A source that can't be decoded never makes an output live.
    match open_source(pushed, stop) {
        Ok(_) => {}
        Err(Halt::Ended) => return,
        Err(Halt::Refused(refusal)) => return refuse(pushed, outputs, stop, refusal),
    }
    let name = pushed.stats().name;
    let mut encoders: Vec<Option<OutputEncoder>> = outputs
        .each()
        .map(|(output, mount)| {
            let sink = mount.begin(output.format.into(), name.clone(), Some(output.kbps))?;
            OutputEncoder::new(output.format, output.kbps, sink).ok()
        })
        .collect();
    let ended = transcode(pushed, outputs, &mut encoders, stop);
    // Ends every output mount and its listeners.
    drop(encoders);
    if let Err(refusal) = ended {
        refuse(pushed, outputs, stop, refusal);
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
        let listening: Vec<bool> = outputs
            .each()
            .zip(encoders.iter())
            .map(|((_, mount), encoder)| encoder.is_some() && mount.stats().listeners > 0)
            .collect();
        if !listening.contains(&true) {
            // A pause: what was decoded before it must not play after it.
            active = None;
            started.fill(false);
            if !pushed.stats().live {
                return Ok(());
            }
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

        let now = pushed.stats();
        let retitled = (&now.title, &now.artist) != (&seen.0, &seen.1);
        let artist = Some(now.artist.as_str()).filter(|artist| !artist.is_empty());
        for ((encoder, started), &listening) in
            encoders.iter_mut().zip(started.iter_mut()).zip(&listening)
        {
            let Some(output) = encoder.as_mut() else {
                continue;
            };
            let done = if !listening {
                *started = false;
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
        for (encoder, started) in encoders.iter_mut().zip(started.iter_mut()) {
            if let Some(output) = encoder.as_mut().filter(|_| *started)
                && output.encode(block).is_err()
            {
                *encoder = None;
                *started = false;
            }
        }
        samples.drain(..BLOCK);
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
