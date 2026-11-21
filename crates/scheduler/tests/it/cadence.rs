//! A woven-in "every N tracks" rule keeps its cadence on a four-deck playout.
//! The playout loads the plan, plays it and skips ahead. Decks and history count toward the rule.

use jiff::Timestamp;
use radio_model::{Item, ItemId, PickOrder, Placement, PlaylistId, Rule, Station, Trigger};
use radio_scheduler::{Entry, PlanInput, PlanOptions, PlayHistory, Played, Timeline, plan};
use radio_testkit::{playlist, station, track};
use std::collections::BTreeMap;
use std::time::Duration;

const POP: PlaylistId = PlaylistId(2);
const DECKS: usize = 4;

fn pop_station() -> Station {
    let mut main = playlist(
        1,
        "main",
        (1..=30)
            .map(|i| track(i, &format!("main-{i}"), &format!("band-{i}"), 180))
            .collect(),
    );
    main.is_main = true;
    main.pick = PickOrder::Shuffle { seed: 7 };
    let mut pop = playlist(
        2,
        "pop",
        (100..120)
            .map(|i| track(i, &format!("pop-{i}"), &format!("singer-{i}"), 180))
            .collect(),
    );
    pop.pick = PickOrder::Shuffle { seed: 7 };
    pop.rules = vec![Rule {
        trigger: Trigger::EveryNTracks(3),
        placement: Placement::BetweenTracks,
        take: 1,
        constraints: Vec::new(),
    }];
    station("UTC", vec![main, pop])
}

/// Each deck's entry follows the one before it. The first starts at `from`.
fn chain(decks: &mut [Entry], from: Timestamp, items: &BTreeMap<ItemId, Item>) {
    let mut at = from;
    let mut prev: Option<&Item> = None;
    for e in decks.iter_mut() {
        if let Some(p) = prev {
            at += radio_timing::advance(p, items.get(&e.item));
        }
        e.start = at;
        prev = items.get(&e.item);
    }
}

fn played(e: &Entry, items: &BTreeMap<ItemId, Item>) -> Played {
    let item = &items[&e.item];
    Played {
        item: e.item,
        playlist: e.playlist,
        // The wall-clock start is a few ms after the planned start.
        at: e.start + Duration::from_millis(7),
        artist: item.artist.clone(),
        title: item.title.clone(),
    }
}

/// Plays `advances` items on a simulated four-deck playout. Every third advance is a
/// "Play next" that cuts the item on air after 30 s. Returns the playlists in air order.
fn simulate(st: &Station, advances: usize) -> Vec<PlaylistId> {
    let items: BTreeMap<_, _> = st
        .playlists
        .iter()
        .flat_map(|p| p.items.iter().map(|i| (i.id, i.clone())))
        .collect();
    let opts = PlanOptions {
        resume_rotation: true,
        ..PlanOptions::default()
    };
    let mut now: Timestamp = "2026-11-21T13:00:00Z".parse().unwrap();
    let mut history = PlayHistory::new();
    let mut decks: Vec<Entry> = Vec::new();
    let mut prev: Option<Timeline> = None;
    let mut aired = Vec::new();
    for step in 0..=advances {
        let input = PlanInput {
            station: st,
            history: &history,
            now,
            horizon: Duration::from_secs(3 * 3600),
            pinned: &decks,
        };
        let tl = plan(&input, &opts, prev.as_ref());
        // Free decks take the next plan entries after the last loaded one.
        let last = decks.last().map(|d| d.start);
        let next: Vec<Entry> = tl
            .entries
            .iter()
            .filter(|e| last.is_none_or(|l| e.start > l) && !decks.contains(e))
            .take(DECKS - decks.len())
            .cloned()
            .collect();
        decks.extend(next);
        prev = Some(tl);
        if step == 0 {
            history.push(played(&decks[0], &items));
            aired.push(decks[0].playlist);
        }
        if step == advances {
            break;
        }
        // The item on air ends: at its segue, or early by "Play next".
        let on_air = decks.remove(0);
        now = if step % 3 == 2 {
            on_air.start + Duration::from_secs(30)
        } else {
            on_air.start + radio_timing::advance(&items[&on_air.item], items.get(&decks[0].item))
        };
        chain(&mut decks, now, &items);
        history.push(played(&decks[0], &items));
        aired.push(decks[0].playlist);
    }
    aired
}

#[test]
fn every_3_tracks_insertions_reach_the_decks_and_air() {
    let aired = simulate(&pop_station(), 24);
    let pops: Vec<usize> = (0..aired.len()).filter(|&i| aired[i] == POP).collect();
    assert!(
        pops.first().is_some_and(|&i| i < 5),
        "a POP item airs within the first 5 items: {aired:?}"
    );
    assert!(
        pops.windows(2).all(|w| w[1] - w[0] <= 4),
        "POP items air at most 4 items apart: {aired:?}"
    );
    assert!(
        pops.windows(2).all(|w| w[1] - w[0] > 3),
        "at least 3 main items air between two POP items: {aired:?}"
    );
    let last = pops.last().copied().unwrap_or(0);
    assert!(
        aired.len() - last <= 5,
        "POP keeps airing to the end of the run: {aired:?}"
    );
}
