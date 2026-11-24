//! Pure serde data model for the radio station: items, playlists, rules and constraints.

mod air;
mod branding;
mod decks;
mod dynamics;
mod error;
mod ids;
mod item;
mod loudness;
mod playlist;
mod rule;
mod server;
mod station;

pub use air::{AIR_PART, ON_AIR_KEY, on_air};
pub use branding::{BRANDING_LOGO_KEY, BRANDING_NAME_KEY, BRANDING_PART, DEFAULT_STATION_NAME};
pub use decks::{DECK_COUNT_KEY, DECKS_PART, DEFAULT_DECKS, MAX_DECKS, MIN_DECKS, deck_count};
pub use dynamics::{
    CompressorSettings, DynamicsSettings, LimiterSettings, range as dynamics_range,
};
pub use error::ModelError;
pub use ids::{ItemId, PlaylistId};
pub use item::{Item, ItemMeta, PublishMode, Source, Timing, stream_title};
pub use loudness::{
    Clamp, GainSource, ItemGain, Loudness, LoudnessSettings, MAX_CUT_DB, PEAK_CEILING_DB,
    check_trim, range as loudness_range,
};
pub use playlist::{FolderSource, OnInterrupt, PickOrder, Playlist, PlaylistRole};
pub use rule::{Constraint, FixMode, Placement, Repeat, Rule, Trigger};
pub use server::{
    PAIRING_VERSION, PairingDocument, PairingIcecast, PairingOutput, SERVER_ADDRESS_KEY,
    SERVER_PART, SERVER_PUSH_KEY,
};
pub use station::Station;

/// Supported audio file extensions for imports and playback.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "flac", "wav", "ogg", "m4a", "opus", "aac"];
