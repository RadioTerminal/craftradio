//! Decoding the stream a station pushes, for the re-encoded outputs.
//!
//! A [`SourceDecoder`] reads a mount through a tap ([`Mount::tap`](crate::Mount::tap)) and
//! gives 48 kHz interleaved stereo. An Ogg/Opus source starts a new logical stream at each
//! title change; the decoder goes on across it with a fresh codec.

use crate::mount::{Listener, Received};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

/// The only sample rate the decoder gives, and the only one it takes.
pub const RATE: u32 = 48_000;
/// Interleaved stereo samples in one block: 20 ms.
pub const BLOCK: usize = 1920;
/// The only channel count the decoder takes.
const CHANNELS: usize = 2;
/// How long a read waits on the tap before it looks at `stop` again.
const POLL: Duration = Duration::from_millis(100);

/// Why a pushed stream cannot be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The source is not 48 kHz stereo.
    #[error("the source is {rate} Hz with {channels} channels; only 48000 Hz stereo is decoded")]
    Unsupported { rate: u32, channels: usize },
    /// The bytes are not a stream the decoders read.
    #[error("the source cannot be decoded: {0}")]
    Unreadable(String),
    /// Only from [`SourceDecoder::open`]: the source ended, or `stop` was set, before the
    /// stream's format was known.
    #[error("the source ended before its format was known")]
    Ended,
    /// Only from [`SourceDecoder::open`]: the tap fell behind and the mount dropped it
    /// before the stream's format was known; a new tap may do better.
    #[error("the tap fell behind before the stream's format was known")]
    Overrun,
}

/// What [`SourceDecoder::next`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// Samples were appended.
    Audio,
    /// The tap fell behind and the mount dropped it; nothing more will come through it.
    Overrun,
    /// The source ended, or `stop` was set.
    Ended,
}

/// Why the tap gave no more bytes. Set once, by the reader, and read by the decoder:
/// Symphonia only sees the end of the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Ended,
    Overrun,
    Stopped,
}

/// A mount's stream as a byte source: blocks on the tap until a chunk comes, the source
/// ends, the tap is dropped or `stop` is set.
struct TapSource {
    /// In a mutex only because Symphonia's sources must be `Sync`; one thread reads it.
    tap: Mutex<Listener>,
    stop: Arc<AtomicBool>,
    end: Arc<OnceLock<End>>,
    chunk: Option<Arc<[u8]>>,
    /// How much of `chunk` was read.
    at: usize,
}

impl Read for TapSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(chunk) = &self.chunk {
                let rest = chunk.get(self.at..).unwrap_or_default();
                let n = out.iter_mut().zip(rest).map(|(o, b)| *o = *b).count();
                if n > 0 {
                    self.at += n;
                    return Ok(n);
                }
                self.chunk = None;
            }
            if self.end.get().is_some() {
                return Ok(0);
            }
            if self.stop.load(Ordering::Acquire) {
                let _ = self.end.set(End::Stopped);
                return Ok(0);
            }
            let tap = self.tap.get_mut().unwrap_or_else(PoisonError::into_inner);
            match tap.recv(POLL) {
                Received::Chunk(chunk) => {
                    self.chunk = Some(chunk);
                    self.at = 0;
                }
                Received::Timeout => {}
                Received::Ended => {
                    let end = if tap.fell_behind() {
                        End::Overrun
                    } else {
                        End::Ended
                    };
                    let _ = self.end.set(end);
                    return Ok(0);
                }
            }
        }
    }
}

impl Seek for TapSource {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::other("a live stream cannot seek"))
    }
}

impl MediaSource for TapSource {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Decodes a mount's stream, read through a tap, to 48 kHz interleaved stereo.
pub struct SourceDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    codecs: CodecRegistry,
    stop: Arc<AtomicBool>,
    end: Arc<OnceLock<End>>,
    scratch: Vec<f32>,
}

impl std::fmt::Debug for SourceDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceDecoder")
            .field("track", &self.track)
            .finish_non_exhaustive()
    }
}

