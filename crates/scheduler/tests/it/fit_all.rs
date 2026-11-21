use jiff::{SignedDuration, Timestamp};
use radio_model::{FixMode, Item, ItemId, Station};
use radio_scheduler::{
    Entry, FitResult, FitStep, Fixed, MAX_GAP, Origin, PlayHistory, Timeline, Warning, fit_all,
    select_before,
};
use radio_testkit::{jingle, playlist, station, track};
use radio_timing::{item_len, start_times};
use std::time::Duration;

const TOL: Duration = Duration::from_secs(1);

fn t0() -> Timestamp {
    "2026-11-21T13:00:00Z".parse().unwrap()
}

fn at(secs: u64) -> Timestamp {
    t0() + Duration::from_secs(secs)
}

fn tracks(first: u64, n: u64, secs: u32) -> Vec<Item> {
    (first..first + n)
        .map(|i| track(i, &format!("track-{i}"), &format!("artist-{i}"), secs))
        .collect()
}

/// A station of a main playlist (1), event playlists (2, 3) and an optional fill pool (9).
fn make_station(main: Vec<Item>, events: Vec<(Item, i32)>, fill: Vec<Item>) -> Station {
    let mut pls = vec![playlist(1, "main", main)];
    pls[0].is_main = true;
    for (k, (item, priority)) in events.into_iter().enumerate() {
        let mut pl = playlist(2 + k as u64, &format!("event-{k}"), vec![item]);
        pl.priority = priority;
        pls.push(pl);
    }
    if !fill.is_empty() {
        let mut pool = playlist(9, "fill", fill);
        pool.is_fill_pool = true;
        pls.push(pool);
    }
    station("UTC", pls)
}

fn find(st: &Station, id: ItemId) -> (&Item, radio_model::PlaylistId) {
    st.playlists
        .iter()
        .find_map(|p| p.items.iter().find(|i| i.id == id).map(|i| (i, p.id)))
        .unwrap()
}

/// The main playlist's items back to back from `t0`, plus one fixed entry per event playlist.
fn timeline(st: &Station, events: &[(Timestamp, FixMode)]) -> Timeline {
    let main: Vec<&Item> = st.playlists[0].items.iter().collect();
    let mut entries: Vec<Entry> = start_times(t0(), &main)
        .into_iter()
        .zip(&main)
        .map(|(start, item)| Entry {
            playlist: st.playlists[0].id,
            item: item.id,
            start,
            len: item_len(item),
            origin: Origin::Main,
            locked: item.locked,
            fixed: None,
            trim: Duration::ZERO,
        })
        .collect();
    for (k, (target, mode)) in events.iter().enumerate() {
        let pl = &st.playlists[1 + k];
        entries.push(Entry {
            playlist: pl.id,
            item: pl.items[0].id,
            start: *target,
            len: item_len(&pl.items[0]),
            origin: Origin::Rule {
                playlist: pl.id,
                rule: 0,
            },
            locked: false,
            fixed: Some(Fixed {
                target: *target,
                mode: *mode,
                tolerance: TOL,
                priority: pl.priority,
            }),
            trim: Duration::ZERO,
        });
    }
    entries.sort_by_key(|e| e.start);
    Timeline {
        entries,
        fits: Vec::new(),
        warnings: Vec::new(),
    }
}

fn fit(st: &Station, tl: &mut Timeline) {
    fit_all(tl, st, &PlayHistory::new());
}

fn event_index(tl: &Timeline, item: u64) -> usize {
    tl.entries
        .iter()
        .position(|e| e.item == ItemId(item))
        .unwrap()
}

fn fit_for(tl: &Timeline, target: Timestamp) -> &FitResult {
    tl.fits.iter().find(|f| f.target == target).unwrap()
}

/// Where an entry stops sounding: its start plus its length, less any early fade.
fn sound_end(st: &Station, e: &Entry) -> Timestamp {
    e.start + item_len(find(st, e.item).0) - e.trim
}

fn no_overlap_with_fixed(st: &Station, tl: &Timeline) {
    for w in tl.entries.windows(2) {
        if w[1].fixed.is_some() {
            let item = find(st, w[0].item).0;
            let hard = w[1].fixed.as_ref().unwrap().mode == FixMode::Hard;
            let reach = if hard {
                sound_end(st, &w[0])
            } else {
                w[0].start + radio_timing::advance(item, Some(item)) - w[0].trim
            };
            assert!(reach <= w[1].start, "{:?} runs into a fixed entry", w[0]);
        }
    }
}

#[test]
fn fill_closes_small_gap() {
    // Tracks advance 178 s each; three of them leave a 7 s gap before the event.
    let fill = vec![jingle(201, 6), jingle(202, 3), jingle(203, 4)];
    let st = make_station(tracks(1, 12, 180), vec![(jingle(100, 60), 0)], fill);
    let target = at(3 * 178 + 7);
    let mut tl = timeline(&st, &[(target, FixMode::Soft)]);
    fit(&st, &mut tl);
    let result = fit_for(&tl, target);
    let filled = result
        .steps
        .iter()
        .find_map(|s| match s {
            FitStep::Filled { items } => Some(items.clone()),
            _ => None,
        })
        .expect("a Filled step");
    let total: Duration = filled.iter().map(|id| item_len(find(&st, *id).0)).sum();
    assert_eq!(total, Duration::from_secs(7));
    assert_eq!(result.error, SignedDuration::ZERO);
    assert!(result.within_tolerance);
    assert_eq!(tl.entries[event_index(&tl, 100)].start, target);
    no_overlap_with_fixed(&st, &tl);
}

