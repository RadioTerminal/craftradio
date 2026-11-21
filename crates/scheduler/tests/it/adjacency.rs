//! Adjacency rules (`NotAdjacentToSelf`, `NotAdjacentTo`) hold after fitting, also for
//! entries the fitter moves across a fixed event.

use jiff::Timestamp;
use radio_model::{
    Constraint, FixMode, Item, Placement, Playlist, PlaylistId, Rule, Station, Trigger,
};
use radio_scheduler::{
    Entry, Fixed, Origin, PlanInput, PlanOptions, PlayHistory, Timeline, fit_all, plan,
};
use radio_testkit::{jingle, playlist, station, track};
use radio_timing::item_len;
use std::time::Duration;

const MAIN: PlaylistId = PlaylistId(1);
const JINGLES: PlaylistId = PlaylistId(2);
const NEWS: PlaylistId = PlaylistId(3);

fn t0() -> Timestamp {
    "2026-11-21T13:00:00Z".parse().unwrap()
}

fn at(secs: u64) -> Timestamp {
    t0() + Duration::from_secs(secs)
}

/// A movable, trimmable station ID, as in the example station.
fn station_id(id: u64, secs: u32) -> Item {
    let mut item = jingle(id, secs);
    item.movable = true;
    item.trimmable = true;
    item
}

/// The adjacency constraints that apply to `e`: its playlist's, plus those of the rule
/// that inserted it.
fn adjacency_rules<'a>(st: &'a Station, e: &Entry) -> Vec<&'a Constraint> {
    let Some(pl) = st.playlists.iter().find(|p| p.id == e.playlist) else {
        return Vec::new();
    };
    let from_rule = match e.origin {
        Origin::Rule { playlist, rule } if playlist == pl.id => {
            pl.rules.get(rule).map_or(&[][..], |r| &r.constraints[..])
        }
        _ => &[],
    };
    pl.constraints
        .iter()
        .chain(from_rule)
        .filter(|c| {
            matches!(
                c,
                Constraint::NotAdjacentToSelf | Constraint::NotAdjacentTo(_)
            )
        })
        .collect()
}

/// Every pair of neighbours where the second breaks one of its adjacency rules.
fn adjacency_violations(st: &Station, tl: &Timeline) -> Vec<(Entry, Entry)> {
    tl.entries
        .windows(2)
        .filter(|w| {
            adjacency_rules(st, &w[1]).iter().any(|c| match c {
                Constraint::NotAdjacentToSelf => w[0].playlist == w[1].playlist,
                Constraint::NotAdjacentTo(other) => w[0].playlist == *other,
                _ => false,
            })
        })
        .map(|w| (w[0].clone(), w[1].clone()))
        .collect()
}

/// Main tracks, a jingle playlist whose every-3-tracks rule carries `NotAdjacentToSelf`
/// (on the rule, not the playlist, as in the example station), and a news playlist.
fn small_station(first_len: u32) -> Station {
    let mut main = playlist(
        1,
        "main",
        vec![
            track(10, "first", "artist-a", first_len),
            track(11, "second", "artist-b", 180),
            track(12, "third", "artist-c", 180),
        ],
    );
    main.is_main = true;
    let mut jingles: Playlist =
        playlist(2, "jingles", vec![station_id(20, 10), station_id(21, 10)]);
    jingles.rules = vec![Rule {
        trigger: Trigger::EveryNTracks(3),
        placement: Placement::BetweenTracks,
        take: 1,
        constraints: vec![Constraint::NotAdjacentToSelf],
    }];
    let news = playlist(3, "news", vec![station_id(30, 60)]);
    station("UTC", vec![main, jingles, news])
}

fn entry(st: &Station, pl: PlaylistId, item: u64, start: Timestamp, origin: Origin) -> Entry {
    let it = st
        .playlists
        .iter()
        .flat_map(|p| p.items.iter())
        .find(|i| i.id.0 == item)
        .unwrap();
    Entry {
        playlist: pl,
        item: it.id,
        start,
        len: item_len(it),
        origin,
        locked: false,
        fixed: None,
        trim: Duration::ZERO,
    }
}

