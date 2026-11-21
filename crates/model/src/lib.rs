//! Pure serde data model for the radio station: items, playlists, rules and constraints.

mod error;
mod ids;
mod item;
mod playlist;
mod rule;
mod station;

pub use error::ModelError;
pub use ids::{ItemId, PlaylistId};
pub use item::{Item, ItemMeta, PublishMode, Source, Timing, stream_title};
pub use playlist::{FolderSource, OnInterrupt, PickOrder, Playlist};
pub use rule::{Constraint, FixMode, Placement, Repeat, Rule, Trigger};
pub use station::Station;

/// Supported audio file extensions for imports and playback.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "flac", "wav", "ogg", "m4a", "opus", "aac"];
