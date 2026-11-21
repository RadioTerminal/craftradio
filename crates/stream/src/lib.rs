//! Threaded MP3 and Ogg/Opus output and HTTP radio input. Samples are interleaved stereo at 48 kHz.
mod input;
#[cfg(feature = "mp3")]
mod mp3;
mod output;
pub use input::*;
pub use output::*;

/// True when this build can encode MP3 (the `mp3` feature, LAME).
pub const MP3: bool = cfg!(feature = "mp3");

/// Errors from configuration, codecs, or transport.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Opus(#[from] opus::Error),
}
