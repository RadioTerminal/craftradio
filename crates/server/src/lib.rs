//! The station server: takes Icecast pushes from the radio app and serves them to listeners.

mod config;
mod key;

pub use config::{Config, Station};
pub use key::{hash_key, key_matches, new_key};

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
