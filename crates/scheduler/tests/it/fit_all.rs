use jiff::{SignedDuration, Timestamp};
use radio_model::{Constraint, FixMode, Item, ItemId, Station};
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
                frozen: false,
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

#[test]
fn soft_never_starts_earlier_than_tolerance() {
    // One track cannot reach a soft event ten minutes out, and there is no fill.
    let st = make_station(tracks(1, 1, 180), vec![(jingle(100, 60), 0)], Vec::new());
    let target = at(600);
    let mut tl = timeline(&st, &[(target, FixMode::Soft)]);
    fit(&st, &mut tl);
    let ev = event_index(&tl, 100);
    let start = tl.entries[ev].start;
    assert!(
        start >= target - TOL,
        "starts no earlier than target − tolerance"
    );
    let result = fit_for(&tl, target);
    assert!(!result.within_tolerance);
    let early = result.steps.iter().find_map(|s| match s {
        FitStep::EarlyStart { by } => Some(*by),
        _ => None,
    });
    assert!(early.is_some_and(|by| by <= TOL));
    let prev = &tl.entries[ev - 1];
    let item = find(&st, prev.item).0;
    let handover = prev.start + radio_timing::advance(item, Some(item)) - prev.trim;
    let silence = start.duration_since(handover).unsigned_abs();
    assert!(
        tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::DeadAir { gap, .. } if *gap == silence)),
        "{:?}",
        tl.warnings
    );
}

#[test]
fn reorder_respects_artist_separation() {
    // t1, t2, t5 run 80 s late. The exact fit swaps t5 for t3 (artist "a", like t1);
    // the separation rule leaves t4 (10 s late, then trimmed) instead.
    let mut main = vec![
        track(1, "t1", "a", 180),
        track(2, "t2", "b", 180),
        track(5, "t5", "d", 180),
        track(3, "t3", "a", 100),
        track(4, "t4", "c", 110),
    ];
    main.extend(tracks(6, 7, 180));
    let mut st = make_station(main, vec![(jingle(100, 60), 0)], Vec::new());
    let separation = Duration::from_secs(3600);
    st.playlists[0].constraints = vec![Constraint::ArtistSeparation(separation)];
    let target = at(178 + 178 + 98);
    let mut tl = timeline(&st, &[(target, FixMode::Soft)]);
    fit(&st, &mut tl);
    let ev = event_index(&tl, 100);
    let before = &tl.entries[..ev];
    for (i, a) in before.iter().enumerate() {
        for b in &before[i + 1..] {
            let (x, y) = (find(&st, a.item).0, find(&st, b.item).0);
            assert!(
                x.artist != y.artist
                    || b.start.duration_since(a.start).unsigned_abs() >= separation,
                "{} and {} share an artist within the separation",
                x.title,
                y.title
            );
        }
    }
}

#[test]
fn locked_entry_keeps_its_start() {
    // Track 3 is locked 244 s after it would naturally start; a hard event follows later.
    let st = make_station(tracks(1, 12, 180), vec![(jingle(100, 60), 0)], Vec::new());
    let mut tl = timeline(&st, &[(at(1400), FixMode::Hard)]);
    let shift = Duration::from_secs(244);
    for e in tl.entries.iter_mut().filter(|e| e.fixed.is_none()) {
        if e.item.0 >= 3 {
            e.start += shift;
        }
        e.locked = e.item == ItemId(3);
    }
    let locked_at = tl.entries.iter().find(|e| e.locked).unwrap().start;
    fit(&st, &mut tl);
    let locked = tl.entries.iter().position(|e| e.locked).unwrap();
    assert_eq!(tl.entries[locked].start, locked_at);
    let prev = &tl.entries[locked - 1];
    let item = find(&st, prev.item).0;
    let handover = prev.start + radio_timing::advance(item, Some(item)) - prev.trim;
    assert!(
        tl.warnings.iter().any(|w| matches!(
            w,
            Warning::DeadAir { at, gap } if *at == handover && *at + *gap == locked_at
        )),
        "the silence before the locked entry is reported: {:?}",
        tl.warnings
    );
}

#[test]
fn hard_late_fades_on_air_item_to_end_at_target() {
    // Ten-minute tracks: the first runs 50 s past a hard event, more than the soft cap.
    let st = make_station(tracks(1, 6, 600), vec![(jingle(100, 60), 0)], Vec::new());
    let target = at(550);
    let mut tl = timeline(&st, &[(target, FixMode::Hard)]);
    fit(&st, &mut tl);
    let ev = event_index(&tl, 100);
    assert_eq!(tl.entries[ev].start, target);
    let prev = &tl.entries[ev - 1];
    assert!(prev.trim > Duration::from_secs(30));
    assert_eq!(
        sound_end(&st, prev),
        target,
        "the fade finishes at the target"
    );
    assert!(fit_for(&tl, target).within_tolerance);
}

