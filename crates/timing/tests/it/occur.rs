use jiff::Timestamp;
use jiff::civil::{self, date};
use jiff::tz::TimeZone;
use radio_model::{Repeat, Trigger};
use radio_timing::{occurrences, resolve_local};

fn ljubljana() -> TimeZone {
    TimeZone::get("Europe/Ljubljana").unwrap()
}

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn local(at: civil::DateTime) -> Timestamp {
    resolve_local(&ljubljana(), at)
}

#[test]
fn dst_gap_moves_forward() {
    let got = local(date(2027, 3, 28).at(2, 30, 0, 0));
    assert_eq!(got, ts("2027-03-28T01:30:00Z"));
}

#[test]
fn dst_fold_takes_first() {
    let got = local(date(2026, 10, 25).at(2, 30, 0, 0));
    assert_eq!(got, ts("2026-10-25T00:30:00Z"));
}

#[test]
fn at_minutes_20_50_over_one_hour() {
    let from = local(date(2026, 11, 21).at(14, 0, 0, 0));
    let to = local(date(2026, 11, 21).at(15, 0, 0, 0));
    let got = occurrences(&ljubljana(), &Trigger::AtMinutes(vec![20, 50]), from, to);
    assert_eq!(
        got,
        vec![
            local(date(2026, 11, 21).at(14, 20, 0, 0)),
            local(date(2026, 11, 21).at(14, 50, 0, 0))
        ]
    );
}

#[test]
fn daily_repeat_spans_midnight() {
    let trigger = Trigger::At {
        at: date(2026, 1, 1).at(0, 0, 0, 0),
        repeat: Repeat::Daily,
    };
    let from = local(date(2026, 11, 21).at(23, 0, 0, 0));
    let to = local(date(2026, 11, 22).at(1, 0, 0, 0));
    let got = occurrences(&ljubljana(), &trigger, from, to);
    assert_eq!(got, vec![local(date(2026, 11, 22).at(0, 0, 0, 0))]);
}

#[test]
fn weekdays_skips_weekend() {
    // 2026-11-21 is a Saturday.
    let trigger = Trigger::At {
        at: date(2026, 1, 5).at(7, 0, 0, 0),
        repeat: Repeat::Weekdays,
    };
    let from = local(date(2026, 11, 21).at(0, 0, 0, 0));
    let to = local(date(2026, 11, 24).at(0, 0, 0, 0));
    let got = occurrences(&ljubljana(), &trigger, from, to);
    assert_eq!(got, vec![local(date(2026, 11, 23).at(7, 0, 0, 0))]);
}

#[test]
fn once_in_past_yields_nothing() {
    let trigger = Trigger::At {
        at: date(2020, 1, 1).at(7, 0, 0, 0),
        repeat: Repeat::Once,
    };
    let from = local(date(2026, 11, 21).at(0, 0, 0, 0));
    let to = local(date(2026, 11, 22).at(0, 0, 0, 0));
    assert!(occurrences(&ljubljana(), &trigger, from, to).is_empty());
}
