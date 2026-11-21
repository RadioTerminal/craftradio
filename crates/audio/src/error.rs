//! Errors of the audio core.

use std::path::PathBuf;

/// An error from decoding, loading or control of the mixer.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The system cannot open the file.
    #[error("cannot open {path}: {source}")]
    Open {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The decoder does not support the format, codec or tracks of the file.
    #[error("cannot decode {path}: {reason}")]
    Unsupported {
        /// The file.
        path: PathBuf,
        /// The cause.
        reason: String,
    },
    /// The decoder stopped with an error in the middle of the stream.
    #[error("decode failed: {0}")]
    Decode(String),
    /// The resampler cannot convert the sample rate of the file.
    #[error("resampler failed: {0}")]
    Resample(String),
    /// The queue to the render thread is full. Try again later.
    #[error("the mixer queue is full")]
    QueueFull,
    /// The command has a value that the mixer cannot use, for example a gain that is NaN.
    #[error("invalid command: {0}")]
    InvalidCommand(&'static str),
    /// The cartwall has no slot with this number.
    #[error("cart slot {0} does not exist")]
    NoSuchCartSlot(usize),
    /// A setting of the Program compressor or limiter is out of range.
    #[error("cannot set up Program dynamics: {0}")]
    Dynamics(String),
    /// The system cannot start the decoder thread.
    #[error("cannot start the decoder thread: {0}")]
    Thread(std::io::Error),
}