impl SourceDecoder {
    /// Reads the tap until the stream's format is known. `stop` ends any wait.
    /// [`DecodeError::Ended`] when the source ended or `stop` was set first,
    /// [`DecodeError::Overrun`] when the tap fell behind first.
    pub fn open(tap: Listener, stop: Arc<AtomicBool>) -> Result<Self, DecodeError> {
        let end = Arc::new(OnceLock::new());
        let source = TapSource {
            tap: Mutex::new(tap),
            stop: Arc::clone(&stop),
            end: Arc::clone(&end),
            chunk: None,
            at: 0,
        };
        let stream = MediaSourceStream::new(Box::new(source), Default::default());
        let format = symphonia::default::get_probe()
            .probe(
                &Hint::new(),
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|error| match end.get() {
                Some(End::Stopped | End::Ended) => DecodeError::Ended,
                Some(End::Overrun) => DecodeError::Overrun,
                None => DecodeError::Unreadable(error.to_string()),
            })?;
        let codecs = radio_codecs::registry();
        let (track, decoder) = track_decoder(&*format, &codecs)?;
        Ok(Self {
            format,
            decoder,
            track,
            codecs,
            stop,
            end,
            scratch: Vec::new(),
        })
    }

    /// Appends the next packet's interleaved stereo samples to `out`.
    /// `Overrun`: the tap fell behind and was dropped. `Ended`: the source ended or
    /// `stop` was set.
    pub fn next(&mut self, out: &mut Vec<f32>) -> Result<Decoded, DecodeError> {
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Ok(Decoded::Ended);
            }
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                // A new logical stream (an Ogg title change): its own track and codec.
                Err(SymError::ResetRequired) => {
                    (self.track, self.decoder) = track_decoder(&*self.format, &self.codecs)?;
                    continue;
                }
                Ok(None) => return Ok(self.ended()),
                // Once the tap is done, whatever Symphonia makes of the missing bytes is the
                // end, not a fault in the stream.
                Err(_) if self.end.get().is_some() => return Ok(self.ended()),
                Err(error) => return Err(DecodeError::Unreadable(error.to_string())),
            };
            if packet.track_id != self.track {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    let spec = buffer.spec();
                    supported(spec.rate(), spec.channels().count())?;
                    buffer.copy_to_vec_interleaved(&mut self.scratch);
                }
                // One damaged packet is skipped, as the player does.
                Err(SymError::DecodeError(_)) => continue,
                Err(error) => return Err(DecodeError::Unreadable(error.to_string())),
            }
            // A packet may decode to nothing (an Opus pre-skip); only samples count.
            if !self.scratch.is_empty() {
                out.extend_from_slice(&self.scratch);
                return Ok(Decoded::Audio);
            }
        }
    }

    /// How the stream ended, once the reader has given its last byte.
    fn ended(&self) -> Decoded {
        match self.end.get() {
            Some(End::Overrun) => Decoded::Overrun,
            Some(End::Ended | End::Stopped) | None => Decoded::Ended,
        }
    }
}

/// The audio track the reader is on now, and a decoder for it, if it is 48 kHz stereo.
fn track_decoder(
    format: &dyn FormatReader,
    codecs: &CodecRegistry,
) -> Result<(u32, Box<dyn AudioDecoder>), DecodeError> {
    let unreadable = |reason: &str| DecodeError::Unreadable(reason.into());
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| unreadable("no audio track"))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or_else(|| unreadable("no audio parameters"))?;
    supported(
        params.sample_rate.unwrap_or(0),
        params
            .channels
            .as_ref()
            .map_or(0, |channels| channels.count()),
    )?;
    let decoder = codecs
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|error| DecodeError::Unreadable(error.to_string()))?;
    Ok((track.id, decoder))
}

fn supported(rate: u32, channels: usize) -> Result<(), DecodeError> {
    if rate == RATE && channels == CHANNELS {
        Ok(())
    } else {
        Err(DecodeError::Unsupported { rate, channels })
    }
}
