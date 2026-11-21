use crate::ItemId;

/// Reasons a [`Station`](crate::Station) fails validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("station has no main playlist")]
    NoMainPlaylist,
    #[error("station has more than one main playlist")]
    MultipleMainPlaylists,
    #[error("item {item:?} has bad timing: {reason}")]
    BadTiming { item: ItemId, reason: &'static str },
    #[error("item {0:?} needs a planned length")]
    MissingPlannedLen(ItemId),
    #[error("unknown time zone {0:?}")]
    UnknownTimeZone(String),
    #[error("duplicate id {0}")]
    DuplicateId(u64),
}
