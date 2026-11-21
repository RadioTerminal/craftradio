use jiff::Timestamp;
use radio_model::{FixMode, Placement, Repeat, Rule, Trigger};
use radio_scheduler::{PlanInput, PlanOptions, PlayHistory, plan};
use radio_testkit::{jingle, playlist, station, track};
use std::time::{Duration, Instant};

fn measure(seconds: u64) -> Duration {
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
    let start = Instant::now();
    let timeline = plan(&input, &PlanOptions::default(), None);
    let elapsed = start.elapsed();
    eprintln!(
        "plan {seconds}s: {elapsed:?}, {} entries, {} fits",
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
    assert!(measure(1800) < Duration::from_secs(2));
}

#[test]
#[ignore = "24 hour performance benchmark; run with --release --ignored"]
fn short_items_full_day() {
    let elapsed = measure(86400);
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
