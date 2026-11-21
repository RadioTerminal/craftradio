use jiff::civil::date;
use jiff::tz::TimeZone;
use radio_timing::{LocalStart, StartError, local_start};

fn ljubljana() -> TimeZone {
    TimeZone::get("Europe/Ljubljana").unwrap()
}

#[test]
fn an_ordinary_local_time_has_one_instant() {
    let at = date(2026, 11, 21).at(14, 0, 0, 0);
    let got = local_start(&ljubljana(), at).unwrap();
    assert!(matches!(got, LocalStart::Exact(t) if ljubljana().to_datetime(t) == at));
}

#[test]
fn a_time_in_the_spring_gap_does_not_exist() {
    let err = local_start(&ljubljana(), date(2027, 3, 28).at(2, 30, 0, 0)).unwrap_err();
    assert!(matches!(err, StartError::Skipped { .. }));
    assert!(err.to_string().contains("02:30"), "{err}");
}

#[test]
fn a_time_in_the_autumn_fold_happens_twice_and_the_earlier_comes_first() {
    let got = local_start(&ljubljana(), date(2026, 10, 25).at(2, 30, 0, 0)).unwrap();
    let LocalStart::Twice { earlier, later } = got else {
        panic!("expected two instants, got {got:?}");
    };
    assert!(earlier < later);
    assert_eq!(got.first(), earlier, "a fold airs at its first pass");
}
