use radio_model::{ItemId, ModelError, PlaylistId};

/// Why a command failed. A failed command changes nothing.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown playlist {0:?}")]
    UnknownPlaylist(PlaylistId),
    #[error("unknown item {0:?}")]
    UnknownItem(ItemId),
    #[error("deck {0} does not exist (decks are 0..=3)")]
    UnknownDeck(u8),
    #[error("bad argument: {0}")]
    BadArgument(String),
    /// The rules script failed; the line is in the message and in [`EngineError::script_line`].
    #[error("{0}")]
    Script(#[from] radio_script::ScriptError),
    #[error("station would become invalid: {0}")]
    Model(#[from] ModelError),
    #[error("storage error: {0}")]
    Store(String),
}

impl EngineError {
    /// The 1-based script line, when this is a script error that has one.
    pub fn script_line(&self) -> Option<u32> {
        use radio_script::ScriptError::*;
        match self {
            Self::Script(Syntax { line, .. } | Runtime { line, .. } | Invalid { line, .. }) => {
                Some(*line)
            }
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for EngineError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e.to_string())
    }
}