/// The first track ends just after a hard news event at +300 s, a station ID follows it,
/// and merge inserted the next station ID right after the news.
fn timeline_around_news(st: &Station, first_segue: u64) -> Timeline {
    let jingle_rule = Origin::Rule {
        playlist: JINGLES,
        rule: 0,
    };
    let mut news = entry(st, NEWS, 30, at(300), Origin::Manual);
    news.fixed = Some(Fixed {
        target: at(300),
        mode: FixMode::Hard,
        tolerance: Duration::from_secs(1),
        priority: 10,
        frozen: false,
    });
    Timeline {
        entries: vec![
            entry(st, MAIN, 10, at(0), Origin::Main),
            entry(st, JINGLES, 20, at(first_segue), jingle_rule.clone()),
            news,
            entry(st, JINGLES, 21, at(360), jingle_rule),
            entry(st, MAIN, 11, at(370), Origin::Main),
            entry(st, MAIN, 12, at(548), Origin::Main),
        ],
        fits: Vec::new(),
        warnings: Vec::new(),
    }
}

#[test]
fn no_adjacent_jingles_after_fixed_event() {
    // The first track hands over at +298 s, so its station ID ends 8 s past the news. The
    // nearest fit drops that station ID from before the news, which would put it right
    // in front of the one merge already placed after the news.
    let st = small_station(300);
    let mut tl = timeline_around_news(&st, 298);
    fit_all(&mut tl, &st, &PlayHistory::new());
    let bad = adjacency_violations(&st, &tl);
    assert!(bad.is_empty(), "adjacent station IDs: {bad:#?}");
    let news = tl.entries.iter().find(|e| e.fixed.is_some()).unwrap();
    assert_eq!(news.start, at(300), "the hard event keeps its time");
}

#[test]
fn no_adjacent_jingles_when_hard_event_pushes_one_past_it() {
    // The station ID would start exactly at the hard event, so the fitter moves it after
    // the event, where merge already placed another one.
    let st = small_station(302);
    let mut tl = timeline_around_news(&st, 300);
    fit_all(&mut tl, &st, &PlayHistory::new());
    let bad = adjacency_violations(&st, &tl);
    assert!(bad.is_empty(), "adjacent station IDs: {bad:#?}");
    let news = tl.entries.iter().find(|e| e.fixed.is_some()).unwrap();
    assert_eq!(news.start, at(300), "the hard event keeps its time");
}

fn example_station() -> Station {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/station.toml");
    let st: Station = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    st.validate().unwrap();
    st
}

fn plan_example(st: &Station, now: Timestamp, prev: Option<&Timeline>) -> Timeline {
    let history = PlayHistory::new();
    plan(
        &PlanInput {
            station: st,
            history: &history,
            now,
            horizon: Duration::from_secs(24 * 3600),
            pinned: &[],
        },
        &PlanOptions::default(),
        prev,
    )
}

#[test]
fn example_station_has_no_adjacent_entries_from_not_adjacent_playlists_over_a_day() {
    let st = example_station();
    let first = plan_example(&st, t0(), None);
    // The sweep only proves something if the example has adjacency rules that apply.
    let guarded = first
        .entries
        .iter()
        .filter(|e| !adjacency_rules(&st, e).is_empty())
        .count();
    assert!(
        guarded > 0,
        "no entry in the example plan has an adjacency rule"
    );
    let bad = adjacency_violations(&st, &first);
    assert!(bad.is_empty(), "first plan: {bad:#?}");

    // Re-planning, also with the next fixed event close enough to be frozen.
    for later in [0, 30 * 60, 55 * 60] {
        let now = t0() + Duration::from_secs(later);
        let again = plan_example(&st, now, Some(&first));
        let bad = adjacency_violations(&st, &again);
        assert!(bad.is_empty(), "re-plan at {now}: {bad:#?}");
    }
}
