//! Encoding audio into an output mount, as a source would push it.
//!
//! An [`OutputEncoder`] takes 48 kHz stereo blocks of [`BLOCK`] samples and writes Ogg/Opus
//! or MP3 through the output mount's [`SourceGuard`], so the mount frames it, reads its
//! tags and serves it like any pushed stream.

use crate::config::OutputFormat;
use crate::decode::BLOCK;
use crate::framing::Format;
use crate::mount::SourceGuard;
use radio_codecs::encode::{EncodeError, Mp3Encoder, OggMuxer, OpusEncoder};
use std::io::{self, Write};

/// The most bytes passed to [`SourceGuard::write`] at once (see its doc comment).
const PIECE: usize = 4096;

impl From<OutputFormat> for Format {
    fn from(format: OutputFormat) -> Format {
        match format {
            OutputFormat::Opus => Format::Ogg,
            OutputFormat::Mp3 => Format::Mp3,
        }
    }
}

/// The output mount as a writer, passing bytes on in pieces of at most [`PIECE`].
#[derive(Debug)]
struct MountWriter(SourceGuard);

impl MountWriter {
    fn write_pieces(&mut self, bytes: &[u8]) {
        for piece in bytes.chunks(PIECE) {
            self.0.write(piece);
        }
    }
}

impl Write for MountWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let piece = bytes.get(..PIECE).unwrap_or(bytes);
        self.0.write(piece);
        Ok(piece.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Encodes one output of a station into its mount.
pub struct OutputEncoder(Codec);

enum Codec {
    Opus {
        kbps: u32,
        encoder: OpusEncoder,
        muxer: OggMuxer<MountWriter>,
        /// Whether a chain has begun and not yet been closed.
        open: bool,
    },
    Mp3 {
        kbps: u32,
        encoder: Mp3Encoder,
        sink: MountWriter,
    },
}

impl OutputEncoder {
    /// `sink` is the output mount's source guard.
    pub fn new(format: OutputFormat, kbps: u32, sink: SourceGuard) -> Result<Self, EncodeError> {
        let sink = MountWriter(sink);
        Ok(OutputEncoder(match format {
            OutputFormat::Opus => Codec::Opus {
                kbps,
                encoder: OpusEncoder::new(kbps)?,
                muxer: OggMuxer::new(sink),
                open: false,
            },
            OutputFormat::Mp3 => Codec::Mp3 {
                kbps,
                encoder: Mp3Encoder::new(kbps)?,
                sink,
            },
        }))
    }

    /// Begins a stretch of audio: call before the first block and after a pause. For Opus
    /// a new chain with these tags (closing an open one first); for MP3 a fresh encoder,
    /// so samples held from before the pause are not played after it. For a title change
    /// in the middle of a stretch, call [`retitle`](Self::retitle) instead.
    pub fn start(&mut self, title: &str, artist: Option<&str>) -> Result<(), EncodeError> {
        match &mut self.0 {
            Codec::Opus {
                kbps,
                encoder,
                muxer,
                open,
            } => {
                let mut fresh = OpusEncoder::new(*kbps)?;
                let pre_skip = fresh.pre_skip()?;
                if *open {
                    close_chain(encoder, muxer, open)?;
                }
                *encoder = fresh;
                muxer.start(title, artist, pre_skip)?;
                *open = true;
            }
            Codec::Mp3 { kbps, encoder, .. } => {
                *encoder = Mp3Encoder::new(*kbps)?;
            }
        }
        Ok(())
    }

    /// Changes the title in the middle of a stretch of audio, after [`start`](Self::start).
    /// For Opus the chain is closed and a new one begins with these tags; for MP3, whose
    /// stream carries no title, nothing changes and the audio goes on unbroken. Before
    /// `start`, an Opus output refuses it, as it refuses [`encode`](Self::encode).
    pub fn retitle(&mut self, title: &str, artist: Option<&str>) -> Result<(), EncodeError> {
        match &mut self.0 {
            Codec::Opus { open: false, .. } => Err(not_started()),
            Codec::Opus { .. } => self.start(title, artist),
            Codec::Mp3 { .. } => Ok(()),
        }
    }

    /// Encodes one block of exactly `BLOCK` samples into the mount.
    pub fn encode(&mut self, block: &[f32]) -> Result<(), EncodeError> {
        if block.len() != BLOCK {
            return Err(EncodeError::Invalid(format!(
                "a block is {BLOCK} samples, not {}",
                block.len()
            )));
        }
        match &mut self.0 {
            Codec::Opus {
                encoder,
                muxer,
                open,
                ..
            } => {
                if !*open {
                    return Err(not_started());
                }
                muxer.packet(encoder.encode(block)?, false)?;
            }
            Codec::Mp3 { encoder, sink, .. } => {
                sink.write_pieces(&encoder.encode(block)?);
            }
        }
        Ok(())
    }
}

/// Why an Opus output refused to go on before [`OutputEncoder::start`].
fn not_started() -> EncodeError {
    EncodeError::Invalid("an Opus output takes audio and titles only after start".into())
}

/// Closes the open chain as the app's output does: a silent last packet marked as the end.
fn close_chain(
    encoder: &mut OpusEncoder,
    muxer: &mut OggMuxer<MountWriter>,
    open: &mut bool,
) -> Result<(), EncodeError> {
    muxer.packet(encoder.encode(&[0.0; BLOCK])?, true)?;
    *open = false;
    Ok(())
}
