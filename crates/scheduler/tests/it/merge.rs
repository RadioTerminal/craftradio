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

#[test]
fn late_break_adopts_second_pinned_ad_before_picking_third() {
    use radio_model::{FixMode, Repeat};
    let rule = Rule {
        trigger: Trigger::At {
            at: date(2026, 11, 21).at(15, 0, 0, 0),
            repeat: Repeat::Once,
        },
        placement: Placement::Fixed {
            mode: FixMode::Hard,
            tolerance: Duration::from_secs(30),
        },
        take: 3,
        constraints: vec![],
    };
    let st = station_with(jingles(100, 3), vec![rule]);
    let now = at_14(55);
    let horizon = Duration::from_secs(3600);
    let first = run(&st, now, horizon);
    let mut pinned: Vec<_> = first
        .entries
        .iter()
        .filter(|e| e.playlist == ADS)
        .take(2)
        .cloned()
        .collect();
    // Keep the fixed target, but the second ad follows a first ad that started late.
    pinned[1].start += Duration::from_secs(90);
    let tl = merge(&PlanInput {
        station: &st,
        history: &PlayHistory::new(),
        now,
        horizon,
        pinned: &pinned,
    });
    let ads: Vec<_> = tl.entries.iter().filter(|e| e.playlist == ADS).collect();
    assert_eq!(ads.len(), 3);
    let third = ads
        .iter()
        .find(|e| !pinned.iter().any(|p| p.item == e.item))
        .expect("third distinct ad");
    assert!(third.start >= pinned[1].start + pinned[1].len, "{ads:?}");
}

fn ads_entries(tl: &Timeline) -> Vec<&Entry> {
    tl.entries.iter().filter(|e| e.playlist == ADS).collect()
}

#[test]
fn expired_item_not_picked() {
    let mut ads = jingles(100, 2);
    ads[0].valid_to = Some(date(2026, 11, 20));
    let st = station_with(ads, vec![rule(Trigger::EveryNTracks(2), 1)]);
    let tl = run(&st, at_14(0), Duration::from_secs(3600));
    let aired = ads_entries(&tl);
    assert!(!aired.is_empty());
    assert!(aired.iter().all(|e| e.item != radio_model::ItemId(100)));
}

#[test]
fn daypart_rule_only_fires_inside_window() {
    let (from, to) = (at_14(20), at_14(40));
    let mut r = rule(Trigger::EveryNTracks(1), 1);
    r.constraints = vec![Constraint::Daypart {
        from: jiff::civil::time(14, 20, 0, 0),
        to: jiff::civil::time(14, 40, 0, 0),
    }];
    let st = station_with(jingles(100, 1), vec![r]);
    let tl = run(&st, at_14(0), Duration::from_secs(3600));
    let aired = ads_entries(&tl);
    assert!(!aired.is_empty());
    assert!(
        aired.iter().all(|e| e.start >= from && e.start < to),
        "{aired:?}"
    );
}

#[test]
fn priority_orders_simultaneous_rules() {
    const LOW: PlaylistId = PlaylistId(3);
    for (ads_priority, low_priority, first) in [(5, 1, ADS), (1, 5, LOW)] {
        let mut st = station_with(jingles(100, 1), vec![rule(Trigger::EveryNTracks(3), 1)]);
        st.playlists[1].priority = ads_priority;
        let mut low = playlist(3, "low", jingles(200, 1));
        low.priority = low_priority;
        low.rules = vec![rule(Trigger::EveryNTracks(3), 1)];
        st.playlists.push(low);
        let tl = run(&st, at_14(0), Duration::from_secs(3600));
        let inserted: Vec<PlaylistId> = playlists(&tl.entries[3..5]).to_vec();
        assert_eq!(inserted.len(), 2);
        assert_eq!(inserted[0], first, "{:?}", playlists(&tl.entries));
        assert!(inserted.contains(&ADS) && inserted.contains(&LOW));
    }
}

#[test]
fn artist_separation_skips_same_artist_within_60m() {
    let mut items = tracks(20);
    items[1].artist = items[0].artist.clone();
    let mut st = station_with(Vec::new(), Vec::new());
    st.playlists[0].items = items;
    st.playlists[0].constraints = vec![Constraint::ArtistSeparation(Duration::from_secs(3600))];
    st.playlists.truncate(1);
    let tl = run(&st, at_14(0), Duration::from_secs(3600));
    let first = tl.entries.iter().find(|e| e.item.0 == 1).unwrap().start;
    assert_eq!(tl.entries[1].item.0, 3, "the same-artist track is skipped");
    assert!(
        tl.entries
            .iter()
            .filter(|e| e.item.0 == 2)
            .all(|e| e.start >= first + Duration::from_secs(3600))
    );
}

fn played(e: &Entry) -> radio_scheduler::Played {
    radio_scheduler::Played {
        item: e.item,
        playlist: e.playlist,
        at: e.start,
        artist: None,
        title: format!("item-{}", e.item.0),
    }
}

fn replan(st: &Station, now: Timestamp, aired: &[Entry], pinned: &[Entry]) -> Timeline {
    let mut history = PlayHistory::new();
    for e in aired {
        history.push(played(e));
    }
    merge(&PlanInput {
        station: st,
        history: &history,
        now,
        horizon: Duration::from_secs(3600),
        pinned,
    })
}

/// An ad for the minute-20 break waits on a deck before that minute comes. The break
/// still airs once: the plan does not add a second ad for the same minute.
#[test]
fn a_clock_break_on_a_deck_before_its_time_is_not_planned_twice() {
    let st = station_with(jingles(100, 3), vec![rule(Trigger::AtMinutes(vec![20]), 1)]);
    let now = at_14(14);
    let first = run(&st, now, Duration::from_secs(3600));
    let ad = first
        .entries
        .iter()
        .position(|e| e.playlist == ADS)
        .expect("the break is planned");
    assert!(ad <= 3, "the ad fits on the decks");
    let tl = replan(&st, now, &[], &first.entries[..=ad]);
    let ads = tl.entries.iter().filter(|e| e.playlist == ADS).count();
    assert_eq!(ads, 1, "{:?}", playlists(&tl.entries[..8]));
}

