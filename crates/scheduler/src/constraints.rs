use crate::history::HistoryRead;
use crate::{Entry, PlayHistory};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_model::{Constraint, Item, PlaylistId};
use radio_timing::resolve_local;
use std::time::Duration;

/// `at` minus `d`, saturating at the earliest timestamp.
fn before(at: Timestamp, d: Duration) -> Timestamp {
    at.checked_sub(d).unwrap_or(Timestamp::MIN)
}

/// Whether placing `cand` (from playlist `pl`) at `at`, right after `prev`, satisfies `c`.
/// `hist` holds real and already simulated plays.
pub fn allowed(
    c: &Constraint,
    cand: &Item,
    pl: PlaylistId,
    at: Timestamp,
    prev: Option<&Entry>,
    hist: &PlayHistory,
    tz: &TimeZone,
) -> bool {
    allowed_with(c, cand, pl, at, prev, hist, tz)
}

pub(crate) fn allowed_with(
    c: &Constraint,
    cand: &Item,
    pl: PlaylistId,
    at: Timestamp,
    prev: Option<&Entry>,
    hist: &impl HistoryRead,
    tz: &TimeZone,
) -> bool {
    match c {
        Constraint::NotAdjacentToSelf => prev.is_none_or(|p| p.playlist != pl),
        Constraint::NotAdjacentTo(other) => prev.is_none_or(|p| p.playlist != *other),
        Constraint::MinGapSameItem(gap) => hist
            .last_played(cand.id)
            .is_none_or(|last| at.duration_since(last).unsigned_abs() >= *gap),
        // A play exactly `gap` ago is allowed, as for MinGapSameItem.
        Constraint::ArtistSeparation(gap) => cand.artist.as_ref().is_none_or(|artist| {
            let cutoff = before(at, *gap);
            !hist.artist_after(artist, cutoff)
        }),
        Constraint::TitleSeparation(gap) => {
            let cutoff = before(at, *gap);
            !hist.title_after(&cand.title, cutoff)
        }
        Constraint::Daypart { from, to } => {
            let now = tz.to_datetime(at).time();
            if from == to {
                true
            } else if from < to {
                *from <= now && now < *to
            } else {
                now >= *from || now < *to
            }
        }
        Constraint::MaxPerHour(cap) => {
            let since = before(at, Duration::from_secs(3600));
            hist.playlist_since(pl, since)
                .iter()
                .map(|times| times.len())
                .sum::<usize>()
                < *cap as usize
        }
        Constraint::MaxPerDay(cap) => {
            plays_on_local_day(tz, at, |start| hist.playlist_since(pl, start)) < *cap as usize
        }
    }
}

/// Indexed plays on the local calendar day that contains `at`.
fn plays_on_local_day<'a>(
    tz: &TimeZone,
    at: Timestamp,
    times: impl FnOnce(Timestamp) -> [&'a [Timestamp]; 2],
) -> usize {
    let day = tz.to_datetime(at).date();
    let start = resolve_local(tz, day.at(0, 0, 0, 0));
    times(start)
        .into_iter()
        .map(|times| {
            times
                .iter()
                .take_while(|&&at| tz.to_datetime(at).date() == day)
                .count()
        })
        .sum()
}

/// Item validity at `at`: inside its valid_from / valid_to local dates and under its
/// `max_per_day` cap.
pub(crate) fn item_ok(item: &Item, at: Timestamp, hist: &impl HistoryRead, tz: &TimeZone) -> bool {
    let day = tz.to_datetime(at).date();
    if item.valid_from.is_some_and(|from| day < from) || item.valid_to.is_some_and(|to| day > to) {
        return false;
    }
    item.max_per_day.is_none_or(|cap| {
        plays_on_local_day(tz, at, |start| hist.item_since(item.id, start)) < cap as usize
    })
}
