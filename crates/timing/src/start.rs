//! A start time that a person picks as a local date and time.

use jiff::Timestamp;
use jiff::civil;
use jiff::tz::{AmbiguousOffset, TimeZone};

/// The instants of a local date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalStart {
    /// The time happens once.
    Exact(Timestamp),
    /// The clocks go back, so the time happens twice.
    Twice {
        earlier: Timestamp,
        later: Timestamp,
    },
}

impl LocalStart {
    /// The instant that a fixed-time rule uses: the first pass of a fold.
    pub fn first(self) -> Timestamp {
        match self {
            Self::Exact(t) | Self::Twice { earlier: t, .. } => t,
        }
    }
}

/// Why a local date and time is no start time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    /// The clocks go forward over this time, so it never happens.
    #[error(
        "{} does not exist in {tz}: the clocks go forward to {}. Pick another time.",
        .at.strftime("%Y-%m-%d %H:%M"),
        .resumes.strftime("%H:%M")
    )]
    Skipped {
        at: civil::DateTime,
        tz: String,
        resumes: civil::DateTime,
    },
    /// The time is outside the range of the calendar.
    #[error("{at} is out of range")]
    OutOfRange { at: civil::DateTime },
}

/// Finds the instants of the local time `at` in `tz`. A time in a DST gap is an error.
pub fn local_start(tz: &TimeZone, at: civil::DateTime) -> Result<LocalStart, StartError> {
    let ambiguous = tz.to_ambiguous_timestamp(at);
    let out_of_range = |_| StartError::OutOfRange { at };
    match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { .. } => Ok(LocalStart::Exact(
            ambiguous.compatible().map_err(out_of_range)?,
        )),
        AmbiguousOffset::Fold { .. } => Ok(LocalStart::Twice {
            earlier: ambiguous.earlier().map_err(out_of_range)?,
            later: ambiguous.later().map_err(out_of_range)?,
        }),
        AmbiguousOffset::Gap { .. } => {
            let moved = ambiguous.compatible().map_err(out_of_range)?;
            let resumes = tz.preceding(moved).next().map_or(moved, |t| t.timestamp());
            Err(StartError::Skipped {
                at,
                tz: tz.iana_name().unwrap_or("the station time zone").to_owned(),
                resumes: tz.to_datetime(resumes),
            })
        }
    }
}