/// The first ad of a three-ad break is on a deck, alone. The other two still follow it.
#[test]
fn a_track_count_break_keeps_its_rest_after_its_first_ad_is_on_a_deck() {
    let st = station_with(jingles(100, 5), vec![rule(Trigger::EveryNTracks(2), 3)]);
    let first = run(&st, at_14(0), Duration::from_secs(3600));
    let (m, j) = (MAIN, ADS);
    assert_eq!(playlists(&first.entries[..6]), [m, m, j, j, j, m]);
    let tl = replan(&st, at_14(0), &[], &first.entries[..3]);
    assert_eq!(playlists(&tl.entries[..7]), [m, m, j, j, j, m, m]);
    let ids = |t: &Timeline| -> Vec<u64> { t.entries[..5].iter().map(|e| e.item.0).collect() };
    assert_eq!(ids(&tl), ids(&first), "the same ads, in the same order");
}

/// The first ad of a three-ad break has aired and the second waits on a deck. The third
/// still follows, and no fourth comes.
#[test]
fn a_track_count_break_keeps_its_last_ad_after_the_first_has_aired() {
    let st = station_with(jingles(100, 5), vec![rule(Trigger::EveryNTracks(2), 3)]);
    let first = run(&st, at_14(0), Duration::from_secs(3600));
    let e = &first.entries;
    let tl = replan(&st, e[3].start, &e[..3], &e[3..4]);
    let (m, j) = (MAIN, ADS);
    assert_eq!(playlists(&tl.entries[..4]), [j, j, m, m]);
}

/// A three-ad break at minute 20 is on air with its first ad when that minute has passed.
/// The other two ads still follow.
#[test]
fn a_clock_break_keeps_its_rest_after_its_time_has_passed() {
    let st = station_with(jingles(100, 5), vec![rule(Trigger::AtMinutes(vec![20]), 3)]);
    let first = run(&st, at_14(10), Duration::from_secs(3600));
    let e = &first.entries;
    let ad = e.iter().position(|x| x.playlist == ADS).unwrap();
    assert!(e[ad].start >= at_14(20));
    let now = e[ad].start + Duration::from_secs(1);
    let tl = replan(&st, now, &e[..=ad], &e[ad..=ad]);
    let (m, j) = (MAIN, ADS);
    assert_eq!(playlists(&tl.entries[..5]), [j, j, j, m, m]);
}

/// A fixed three-ad break is on air after its target: two ads are on decks. The third
/// still follows them.
#[test]
fn a_fixed_break_keeps_its_rest_after_its_target_has_passed() {
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
    let first = run(&st, at_14(55), Duration::from_secs(3600));
    let planned: Vec<Entry> = first
        .entries
        .iter()
        .filter(|e| e.playlist == ADS)
        .cloned()
        .collect();
    assert_eq!(planned.len(), 3);
    let now = planned[0].start + Duration::from_secs(5);
    let tl = replan(&st, now, &planned[..1], &planned[..2]);
    let ads: Vec<_> = tl.entries.iter().filter(|e| e.playlist == ADS).collect();
    assert_eq!(ads.len(), 3, "{ads:?}");
    assert!(ads[2].start > planned[1].start);
}

/// Gaps between airings of the same item of `playlist`, in order.
fn repeat_gaps(entries: &[Entry], playlist: PlaylistId) -> Vec<Duration> {
    let mut gaps = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if e.playlist != playlist {
            continue;
        }
        if let Some(prev) = entries[..i]
            .iter()
            .rev()
            .find(|p| p.playlist == playlist && p.item == e.item)
        {
            gaps.push(e.start.duration_since(prev.start).unsigned_abs());
        }
    }
    gaps
}

#[test]
fn repeat_gap_keeps_a_rule_from_playing_its_song_twice_in_a_row() {
    let mut st = station_with(jingles(100, 1), vec![rule(Trigger::EveryNTracks(3), 2)]);
    // Without a gap the single song fills the whole take, back to back.
    let tl = run(&st, at_14(0), Duration::from_secs(3 * 3600));
    assert!(
        tl.entries
            .windows(2)
            .any(|w| w[0].playlist == ADS && w[1].playlist == ADS && w[0].item == w[1].item)
    );

    let gap = Duration::from_secs(3600);
    st.playlists[1].repeat_gap = gap;
    let tl = run(&st, at_14(0), Duration::from_secs(3 * 3600));
    assert!(tl.entries.iter().any(|e| e.playlist == ADS));
    assert!(repeat_gaps(&tl.entries, ADS).iter().all(|g| *g >= gap));
    assert!(
        tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::NoEligibleItem { playlist, .. } if *playlist == ADS))
    );
}

#[test]
fn main_playlist_repeats_inside_its_gap_rather_than_going_silent() {
    let mut st = station_with(Vec::new(), Vec::new());
    // Nine 3-minute tracks come round long before the gap is over.
    st.playlists[0].repeat_gap = Duration::from_secs(3600);
    let tl = run(&st, at_14(0), Duration::from_secs(2 * 3600));
    assert!(
        !tl.warnings
            .iter()
            .any(|w| matches!(w, Warning::DeadAir { .. }))
    );
    let end = tl.entries.last().map(|e| e.start).unwrap();
    assert!(end.duration_since(at_14(0)).unsigned_abs() > Duration::from_secs(3600));
}
