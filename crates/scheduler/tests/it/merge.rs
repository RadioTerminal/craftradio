use jiff::Timestamp;
use jiff::civil::date;
use radio_model::{Constraint, Placement, PlaylistId, Rule, Station, Trigger};
use radio_scheduler::{Entry, PlanInput, PlayHistory, Timeline, Warning, merge};
use radio_testkit::{jingle, playlist, station, track};
use std::time::Duration;

const MAIN: PlaylistId = PlaylistId(1);
const ADS: PlaylistId = PlaylistId(2);

fn tracks(n: u64) -> Vec<radio_model::Item> {
    (1..=n)
        .map(|i| track(i, &format!("track-{i}"), &format!("artist-{i}"), 180))
        .collect()
}

fn jingles(first_id: u64, n: u64) -> Vec<radio_model::Item> {
    (0..n).map(|i| jingle(first_id + i, 20)).collect()
}

fn rule(trigger: Trigger, take: u8) -> Rule {
    Rule {
        trigger,
        placement: Placement::BetweenTracks,
        take,
        constraints: Vec::new(),
    }
}

fn station_with(ads_items: Vec<radio_model::Item>, rules: Vec<Rule>) -> Station {
    let mut main = playlist(1, "main", tracks(9));
    main.is_main = true;
    let mut ads = playlist(2, "ads", ads_items);
    ads.rules = rules;
    station("Europe/Ljubljana", vec![main, ads])
}

fn run(station: &Station, now: Timestamp, horizon: Duration) -> Timeline {
    let history = PlayHistory::new();
    merge(&PlanInput {
        station,
        history: &history,
        now,
        horizon,
        pinned: &[],
    })
}

fn at_14(minute: i8) -> Timestamp {
    date(2026, 11, 21)
        .at(14, minute, 0, 0)
        .in_tz("Europe/Ljubljana")
        .unwrap()
        .timestamp()
}

fn playlists(entries: &[Entry]) -> Vec<PlaylistId> {
    entries.iter().map(|e| e.playlist).collect()
}

#[test]
fn station_id_every_3_tracks() {
    let st = station_with(jingles(100, 1), vec![rule(Trigger::EveryNTracks(3), 1)]);
    let tl = run(&st, at_14(0), Duration::from_secs(2 * 3600));
    let (m, j) = (MAIN, ADS);
    assert_eq!(
        playlists(&tl.entries[..12]),
        [m, m, m, j, m, m, m, j, m, m, m, j]
    );
}

#[test]
fn ads_at_20_and_50_land_after_track_boundary() {
    let st = station_with(
        jingles(100, 3),
        vec![rule(Trigger::AtMinutes(vec![20, 50]), 1)],
    );
    let tl = run(&st, at_14(0), Duration::from_secs(3600));
    let ads: Vec<usize> = (0..tl.entries.len())
        .filter(|&i| tl.entries[i].playlist == ADS)
        .collect();
    assert_eq!(ads.len(), 2);
    for (i, due) in ads.into_iter().zip([at_14(20), at_14(50)]) {
        assert!(tl.entries[i].start >= due, "ad starts before its minute");
        assert!(
            tl.entries[i - 1].start < due,
            "ad is not at the first boundary"
        );
        assert_eq!(tl.entries[i - 1].playlist, MAIN);
    }
}

#[test]
fn take_3_inserts_three_ads() {
    let st = station_with(jingles(100, 5), vec![rule(Trigger::EveryNTracks(2), 3)]);
    let tl = run(&st, at_14(0), Duration::from_secs(3600));
    let (m, j) = (MAIN, ADS);
    assert_eq!(playlists(&tl.entries[..7]), [m, m, j, j, j, m, m]);
}

#[test]
fn not_adjacent_to_self_never_two_ads() {
    let mut st = station_with(
        jingles(100, 5),
        vec![
            rule(Trigger::EveryNTracks(2), 1),
            rule(Trigger::EveryNTracks(2), 1),
        ],
    );
    st.playlists[1].constraints = vec![Constraint::NotAdjacentToSelf];
    let tl = run(&st, at_14(0), Duration::from_secs(3600));
    assert!(tl.entries.iter().any(|e| e.playlist == ADS));
    assert!(
        tl.entries
            .windows(2)
            .all(|w| !(w[0].playlist == ADS && w[1].playlist == ADS))
    );
}