#[test]
fn first_event_keeps_its_floor_when_everything_before_it_is_dropped() {
    for mode in [FixMode::Soft, FixMode::Hard] {
        // 30 s early with nothing before the event beats a 180 s track running 150 s over.
        let st = make_station(tracks(1, 6, 180), vec![(jingle(100, 60), 0)], Vec::new());
        let target = at(30);
        let mut tl = timeline(&st, &[(target, mode)]);
        fit(&st, &mut tl);
        let start = tl.entries[event_index(&tl, 100)].start;
        match mode {
            FixMode::Hard => assert_eq!(start, target),
            FixMode::Soft => assert!(start >= target - TOL),
        }
        assert_eq!(
            tl.entries[0].item,
            ItemId(100),
            "the track went after the event"
        );
        assert!(tl.warnings.iter().any(|w| matches!(
            w,
            Warning::DeadAir { at, gap } if *at == t0() && *at + *gap == start
        )));
    }
}

#[test]
fn locked_entry_keeps_start_when_material_is_displaced_before_it() {
    // A hard event 60 s in drops track 1, which then queues in front of locked track 3.
    let st = make_station(tracks(1, 8, 180), vec![(jingle(100, 60), 0)], Vec::new());
    let mut tl = timeline(&st, &[(at(60), FixMode::Hard)]);
    for e in tl.entries.iter_mut() {
        e.locked = e.item == ItemId(3);
    }
    let locked_at = tl.entries.iter().find(|e| e.locked).unwrap().start;
    fit(&st, &mut tl);
    let locked = tl.entries.iter().position(|e| e.locked).unwrap();
    assert_eq!(tl.entries[locked].start, locked_at);
    let prev = &tl.entries[locked - 1];
    let item = find(&st, prev.item).0;
    let handover = prev.start + radio_timing::advance(item, Some(item)) - prev.trim;
    assert!(handover <= locked_at, "nothing runs into the locked entry");
    assert!(
        !tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::LockedMoved { .. }))
    );
}

#[test]
fn hard_event_beats_locked_entry() {
    // A 60 s hard event at 340 s overlaps locked track 3 at 356 s.
    let st = make_station(tracks(1, 8, 180), vec![(jingle(100, 60), 0)], Vec::new());
    let target = at(340);
    let mut tl = timeline(&st, &[(target, FixMode::Hard)]);
    for e in tl.entries.iter_mut() {
        e.locked = e.item == ItemId(3);
    }
    let locked_at = tl.entries.iter().find(|e| e.locked).unwrap().start;
    fit(&st, &mut tl);
    let hard = &tl.entries[event_index(&tl, 100)];
    assert_eq!(hard.start, target, "the hard event keeps its time");
    let locked = tl.entries.iter().find(|e| e.locked).unwrap();
    assert!(locked.start >= hard.start + item_len(find(&st, hard.item).0));
    assert!(tl.warnings.iter().any(|w| matches!(
        w,
        Warning::LockedMoved { item, by }
            if *item == ItemId(3) && locked_at + *by == locked.start
    )));
}

/// Three 180 s tracks hand over at +534 s; before a hard event the last one plays to +536 s.
fn hard_fill_case(fill: Vec<Item>) -> (Station, Timeline, Timestamp) {
    let st = make_station(tracks(1, 12, 180), vec![(jingle(100, 60), 0)], fill);
    let target = at(546);
    let tl = timeline(&st, &[(target, FixMode::Hard)]);
    (st, tl, target)
}

