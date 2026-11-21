use serde::{Deserialize, Serialize};

/// Identifier of an [`Item`](crate::Item), unique within a station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId(pub u64);

/// Identifier of a [`Playlist`](crate::Playlist), unique within a station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlaylistId(pub u64);
