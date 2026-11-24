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

    /// Begins a stretch of audio: for Opus a new chain with these tags, for MP3 a
    /// fresh encoder. Call before the first block, after a pause, and when the title
    /// changes (Opus).
    pub fn start(&mut self, title: &str, artist: Option<&str>) -> Result<(), EncodeError> {
        match &mut self.0 {
            Codec::Opus {
                kbps,
                encoder,
                muxer,
                open,
            } => {
                if *open {
                    // Closes the chain as the app's output does: a silent last packet
                    // marked as the end, and a fresh encoder for the next chain.
                    muxer.packet(encoder.encode(&[0.0; BLOCK])?, true)?;
                    *open = false;
                    *encoder = OpusEncoder::new(*kbps)?;
                }
                muxer.start(title, artist, encoder.pre_skip()?)?;
                *open = true;
            }
            Codec::Mp3 { kbps, encoder, .. } => {
                *encoder = Mp3Encoder::new(*kbps)?;
            }
        }
        Ok(())
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
                    return Err(EncodeError::Invalid(
                        "an Opus output encodes only after start".into(),
                    ));
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
