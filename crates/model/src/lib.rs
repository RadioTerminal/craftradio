//! Pure serde data model for the radio station: items, playlists, rules and constraints.

mod dynamics;
mod error;
mod ids;
mod item;
mod loudness;
mod playlist;
mod rule;
mod station;

pub use dynamics::{
    CompressorSettings, DynamicsSettings, LimiterSettings, range as dynamics_range,
};
pub use error::ModelError;
pub use ids::{ItemId, PlaylistId};
pub use loudness::{
    Clamp, GainSource, ItemGain, Loudness, LoudnessSettings, MAX_CUT_DB, PEAK_CEILING_DB, check_trim,
    range as loudness_range,
};
pub use item::{Item, ItemMeta, PublishMode, Source, Timing, stream_title};
pub use playlist::{FolderSource, OnInterrupt, PickOrder, Playlist};
pub use rule::{Constraint, FixMode, Placement, Repeat, Rule, Trigger};
pub use station::Station;

/// Supported audio file extensions for imports and playback.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "flac", "wav", "ogg", "m4a", "opus", "aac"];
