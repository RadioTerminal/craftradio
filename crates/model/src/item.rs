use crate::ItemId;
use jiff::civil;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn yes() -> bool {
    true
}

/// Where an item's audio comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    File { path: String },
    LiveInput { device: String, channels: [u16; 2] },
    Stream { url: String },
}

/// Cue points within a file, as offsets from its start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    pub cue_in: Duration,
    pub segue: Duration,
    pub cue_out: Duration,
    pub intro_end: Option<Duration>,
    pub fade_in: Duration,
    pub fade_out: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub title: String,
    pub artist: Option<String>,
    pub source: Source,
    pub timing: Option<Timing>,
    pub planned_len: Option<Duration>,
    #[serde(default = "yes")]
    pub movable: bool,
    #[serde(default = "yes")]
    pub trimmable: bool,
    pub locked: bool,
    pub valid_from: Option<civil::Date>,
    pub valid_to: Option<civil::Date>,
    pub max_per_day: Option<u32>,
}
