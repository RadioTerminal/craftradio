use crate::fixtures::*;
use jiff::{SignedDuration, Timestamp};
use radio_engine::{LogEntry, LogStatus};
use radio_ui_egui::view::{AirStatus, Edit, HistoryReport, ViewSource};
use std::time::{Duration, Instant};

/// Logs an airing of `title` from `start` for a minute, placed by `rule`.
fn air(source: &radio_ui_engine::EngineSource, start: Timestamp, title: &str, status: LogStatus) {
    let log = source.playout().log();
    let row = log
        .log_start(&LogEntry {
            start,
            playlist: "Ads".into(),
            title: title.into(),
            artist: Some("Kesten Bakery".into()),
            origin: "rule".into(),
            rule: Some("every hour".into()),
        })
        .unwrap();
    log.log_end(row, start + SignedDuration::from_mins(1), status)
        .unwrap();
}

/// History shows the last hours until a person picks a range; then it shows the rows of that
/// range, each with how its airing ended and the rule that placed it.
#[test]
fn a_picked_range_shows_the_airings_of_that_day_with_status_and_rule() {
    let (mut source, clock) = source(Vec::new());
    let yesterday = t0() - SignedDuration::from_hours(26);
    air(&source, yesterday, "Spot A", LogStatus::Interrupted);
    air(
        &source,
        t0() - SignedDuration::from_mins(10),
        "Spot B",
        LogStatus::Aired,
    );

    let has = |title: &'static str| {
        move |v: &radio_ui_egui::view::StationView| {
            v.history.iter().any(|r| r.title.contains(title))
        }
    };
    let recent = wait_for(&mut source, &clock, has("Spot B"));
    assert!(has("Spot B")(&recent), "the last hours show the new airing");
    assert!(!has("Spot A")(&recent), "and not the one of the day before");

    let day = (
        yesterday - SignedDuration::from_hours(1),
        yesterday + SignedDuration::from_hours(1),
    );
    source.history_range(Some(day));
    let view = wait_for(&mut source, &clock, has("Spot A"));
    let row = view
        .history
        .iter()
        .find(|r| r.title.contains("Spot A"))
        .expect("the picked range shows its airing");
    let aired = row.aired.as_ref().expect("a log row says how it aired");
    assert_eq!(aired.status, AirStatus::CutOff);
    assert_eq!(aired.rule.as_deref(), Some("every hour"));
    assert!(aired.end.as_ref().is_some_and(|end| *end > row.start));
    assert!(!has("Spot B")(&view), "rows outside the range are gone");
}

/// The royalty report goes to the file that the person chose.
#[test]
fn the_royalty_export_writes_one_line_per_title_to_the_file() {
    let (mut source, _clock) = source(Vec::new());
    let start = t0() - SignedDuration::from_hours(2);
    air(&source, start, "Spot A", LogStatus::Aired);
    air(
        &source,
        start + SignedDuration::from_mins(5),
        "Spot A",
        LogStatus::Aired,
    );
    let path = dir().join("royalty.csv");
    source
        .edit(Edit::ExportHistory {
            path: path.display().to_string(),
            report: HistoryReport::Plays,
            since: start,
            until: t0(),
            playlist: None,
        })
        .unwrap();
    // The file exists before its text is all there, so wait for the line itself.
    let line = "Kesten Bakery,Spot A,2,120";
    let read = || std::fs::read_to_string(&path).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !read().lines().any(|l| l == line) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(read().lines().any(|l| l == line), "{}", read());
}
