//! The station server: takes Icecast pushes from the radio app and serves them to listeners.

mod config;
mod http;
mod key;
mod lockout;

pub use config::{Config, Station};
pub use http::{HttpError, Request, client_addr, read_request, respond};
pub use key::{hash_key, key_matches, new_key};
pub use lockout::Lockout;

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
