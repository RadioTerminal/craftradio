//! The station server: takes Icecast pushes from the radio app and serves them to listeners.

mod addr;
mod config;
mod conn;
mod decode;
mod encode;
mod framing;
mod http;
mod icy;
mod key;
mod lockout;
mod mount;
pub mod pairing;
mod server;
mod stats;
mod transcode;

pub use config::{Config, MAX_SAVED_OUTPUTS, Output, OutputFormat, Station, read_saved_outputs};
pub use decode::{BLOCK, DecodeError, Decoded, RATE, SourceDecoder};
pub use encode::OutputEncoder;
pub use framing::{Format, Framer};
pub use http::{HttpError, Request, client_addr, read_body, read_request, respond};
pub use icy::{IcyWriter, METAINT};
pub use key::{hash_key, key_matches, new_key};
pub use lockout::Lockout;
pub use mount::{ListenError, Listener, ListenerCap, Mount, MountStats, Received, SourceGuard};
pub use server::{Server, ServerHandle, Timeouts};
pub use stats::{StatsEntry, status_json};
pub use transcode::{Outputs, Refusal, Transcoder, spawn};

/// Why the server could not start or do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("config: {0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("TLS: {0}")]
    Tls(String),
}
