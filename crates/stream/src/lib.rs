//! Threaded MP3 and Ogg/Opus output, WebRTC listening, and HTTP radio input. All samples use interleaved stereo at 48 kHz.
mod input;
#[cfg(feature = "mp3")]
mod mp3;
mod now;
mod output;
mod packets;
mod relay;
mod upnp;
mod webrtc;
pub use input::*;
pub use now::{Played, cover_messages, history_message};
pub use output::*;
pub use packets::{
    AUDIO_CHANNEL, BLOCK_CONTEXT, BLOCK_PACKETS, HASH_LEN, KIND_AUDIO, KIND_BLOCK, SIGNATURE_LEN,
    packet_hash,
};
pub use relay::{DEFAULT_TRACKERS, Identity, SIGNATURE_LINE};
pub use webrtc::*;

/// True when this build can encode MP3 (the `mp3` feature).
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
