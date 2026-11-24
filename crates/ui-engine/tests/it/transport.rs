//! The deck transport through the UI path: an `Edit`, its engine command, then playout.

use crate::fixtures::*;
use radio_playout::{Clock, ManualClock};
use radio_ui_egui::view::{DeckCommand, DeckState, Edit, StationView, ViewSource};
use radio_ui_engine::EngineSource;
use std::time::{Duration, Instant};

/// A station of six distinct songs, `secs` long each, with a deck on air.
fn six_songs(secs: f32) -> (EngineSource, ManualClock) {
    let d = dir();
    let songs = (1..=6)
        .map(|i| {
            let title = format!("Song {i}");
            track(i, &title, &wav(&d, &format!("t{i}"), secs), secs)
        })
        .collect();
    let (mut source, clock) = source(songs);
    let _ = settle(&mut source, &clock, Duration::from_secs(5), |v| {
        v.decks.iter().any(|d| d.state == DeckState::Playing)
            && v.decks.iter().filter(|d| d.item.is_some()).count() == 4
    });
    (source, clock)
}

/// Takes snapshots until `done` holds, for at most `max`. The clock follows real
/// time. Each snapshot must have no duplicates.
fn settle(
    source: &mut EngineSource,
    clock: &ManualClock,
    max: Duration,
    done: impl Fn(&StationView) -> bool,
) -> StationView {
    watch(source, clock, max, true, done)
}

/// Does [`settle`]. With `ticking` false, the clock stands still.
fn watch(
    source: &mut EngineSource,
    clock: &ManualClock,
    max: Duration,
    ticking: bool,
    done: impl Fn(&StationView) -> bool,
) -> StationView {
    let start = Instant::now();
    let mut last = Instant::now();
    loop {
        if ticking {
            clock.advance(last.elapsed());
        }
        last = Instant::now();
        let view = source.snapshot(clock.now());
        no_duplicates(&view);
        if done(&view) || start.elapsed() > max {
            return view;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// No item sits on two decks, and the Main timeline has no item twice in a row.
fn no_duplicates(view: &StationView) {
    let decks: Vec<u64> = view.decks.iter().filter_map(|d| d.item).collect();
    for (k, a) in decks.iter().enumerate() {
        assert!(
            !decks[k + 1..].contains(a),
            "item {a} is on two decks: {:?}",
            view.decks
                .iter()
                .map(|d| (d.label, d.title.as_str()))
                .collect::<Vec<_>>()
        );
    }
    let titles: Vec<&str> = view
        .rows
        .iter()
        .filter(|r| r.is_main)
        .map(|r| r.title.as_str())
        .collect();
    assert!(
        titles.windows(2).all(|w| w[0] != w[1]),
        "the Main timeline repeats an item: {titles:?}"
    );
}

fn on_air(view: &StationView) -> Option<usize> {
    view.decks
        .iter()
        .position(|d| d.state == DeckState::Playing)
}

/// The NEXT `rank` deck: the deck that waits, by its start. NEXT 1 plays next.
fn next(view: &StationView, rank: usize) -> Option<usize> {
    let mut ready: Vec<usize> = (0..view.decks.len())
        .filter(|&k| view.decks[k].state == DeckState::Ready)
        .collect();
    ready.sort_by_key(|&k| view.decks[k].start.as_ref().map(|s| s.timestamp()));
    ready.get(rank - 1).copied()
}

/// The user saw one track on decks A, B and D after a press of next. Many presses of
/// Play next must put no song on two decks. They must put no song twice in a row in Main.
#[test]
fn play_next_never_duplicates_a_track() {
    let (mut source, clock) = six_songs(20.0);
    for _ in 0..6 {
        let view = source.snapshot(clock.now());
        let item = on_air(&view).and_then(|k| view.decks[k].item);
        let r = source.edit(Edit::PlayNext { fade_in_s: 0.5 });
        assert!(r.is_ok(), "play next: {r:?}");
        // A real output sounds a start some ms after the control clock. The clock steps
        // back a little, so the new item on air starts after `now` like that.
        clock.set(clock.now() - Duration::from_millis(80));
        // Plans run every second, so this waits for a few.
        let _ = settle(&mut source, &clock, Duration::from_millis(2500), |_| false);
        let _ = settle(&mut source, &clock, Duration::from_secs(4), |v| {
            on_air(v).is_some_and(|k| v.decks[k].item != item)
                && v.decks.iter().filter(|d| d.item.is_some()).count() == 4
        });
    }
}

/// The global Play next starts the NEXT 1 deck.
#[test]
fn play_next_plays_the_next_1_deck() {
    let (mut source, clock) = six_songs(20.0);
    let view = source.snapshot(clock.now());
    let first = next(&view, 1).and_then(|k| view.decks[k].item);
    assert!(first.is_some(), "a deck waits as NEXT 1");
    assert_eq!(source.edit(Edit::PlayNext { fade_in_s: 0.5 }), Ok(1));
    let view = settle(&mut source, &clock, Duration::from_secs(4), |v| {
        on_air(v).is_some_and(|k| v.decks[k].item == first)
    });
    assert_eq!(on_air(&view).and_then(|k| view.decks[k].item), first);
}

/// Play this deck on NEXT 2: it goes on air, and the old item leaves the air. The NEXT 1
/// item waits for a later turn. No song is on two decks or twice in a row on the way.
#[test]
fn play_this_deck_on_next_2_goes_on_air_and_skips_next_1() {
    let (mut source, clock) = six_songs(20.0);
    // NEXT 2 needs a third deck: the station uses all four here, set as Preferences sets it.
    let four = Edit::SetPartSetting {
        part: radio_model::DECKS_PART.into(),
        key: radio_model::DECK_COUNT_KEY.into(),
        value: 4.into(),
    };
    assert_eq!(source.edit(four), Ok(1));
    let view = settle(&mut source, &clock, Duration::from_secs(4), |v| {
        next(v, 2).is_some()
    });
    let old = on_air(&view).expect("a deck is on air");
    let one = next(&view, 1).and_then(|k| view.decks[k].item);
    let two = next(&view, 2).expect("a deck waits as NEXT 2");
    let item = view.decks[two].item;
    let r = source.edit(Edit::Deck {
        deck: two,
        command: DeckCommand::Play(0.5),
    });
    assert_eq!(r, Ok(1));
    let view = settle(&mut source, &clock, Duration::from_secs(4), |v| {
        on_air(v) == Some(two) && v.decks[old].state != DeckState::Playing
    });
    assert_eq!(on_air(&view), Some(two), "NEXT 2 is on air");
    assert_eq!(view.decks[two].item, item);
    assert_ne!(
        view.decks[old].state,
        DeckState::Playing,
        "the old item faded"
    );
    let view = settle(&mut source, &clock, Duration::from_millis(2500), |_| false);
    assert!(
        view.decks.iter().all(|d| d.item != one),
        "the NEXT 1 song left the decks for a later turn"
    );
}

/// A folder or a system edit adds the songs, not a person, so no row shows a lock.
#[test]
fn rows_that_nobody_placed_by_hand_have_no_lock() {
    let (mut source, clock) = six_songs(20.0);
    let view = settle(&mut source, &clock, Duration::from_secs(1), |_| false);
    let rows: Vec<(&str, bool)> = view
        .rows
        .iter()
        .take(6)
        .map(|r| (r.title.as_str(), r.locked))
        .collect();
    assert!(
        rows.iter().all(|(_, locked)| !locked),
        "nobody placed these rows by hand, so none is locked: {rows:?}"
    );
}
