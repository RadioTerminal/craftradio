use crate::{Constraint, Item, ItemId, PlaylistId, Rule};
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

/// The default of [`Playlist::repeat_gap`]: a song comes back after an hour at the soonest.
pub const DEFAULT_REPEAT_GAP: std::time::Duration = std::time::Duration::from_secs(60 * 60);

fn default_repeat_gap() -> std::time::Duration {
    DEFAULT_REPEAT_GAP
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum PickOrder {
    InOrder,
    Shuffle { seed: u64 },
    Weighted { weights: Vec<(ItemId, u32)> },
    LeastRecentlyPlayed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Playlist {
    pub id: PlaylistId,
    #[serde(default)]
    pub source: Option<FolderSource>,
    pub name: String,
    pub color: [u8; 3],
    #[serde(default = "yes")]
    pub enabled: bool,
    pub is_main: bool,
    pub is_fill_pool: bool,
    /// What the playlist is for. A system playlist has no tab, and nobody deletes it.
    #[serde(default)]
    pub role: PlaylistRole,
    /// What a restart does with an item of this playlist that a crash cut off.
    #[serde(default)]
    pub on_interrupt: OnInterrupt,
    /// How its items play: one after another on the decks, or over the music.
    #[serde(default)]
    pub plays: PlayMode,
    pub priority: i32,
    pub pick: PickOrder,
    pub items: Vec<Item>,
    pub rules: Vec<Rule>,
    pub constraints: Vec<Constraint>,
    /// How long after a song plays the scheduler may pick it again from this playlist.
    /// Zero lets a song repeat at once. The main playlist plays a song again sooner only
    /// when no song of it is free, so it never runs dry.
    #[serde(default = "default_repeat_gap")]
    pub repeat_gap: std::time::Duration,
}

/// What a playlist is for.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum PlaylistRole {
    /// A playlist that a person made.
    #[default]
    User,
    /// The system playlist "One-off". It holds the one-off items of Upcoming. Each item has
    /// one At rule that names it with [`Constraint::OnlyItem`](crate::Constraint::OnlyItem).
    OneOff,
}

/// What playout does after a restart with an item that the crash cut off (spec §6.11).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum OnInterrupt {
    /// Continue at the position that wall-clock time gives.
    #[default]
    Resume,
    /// Play it again from the start, for example an ad. The log marks the cut-off airing
    /// as interrupted.
    Replay,
    /// Do not play it again, and continue with the next item.
    Skip,
}

/// The duck depth that a new over playlist starts with, in dB.
pub const DEFAULT_DUCK_DB: i16 = -12;
/// The deepest duck that a playlist may set, in dB.
pub const MAX_DUCK_DB: i16 = -40;

/// How the items of a playlist play.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum PlayMode {
    /// One after another on the decks, each taking its own airtime.
    #[default]
    InSequence,
    /// Over a song's intro or outro, on the cart bus, lowering the music by `duck_db`
    /// (whole dB, `MAX_DUCK_DB..=0`; 0 plays over without ducking). An item that fits no
    /// intro or outro plays in sequence.
    Over { duck_db: i16 },
}

/// A folder that supplies playlist files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FolderSource {
    pub path: String,
    pub recursive: bool,
    pub watch: bool,
}