fn filled_items(result: &FitResult) -> Vec<ItemId> {
    result
        .steps
        .iter()
        .find_map(|s| match s {
            FitStep::Filled { items } => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn fill_before_hard_event_starts_at_the_segue_of_the_last_track() {
    // Followed by fill, the last track hands over at its segue (+534 s), not at its end
    // (+536 s), so the 12 s item closes the gap and the 10 s one would leave dead air.
    let (st, mut tl, target) = hard_fill_case(vec![jingle(201, 10), jingle(202, 12)]);
    fit(&st, &mut tl);
    let result = fit_for(&tl, target);
    assert_eq!(filled_items(result), vec![ItemId(202)]);
    assert!(result.within_tolerance, "{result:?}");
    assert!(
        !tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::DeadAir { .. })),
        "{:?}",
        tl.warnings
    );
    assert_eq!(tl.entries[event_index(&tl, 100)].start, target);
    no_overlap_with_fixed(&st, &tl);
}

#[test]
fn hard_event_fill_prefers_a_fadeable_overrun_to_dead_air() {
    // The gap is 12 s. A 10 s bed ends 2 s early, which nothing can close; a 15 s bed runs
    // 3 s over and can be faded at the target.
    let bed = |id, secs| {
        let mut item = jingle(id, secs);
        item.trimmable = true;
        item
    };
    let (st, mut tl, target) = hard_fill_case(vec![bed(201, 10), bed(202, 15)]);
    fit(&st, &mut tl);
    let result = fit_for(&tl, target);
    assert_eq!(filled_items(result), vec![ItemId(202)]);
    assert_eq!(result.error, SignedDuration::ZERO, "{result:?}");
    assert!(
        result
            .steps
            .iter()
            .any(|s| matches!(s, FitStep::Trimmed { .. })),
        "{result:?}"
    );
    assert_eq!(tl.entries[event_index(&tl, 100)].start, target);
    no_overlap_with_fixed(&st, &tl);
}

fn fadeable_bed(id: u64, secs: u32) -> Item {
    let mut item = jingle(id, secs);
    item.trimmable = true;
    item
}

fn dead_air_before(tl: &Timeline, target: Timestamp) -> Duration {
    tl.warnings
        .iter()
        .filter_map(|w| match w {
            Warning::DeadAir { at, gap } if *at < target && *at + *gap >= target => Some(*gap),
            _ => None,
        })
        .sum()
}

#[test]
fn hard_event_fill_does_not_cut_a_long_bed_to_a_scrap() {
    // The third track plays to its end at +536 s before the hard event at +539 s. Only a
    // 60 s bed could fill those 3 s; faded at the target it would play a few seconds of 60,
    // so the gap stays dead air.
    let st = make_station(
        tracks(1, 12, 180),
        vec![(jingle(100, 60), 0)],
        vec![fadeable_bed(201, 60)],
    );
    let target = at(2 * 178 + 180 + 3);
    let mut tl = timeline(&st, &[(target, FixMode::Hard)]);
    fit(&st, &mut tl);
    assert!(
        filled_items(fit_for(&tl, target)).is_empty(),
        "{:?}",
        tl.fits
    );
    assert_eq!(dead_air_before(&tl, target), Duration::from_secs(3));
    assert_eq!(tl.entries[event_index(&tl, 100)].start, target);
}

#[test]
fn hard_event_fill_overrun_must_keep_half_the_bed() {
    // A 7 s gap. A 4 s bed ends 3 s early; a 15 s bed would run 8 s over and play only 7 s
    // of 15, under half, so the 4 s bed plays and 3 s of dead air is reported.
    let st = make_station(
        tracks(1, 12, 180),
        vec![(jingle(100, 60), 0)],
        vec![fadeable_bed(201, 4), fadeable_bed(202, 15)],
    );
    let target = at(3 * 178 + 7);
    let mut tl = timeline(&st, &[(target, FixMode::Hard)]);
    fit(&st, &mut tl);
    assert_eq!(filled_items(fit_for(&tl, target)), vec![ItemId(201)]);
    assert_eq!(dead_air_before(&tl, target), Duration::from_secs(3));
    assert_eq!(tl.entries[event_index(&tl, 100)].start, target);
}

#[test]
fn trim_rechecks_rotation_after_the_event() {
    let main = vec![
        track(1, "Opening", "Alpha", 60),
        track(2, "Moved", "Beta", 60),
        track(3, "Spacer", "Gamma", 60),
    ];
    let mut event = jingle(100, 10);
    event.artist = Some("Beta".into());
    let mut st = make_station(main, vec![(event, 0)], vec![]);
    st.playlists[0].constraints = vec![Constraint::ArtistSeparation(Duration::from_secs(30))];
    st.playlists[0].items[0].movable = false;
    let target = t0() + Duration::from_millis(58_500);
    let mut tl = timeline(&st, &[(target, FixMode::Hard)]);
    fit(&st, &mut tl);
    let event = tl.entries.iter().find(|e| e.item == ItemId(100)).unwrap();
    let moved = tl
        .entries
        .iter()
        .find(|e| e.item == ItemId(2))
        .expect("moved track stays in the plan");
    assert!(
        moved.start >= event.start + Duration::from_secs(30),
        "{:#?}",
        tl.entries
    );
}