#[test]
fn trim_last_track_when_late_hard() {
    // Four tracks end 10 s after the target; the fourth plays to its end before a hard event.
    let st = make_station(tracks(1, 12, 180), vec![(jingle(100, 60), 0)], Vec::new());
    let target = at(3 * 178 + 170);
    let mut tl = timeline(&st, &[(target, FixMode::Hard)]);
    fit(&st, &mut tl);
    let result = fit_for(&tl, target);
    assert!(
        result
            .steps
            .iter()
            .any(|s| matches!(s, FitStep::Trimmed { .. }))
    );
    assert!(result.within_tolerance);
    let ev = event_index(&tl, 100);
    assert_eq!(tl.entries[ev].start, target);
    let prev = &tl.entries[ev - 1];
    assert!(prev.trim > Duration::ZERO);
    assert_eq!(
        sound_end(&st, prev),
        target,
        "the fade finishes at the target"
    );
    no_overlap_with_fixed(&st, &tl);
}

#[test]
fn ad_is_never_trimmed() {
    for mode in [FixMode::Soft, FixMode::Hard] {
        // Only ads (neither movable nor trimmable) before the event; the first runs 10 s over.
        let ads = (1..=6).map(|i| jingle(i, 200)).collect();
        let st = make_station(ads, vec![(jingle(100, 60), 0)], Vec::new());
        let target = at(190);
        let mut tl = timeline(&st, &[(target, mode)]);
        fit(&st, &mut tl);
        assert!(tl.entries.iter().all(|e| e.trim == Duration::ZERO));
        let result = fit_for(&tl, target);
        assert!(
            !result
                .steps
                .iter()
                .any(|s| matches!(s, FitStep::Trimmed { .. }))
        );
        assert!(!result.within_tolerance);
        assert!(result.error.is_positive());
        match mode {
            FixMode::Soft => assert!(
                result
                    .steps
                    .iter()
                    .any(|s| matches!(s, FitStep::LateStart { .. }))
            ),
            FixMode::Hard => assert!(
                tl.warnings
                    .iter()
                    .any(|w| matches!(w, Warning::OutsideTolerance { .. }))
            ),
        }
        let ev = event_index(&tl, 100);
        assert!(tl.entries[ev].start >= sound_end(&st, &tl.entries[ev - 1]));
    }
}

#[test]
fn dead_air_warned_when_material_short() {
    // One track and a 5 s bed cannot reach a hard event ten minutes out.
    let st = make_station(
        tracks(1, 1, 180),
        vec![(jingle(100, 60), 0)],
        vec![jingle(201, 5)],
    );
    let target = at(600);
    let mut tl = timeline(&st, &[(target, FixMode::Hard)]);
    fit(&st, &mut tl);
    let result = fit_for(&tl, target);
    assert!(!result.within_tolerance);
    let ev = event_index(&tl, 100);
    assert_eq!(tl.entries[ev].start, target, "a hard event keeps its time");
    let material_end = sound_end(&st, &tl.entries[ev - 1]);
    let expected = target.duration_since(material_end).unsigned_abs();
    assert!(
        tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::DeadAir { gap, .. } if *gap == expected)),
        "{:?}",
        tl.warnings
    );
    assert!(
        tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::OutsideTolerance { .. }))
    );
}

#[test]
fn conflicting_fixed_events_priority_wins() {
    // A low-priority 60 s event 30 s before a higher-priority one overlaps it. Four tracks
    // reach the higher one's time with a short trim.
    let events = vec![(jingle(100, 60), 1), (jingle(101, 60), 5)];
    let st = make_station(tracks(1, 12, 180), events, Vec::new());
    let high_at = at(3 * 178 + 170);
    let low_at = high_at - Duration::from_secs(30);
    let mut tl = timeline(&st, &[(low_at, FixMode::Soft), (high_at, FixMode::Hard)]);
    fit(&st, &mut tl);
    let (low, high) = (
        &tl.entries[event_index(&tl, 100)],
        &tl.entries[event_index(&tl, 101)],
    );
    assert_eq!(high.start, high_at, "the higher priority keeps its time");
    assert!(low.start >= high.start + item_len(find(&st, high.item).0));
    assert!(tl.warnings.iter().any(|w| matches!(
        w,
        Warning::FixedConflict { playlist, wanted, got }
            if *playlist == low.playlist && *wanted == low_at && *got == low.start
    )));
    assert!(!tl.warnings.iter().any(
        |w| matches!(w, Warning::FixedConflict { playlist, .. } if *playlist == high.playlist)
    ));
    no_overlap_with_fixed(&st, &tl);
}

#[test]
fn select_before_search_is_bounded_on_huge_gap() {
    // 40 quarter-hour candidates (10 h) against a 48 h gap.
    let cands = vec![(Duration::from_secs(900), true, true); 40];
    let gap = Duration::from_secs(48 * 3600);
    let (kept, leftover) = select_before(gap, &cands);
    let played: Duration = kept.iter().map(|&i| cands[i].0).sum();
    let bound = gap - MAX_GAP - Duration::from_secs(600);
    assert!(leftover.is_positive());
    assert_eq!(
        leftover,
        SignedDuration::try_from(gap - played).unwrap(),
        "leftover is the real remainder"
    );
    assert!(leftover >= SignedDuration::try_from(bound).unwrap());
}
