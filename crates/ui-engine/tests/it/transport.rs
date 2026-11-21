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

/// The user saw one track on decks A, B and D after Skip. Many skips must put no
/// song on two decks. They must put no song twice in a row in Main.
#[test]
fn skipping_never_duplicates_a_track() {
    let (mut source, clock) = six_songs(20.0);
    for _ in 0..6 {
        let view = source.snapshot(clock.now());
        let deck = on_air(&view).expect("a deck is on air");
        let item = view.decks[deck].item;
        let r = source.edit(Edit::Deck {
            deck,
            command: DeckCommand::Skip,
        });
        assert!(r.is_ok(), "skip: {r:?}");
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
