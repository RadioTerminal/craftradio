use crate::{Constraint, Item, ItemId, PlaylistId, Rule};
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickOrder {
    InOrder,
    Shuffle { seed: u64 },
    Weighted { weights: Vec<(ItemId, u32)> },
    LeastRecentlyPlayed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub priority: i32,
    pub pick: PickOrder,
    pub items: Vec<Item>,
    pub rules: Vec<Rule>,
    pub constraints: Vec<Constraint>,
}

/// A folder that supplies playlist files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSource {
    pub path: String,
    pub recursive: bool,
    pub watch: bool,
}
