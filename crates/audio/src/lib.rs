//! The real-time audio core: decoding, decks, the mixer, meters and offline render.
//!
//! This crate does no device I/O. The engine format is stereo interleaved f32 at 48 kHz.
//! The audio-device crate pulls Program from [`Mixer::render`] or [`DeviceRenderer`].
//! The engine controls the mixer through a [`MixerHandle`] with sample-accurate commands.

mod command;
mod deck;
mod decode;
mod error;
mod meter;
mod mixer;

pub use command::{AudioEvent, Command, CommandKind, DeckId, Strip};
pub use deck::{DeckFeeder, DeckState, LoadOptions};
pub use decode::{Clip, Decoder, MAX_CHANNELS, MAX_RATE, MIN_RATE, Probe, codecs, probe};
pub use error::AudioError;
pub use meter::{METER_FLOOR_DB, MeterReader, MeterSnapshot, SilenceDetector, StripMeter};
pub use mixer::{
    CART_VOICES, COMMAND_CAPACITY, DeviceRenderer, LiveInput, MAX_GAIN_DB, Mixer, MixerConfig,
    MixerHandle, render_offline,
};

/// The engine sample rate in Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// The engine channel count: stereo, interleaved.
pub const CHANNELS: usize = 2;
