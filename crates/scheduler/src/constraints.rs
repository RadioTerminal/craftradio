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
    match c {
        Constraint::NotAdjacentToSelf => prev.is_none_or(|p| p.playlist != pl),
        Constraint::NotAdjacentTo(other) => prev.is_none_or(|p| p.playlist != *other),
        Constraint::MinGapSameItem(gap) => hist
            .last_played(cand.id)
            .is_none_or(|last| at.duration_since(last).unsigned_abs() >= *gap),
        Constraint::ArtistSeparation(gap) => cand.artist.as_ref().is_none_or(|artist| {
            hist.count_since(|p| p.artist.as_ref() == Some(artist), before(at, *gap)) == 0
        }),
        Constraint::TitleSeparation(gap) => {
            hist.count_since(|p| p.title == cand.title, before(at, *gap)) == 0
        }
        Constraint::Daypart { from, to } => {
            let now = tz.to_datetime(at).time();
            if from <= to {
                *from <= now && now < *to
            } else {
                now >= *from || now < *to
            }
        }
        Constraint::MaxPerHour(cap) => {
            let since = before(at, Duration::from_secs(3600));
            hist.count_since(|p| p.playlist == pl, since) < *cap as usize
        }
        Constraint::MaxPerDay(cap) => {
            plays_on_local_day(hist, tz, at, |p| p.playlist == pl) < *cap as usize
        }
    }
}

/// Plays matching `pred` on the local calendar day that contains `at`.
fn plays_on_local_day(
    hist: &PlayHistory,
    tz: &TimeZone,
    at: Timestamp,
    pred: impl Fn(&crate::Played) -> bool,
) -> usize {
    let day = tz.to_datetime(at).date();
    let start = resolve_local(tz, day.at(0, 0, 0, 0));
    hist.count_since(|p| pred(p) && tz.to_datetime(p.at).date() == day, start)
}

/// Item validity at `at`: inside its valid_from / valid_to local dates and under its
/// `max_per_day` cap.
pub(crate) fn item_ok(item: &Item, at: Timestamp, hist: &PlayHistory, tz: &TimeZone) -> bool {
    let day = tz.to_datetime(at).date();
    if item.valid_from.is_some_and(|from| day < from) || item.valid_to.is_some_and(|to| day > to) {
        return false;
    }
    item.max_per_day
        .is_none_or(|cap| plays_on_local_day(hist, tz, at, |p| p.item == item.id) < cap as usize)
}
