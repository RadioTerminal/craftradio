use jiff::Timestamp;
use jiff::civil::{self, Weekday};
use jiff::tz::{AmbiguousOffset, TimeZone};
use radio_model::{Repeat, Trigger};

/// Resolves a wall-clock time in `tz`. A time skipped by a DST gap moves forward by the gap;
/// a repeated time takes its first occurrence (jiff's "compatible" rule). A time outside
/// jiff's range yields [`Timestamp::MIN`].
pub fn resolve_local(tz: &TimeZone, at: civil::DateTime) -> Timestamp {
    try_resolve(tz, at).unwrap_or(Timestamp::MIN)
}

/// Like [`resolve_local`], but `None` for a time outside jiff's range.
fn try_resolve(tz: &TimeZone, at: civil::DateTime) -> Option<Timestamp> {
    tz.to_ambiguous_timestamp(at).compatible().ok()
}

/// Every instant at which the wall-clock time `at` happens: both passes of a DST fold,
/// otherwise the one `try_resolve` finds.
fn resolve_all(tz: &TimeZone, at: civil::DateTime) -> Vec<Timestamp> {
    let ambiguous = tz.to_ambiguous_timestamp(at);
    if matches!(ambiguous.offset(), AmbiguousOffset::Fold { .. }) {
        [ambiguous.earlier(), ambiguous.later()]
            .into_iter()
            .filter_map(Result::ok)
            .collect()
    } else {
        try_resolve(tz, at).into_iter().collect()
    }
}

/// Every instant in `[from, to)` at which a fixed or clock trigger fires, in ascending order.
///
/// `AtMinutes(m)` fires at :m of every local hour; `EveryNMinutes(n)` fires at every local
/// minute whose minute-of-day is a multiple of `n`. `EveryNTracks` and `ScriptEvent` are not
/// clock-driven and return an empty `Vec`. During a DST fold an explicit `At` time fires once,
/// at its first occurrence, while the hourly triggers (`AtMinutes`, `EveryNMinutes`) fire in
/// both passes, because both are real hours on air.
pub fn occurrences(
    tz: &TimeZone,
    trigger: &Trigger,
    from: Timestamp,
    to: Timestamp,
) -> Vec<Timestamp> {
    if from >= to {
        return Vec::new();
    }
    // Local days that can contain an instant of [from, to), padded by one day each side.
    let first = tz.to_datetime(from).date().yesterday().ok();
    let last = tz.to_datetime(to).date().tomorrow().ok();
    let (Some(first), Some(last)) = (first, last) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut day = first;
    while day <= last {
        for time in times_on(trigger, day) {
            let wall = day.to_datetime(time);
            let instants = if matches!(trigger, Trigger::At { .. }) {
                try_resolve(tz, wall).into_iter().collect()
            } else {
                resolve_all(tz, wall)
            };
            out.extend(instants.into_iter().filter(|t| from <= *t && *t < to));
        }
        match day.tomorrow() {
            Ok(next) => day = next,
            Err(_) => break,
        }
    }
    // A DST gap can map two wall-clock times onto one instant.
    out.sort();
    out.dedup();
    out
}

/// Wall-clock times at which `trigger` fires on the local date `day`.
fn times_on(trigger: &Trigger, day: civil::Date) -> Vec<civil::Time> {
    match trigger {
        Trigger::AtMinutes(minutes) => (0..24)
            .flat_map(|h| {
                minutes
                    .iter()
                    .filter_map(move |&m| time(h, i8::try_from(m).ok()?))
            })
            .collect(),
        Trigger::EveryNMinutes(n) if *n > 0 => (0..24 * 60_u32)
            .step_by(*n as usize)
            .filter_map(|m| time(i8::try_from(m / 60).ok()?, i8::try_from(m % 60).ok()?))
            .collect(),
        Trigger::At { at, repeat } if repeats_on(*repeat, at.date(), day) => vec![at.time()],
        _ => Vec::new(),
    }
}

fn time(hour: i8, minute: i8) -> Option<civil::Time> {
    civil::Time::new(hour, minute, 0, 0).ok()
}

fn repeats_on(repeat: Repeat, start: civil::Date, day: civil::Date) -> bool {
    if day < start {
        return false;
    }
    match repeat {
        Repeat::Once => day == start,
        Repeat::Daily => true,
        Repeat::Weekdays => !matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday),
        Repeat::Weekly => day.weekday() == start.weekday(),
    }
}
