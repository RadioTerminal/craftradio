use crate::fixtures::*;
use radio_engine::LogStatus;

/// Track 2 starts within 5 ms of the segue of track 1, while track 1 still fades out.
/// Then track 3 follows, not track 1 again, and the log has a row for each start.
#[test]
fn two_tracks_crossfade_at_segue() {
    let one = file_item(1, "One", &wav("one", 2.0, Side::Left), 2.0, 1.5, 0.5);
    let two = file_item(2, "Two", &wav("two", 2.0, Side::Right), 2.0, 1.5, 0.5);
    let three = file_item(3, "Three", &wav("three", 2.0, Side::Both), 2.0, 1.5, 0.5);
    let (p, clock) = offline(engine_with(vec![one, two, three]), |_| {});
    let program = run(&p, &clock, 4.0);

    let start1 = first_audible(&program, 0, 0).expect("track 1 sounds");
    let start2 = first_audible(&program, 1, 0).expect("track 2 sounds");
    let segue = start1 + (1.5 * SR as f32) as usize;
    assert!(
        start2.abs_diff(segue) <= SR / 200,
        "track 2 at {start2}, segue at {segue}"
    );
    assert!(
        first_audible(&program, 0, start2 + SR / 10).is_some_and(|f| f < start2 + SR / 5),
        "track 1 does not overlap track 2"
    );

    let rows = p.history(t0(), 10).unwrap();
    let titles: Vec<&str> = rows.iter().map(|r| r.entry.title.as_str()).collect();
    assert_eq!(titles, ["One", "Two", "Three"]);
    assert_eq!(rows[0].status, Some(LogStatus::Aired));
    assert!(rows[0].end.is_some_and(|end| end > rows[1].entry.start));
}