#[test]
fn empty_playlist_rule_warns_and_continues() {
    let st = station_with(Vec::new(), vec![rule(Trigger::EveryNTracks(2), 1)]);
    let tl = run(&st, at_14(0), Duration::from_secs(3600));
    assert!(
        tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::NoEligibleItem { playlist, .. } if *playlist == ADS))
    );
    assert!(tl.entries.len() > 5);
    assert!(tl.entries.iter().all(|e| e.playlist == MAIN));
}

#[test]
fn fixed_entry_mid_track_keeps_main_walk_monotonic() {
    use radio_model::{FixMode, Repeat};
    let fixed_rule = Rule {
        trigger: Trigger::At {
            at: date(2026, 11, 21).at(15, 0, 0, 0),
            repeat: Repeat::Once,
        },
        placement: Placement::Fixed {
            mode: FixMode::Soft,
            tolerance: Duration::from_secs(60),
        },
        take: 1,
        constraints: Vec::new(),
    };
    let st = station_with(jingles(100, 1), vec![fixed_rule]);
    let tl = run(&st, at_14(0), Duration::from_secs(2 * 3600));
    let target = date(2026, 11, 21)
        .at(15, 0, 0, 0)
        .in_tz("Europe/Ljubljana")
        .unwrap()
        .timestamp();
    let fixed_at = tl.entries.iter().position(|e| e.fixed.is_some()).unwrap();
    assert_eq!(tl.entries[fixed_at].start, target);
    // The fixed time falls inside the preceding track.
    assert!(tl.entries[fixed_at - 1].start < target);
    let main: Vec<_> = tl.entries.iter().filter(|e| e.playlist == MAIN).collect();
    // Each track's segue point is 2 s before its end, so the next may start 178 s after.
    assert!(
        main.windows(2)
            .all(|w| w[1].start >= w[0].start + Duration::from_secs(178))
    );
    assert!(tl.entries.windows(2).all(|w| w[0].start <= w[1].start));
}

/// The first ads of a fixed break are on decks already (pinned). The break still has
/// its `take` ads: the pinned ones are not picked again, and the rest follow them.
#[test]
fn pinned_ads_of_a_fixed_break_keep_the_rest_of_the_break() {
    use radio_model::{FixMode, Repeat};
    let break_rule = Rule {
        trigger: Trigger::At {
            at: date(2026, 11, 21).at(15, 0, 0, 0),
            repeat: Repeat::Once,
        },
        placement: Placement::Fixed {
            mode: FixMode::Hard,
            tolerance: Duration::from_secs(30),
        },
        take: 3,
        constraints: Vec::new(),
    };
    let st = station_with(jingles(100, 3), vec![break_rule]);
    let now = at_14(55);
    let horizon = Duration::from_secs(3600);
    let first = run(&st, now, horizon);
    let planned: Vec<Entry> = first
        .entries
        .iter()
        .filter(|e| e.playlist == ADS)
        .cloned()
        .collect();
    assert_eq!(planned.len(), 3);
    for on_decks in 1..=2 {
        let pinned = &planned[..on_decks];
        let history = PlayHistory::new();
        let tl = merge(&PlanInput {
            station: &st,
            history: &history,
            now,
            horizon,
            pinned,
        });
        let ads: Vec<_> = tl.entries.iter().filter(|e| e.playlist == ADS).collect();
        assert_eq!(ads.len(), 3, "{on_decks} pinned, the break has: {ads:?}");
        assert_eq!(ads.iter().filter(|e| e.fixed.is_some()).count(), 1);
        assert!(
            pinned.iter().all(|p| ads.contains(&p)),
            "the pinned ads are kept as given"
        );
        let last_pinned = pinned.iter().map(|p| p.start).max().unwrap();
        assert!(
            ads.iter()
                .filter(|e| !pinned.contains(e))
                .all(|e| e.start > last_pinned),
            "the rest of the break follows the pinned ads"
        );
    }
}
