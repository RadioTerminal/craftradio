use crate::PlaylistId;
use jiff::civil;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn one() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    EveryNTracks(u32),
    EveryNMinutes(u32),
    AtMinutes(Vec<u8>),
    At { at: civil::DateTime, repeat: Repeat },
    ScriptEvent(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    Once,
    Daily,
    Weekdays,
    Weekly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixMode {
    Soft,
    Hard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    BetweenTracks,
    Fixed { mode: FixMode, tolerance: Duration },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub trigger: Trigger,
    pub placement: Placement,
    #[serde(default = "one")]
    pub take: u8,
    pub constraints: Vec<Constraint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Constraint {
    NotAdjacentToSelf,
    NotAdjacentTo(PlaylistId),
    MinGapSameItem(Duration),
    ArtistSeparation(Duration),
    TitleSeparation(Duration),
    Daypart { from: civil::Time, to: civil::Time },
    MaxPerHour(u32),
    MaxPerDay(u32),
}
