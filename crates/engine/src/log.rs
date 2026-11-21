use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Row id of an as-played entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RowId(pub i64);

/// What is known when an item goes to air.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub start: Timestamp,
    pub playlist: String,
    pub title: String,
    pub artist: Option<String>,
    /// Source and type, e.g. "music", "jingle", "ad", "live".
    pub origin: String,
    /// The rule that placed it, if any.
    pub rule: Option<String>,
}

/// How an airing ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogStatus {
    Aired,
    Interrupted,
    /// Found without an end after a restart (crash).
    Unfinished,
}

impl LogStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Aired => "aired",
            Self::Interrupted => "interrupted",
            Self::Unfinished => "unfinished",
        }
    }
}

/// One as-played row. `status` is `None` while the item is still on air.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsPlayed {
    pub id: RowId,
    pub entry: LogEntry,
    pub end: Option<Timestamp>,
    pub status: Option<LogStatus>,
}

/// Counts for one playlist on one day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Items the playlist holds. Until planning is wired in, this is the playlist's item count.
    pub scheduled: u32,
    pub aired: u32,
    pub interrupted: u32,
}
