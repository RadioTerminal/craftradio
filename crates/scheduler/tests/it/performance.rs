use jiff::Timestamp;
use radio_model::{FixMode, Placement, Repeat, Rule, Trigger};
use radio_scheduler::{PlanInput, PlanOptions, PlayHistory, plan};
use radio_testkit::{jingle, playlist, station, track};
use std::time::{Duration, Instant};

fn measure(seconds: u64, replan: bool) -> Duration {
    let now: Timestamp = "2026-11-21T00:00:00Z".parse().unwrap();
    let mut main = playlist(
        1,
        "main",
        (1..=30)
            .map(|id| {
                let mut item = track(id, &format!("piece-{id}"), &format!("voice-{id}"), 1);
                item.timing.as_mut().unwrap().segue = Duration::from_secs(1);
                item
            })
            .collect(),
    );
    main.is_main = true;
    let mut jingles = playlist(2, "jingles", vec![jingle(100, 1)]);
    jingles.rules.push(Rule {
        trigger: Trigger::EveryNTracks(3),
        placement: Placement::BetweenTracks,
        take: 1,
        constraints: vec![],
    });
    let mut events = playlist(3, "events", vec![jingle(101, 1)]);
    for hour in 0..24 {
        for minute in [15, 45] {
            events.rules.push(Rule {
                trigger: Trigger::At {
                    at: jiff::civil::date(2026, 11, 21).at(hour, minute, 0, 0),
                    repeat: Repeat::Once,
                },
                placement: Placement::Fixed {
                    mode: FixMode::Hard,
                    tolerance: Duration::ZERO,
                },
                take: 1,
                constraints: vec![],
            });
        }
    }
    let station = station("UTC", vec![main, jingles, events]);
    let history = PlayHistory::new();
    let input = PlanInput {
        station: &station,
        history: &history,
        now,
        horizon: Duration::from_secs(seconds),
        pinned: &[],
    };
    let opts = PlanOptions::default();
    let prev = replan.then(|| {
        plan(
            &PlanInput {
                now: now - Duration::from_secs(1),
                ..input
            },
            &opts,
            None,
        )
    });
    let start = Instant::now();
    let timeline = plan(&input, &opts, prev.as_ref());
    let elapsed = start.elapsed();
    eprintln!(
        "replan={replan} plan {seconds}s: {elapsed:?}, {} entries, {} fits",
        timeline.entries.len(),
        timeline.fits.len()
    );
    assert!(timeline.entries.len() >= seconds as usize - 48);
    assert!(
        timeline
            .entries
            .windows(2)
            .all(|w| w[0].start <= w[1].start)
    );
    assert!(timeline.fits.iter().all(|f| f.within_tolerance));
    elapsed
}

#[test]
fn short_items_small_horizon() {
    assert!(measure(1800, false) < Duration::from_secs(2));
}

#[test]
#[ignore = "24 hour performance benchmark; run with --release --ignored"]
fn short_items_full_day() {
    let elapsed = measure(86400, false);
    let budget = if cfg!(debug_assertions) { 10 } else { 2 };
    assert!(
        elapsed < Duration::from_secs(budget),
        "{elapsed:?} exceeds {budget}s"
    );
}

#[test]
fn subset_search_keeps_distance_and_displacement_priority() {
    // Compare compressed search with all subsets, including zero lengths,
    // mandatory entries, and targets between reachable cells.
    for lengths in [[0, 10, 20, 30], [10, 10, 10, 10], [6, 12, 18, 24]] {
        for gap in 0..65 {
            let cands: Vec<_> = lengths
                .iter()
                .enumerate()
                .map(|(i, &ticks)| (Duration::from_millis(ticks * 100), i != 0, i < 2))
                .collect();
            let (kept, _) =
                radio_scheduler::select_before(Duration::from_millis(gap * 100), &cands);
            let score = |mask: usize| {
                let sum: u64 = lengths
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, n)| n)
                    .sum();
                let cost = (1..4)
                    .filter(|&i| (mask & (1 << i) != 0) != (i < 2))
                    .count();
                (gap.abs_diff(sum), cost, sum)
            };
            let mask = kept.iter().fold(0, |mask, i| mask | (1 << i));
            let best = (0..16)
                .filter(|mask| mask & 1 != 0)
                .map(score)
                .min()
                .unwrap();
            assert_eq!(score(mask), best);
        }
    }
}

#[test]
fn history_indexes_keep_out_of_order_plays_and_cutoffs() {
    use radio_model::{Constraint, ItemId, PlaylistId};
    use radio_scheduler::{Played, allowed};
    let now: Timestamp = "2026-11-21T12:00:00Z".parse().unwrap();
    let item = track(1, "piece", "voice", 1);
    let mut history = PlayHistory::new();
    for age in [0, 20, 10] {
        history.push(Played {
            item: item.id,
            playlist: PlaylistId(1),
            at: now - Duration::from_secs(age),
            artist: item.artist.clone(),
            title: item.title.clone(),
        });
    }
    assert_eq!(history.last_played(ItemId(1)), Some(now));
    let tz = jiff::tz::TimeZone::UTC;
    let at = now + Duration::from_secs(10);
    for constraint in [
        Constraint::ArtistSeparation(Duration::from_secs(10)),
        Constraint::TitleSeparation(Duration::from_secs(10)),
    ] {
        assert!(allowed(
            &constraint,
            &item,
            PlaylistId(1),
            at,
            None,
            &history,
            &tz
        ));
        assert!(!allowed(
            &constraint,
            &item,
            PlaylistId(1),
            now,
            None,
            &history,
            &tz
        ));
    }
    assert!(!allowed(
        &Constraint::MaxPerHour(3),
        &item,
        PlaylistId(1),
        at,
        None,
        &history,
        &tz
    ));
    assert!(allowed(
        &Constraint::MaxPerHour(3),
        &item,
        PlaylistId(2),
        at,
        None,
        &history,
        &tz
    ));
}

