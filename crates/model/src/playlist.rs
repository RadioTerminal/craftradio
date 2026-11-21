use crate::{Constraint, Item, ItemId, PlaylistId, Rule};
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
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
    /// What a restart does with an item of this playlist that a crash cut off.
    #[serde(default)]
    pub on_interrupt: OnInterrupt,
    pub priority: i32,
    pub pick: PickOrder,
    pub items: Vec<Item>,
    pub rules: Vec<Rule>,
    pub constraints: Vec<Constraint>,
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

/// A folder that supplies playlist files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FolderSource {
    pub path: String,
    pub recursive: bool,
    pub watch: bool,
}