#[test]
#[ignore = "24 hour replan benchmark; run with --release --ignored"]
fn short_items_full_day_replan() {
    let elapsed = measure(86400, true);
    let budget = if cfg!(debug_assertions) {
        Duration::from_secs(10)
    } else {
        Duration::from_millis(150)
    };
    assert!(elapsed < budget, "{elapsed:?} exceeds {budget:?}");
}

#[test]
fn daily_caps_use_local_midnight_and_exclude_later_days() {
    use radio_model::{Constraint, PlaylistId};
    use radio_scheduler::{Played, allowed};
    let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").unwrap();
    // Ordinary, 23-hour and 25-hour local days.
    for (midnight, following) in [
        ("2026-11-20T23:00:00Z", "2026-11-21T23:00:00Z"),
        ("2026-03-28T23:00:00Z", "2026-03-29T22:00:00Z"),
        ("2026-10-24T22:00:00Z", "2026-10-25T23:00:00Z"),
    ] {
        let at: Timestamp = midnight.parse().unwrap();
        let next: Timestamp = following.parse().unwrap();
        let mut item = track(1, "day-piece", "day-voice", 1);
        item.timing.as_mut().unwrap().segue = Duration::from_secs(1);
        item.max_per_day = Some(1);
        let mut history = PlayHistory::new();
        let played = |at| Played {
            item: item.id,
            playlist: PlaylistId(1),
            at,
            artist: item.artist.clone(),
            title: item.title.clone(),
        };
        // Future days must not count, even when the history was supplied out of order.
        history.push(played(next));
        history.push(played(at - Duration::from_secs(1)));
        assert!(!allowed(
            &Constraint::MaxPerDay(1),
            &item,
            PlaylistId(1),
            at - Duration::from_secs(1),
            None,
            &history,
            &tz
        ));
        assert!(allowed(
            &Constraint::MaxPerDay(1),
            &item,
            PlaylistId(1),
            at,
            None,
            &history,
            &tz
        ));
        let mut main = playlist(1, "day-main", vec![item.clone()]);
        main.is_main = true;
        let st = station("Europe/Ljubljana", vec![main]);
        let input = PlanInput {
            station: &st,
            history: &history,
            now: at,
            horizon: Duration::from_secs(3),
            pinned: &[],
        };
        let fresh = plan(&input, &PlanOptions::default(), None);
        assert_eq!(fresh.entries.len(), 1);
        history.push(played(at));
        assert!(!allowed(
            &Constraint::MaxPerDay(1),
            &item,
            PlaylistId(1),
            at,
            None,
            &history,
            &tz
        ));
        assert!(allowed(
            &Constraint::MaxPerDay(2),
            &item,
            PlaylistId(1),
            at,
            None,
            &history,
            &tz
        ));
        let capped = plan(
            &PlanInput {
                station: &st,
                history: &history,
                now: at,
                horizon: Duration::from_secs(3),
                pinned: &[],
            },
            &PlanOptions::default(),
            None,
        );
        assert!(capped.entries.is_empty());
    }
}

#[test]
fn equal_length_search_keeps_the_dp_tie_order() {
    use radio_scheduler::select_before;
    for before in 0_u32..64 {
        for movable in [0_u32, 21, 42, 63] {
            let cands: Vec<_> = (0..6)
                .map(|i| {
                    (
                        Duration::from_secs(1),
                        movable & (1 << i) != 0,
                        before & (1 << i) != 0,
                    )
                })
                .collect();
            for half_seconds in 0..15_u64 {
                let gap = Duration::from_millis(half_seconds * 500);
                let expected = (0_u32..64)
                    .filter(|mask| {
                        (0..6).all(|i| {
                            movable & (1 << i) != 0
                                || (mask & (1 << i) != 0) == (before & (1 << i) != 0)
                        })
                    })
                    .min_by_key(|mask| {
                        let millis = u64::from(mask.count_ones()) * 1000;
                        (
                            gap.as_millis().abs_diff(u128::from(millis)),
                            (mask ^ before).count_ones(),
                            millis,
                            *mask,
                        )
                    })
                    .unwrap();
                let (picked, _) = select_before(gap, &cands);
                let actual = picked.iter().fold(0_u32, |mask, i| mask | (1 << i));
                assert_eq!(
                    actual, expected,
                    "before={before}, movable={movable}, gap={gap:?}"
                );
            }
        }
    }
}
