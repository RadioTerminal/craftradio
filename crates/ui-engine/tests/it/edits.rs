use crate::fixtures::*;
use radio_engine::Command;
use radio_ui_egui::view::{DeckCommand, DeckState, Edit, ViewSource};
use std::time::{Duration, Instant};

fn titles(
    source: &mut radio_ui_engine::EngineSource,
    clock: &radio_playout::ManualClock,
) -> Vec<String> {
    use radio_playout::Clock;
    source
        .snapshot(clock.now())
        .rows
        .into_iter()
        .map(|r| r.title)
        .collect()
}

/// Two added files show as rows in the next snapshot, and Undo takes them out again.
#[test]
fn added_files_show_in_the_timeline_and_undo_removes_them() {
    let d = dir();
    let song = track(1, "Song", &wav(&d, "song", 3.0), 3.0);
    let (mut source, clock) = source(vec![song]);
    let view = wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    let main = view.main_playlist.clone();
    let paths = vec![wav(&d, "first-new", 2.0), wav(&d, "second-new", 2.0)];
    let added = source.edit(Edit::AddFiles {
        playlist: main,
        paths,
        at: None,
    });
    assert_eq!(added, Ok(2));
    let rows = titles(&mut source, &clock);
    assert!(
        rows.iter().any(|t| t == "first-new") && rows.iter().any(|t| t == "second-new"),
        "the new files are rows: {rows:?}"
    );

    assert_eq!(source.edit(Edit::Undo), Ok(1));
    let rows = titles(&mut source, &clock);
    assert!(
        !rows.iter().any(|t| t.ends_with("-new")),
        "undo takes the files out: {rows:?}"
    );
    assert_eq!(source.edit(Edit::Redo), Ok(1));
    let rows = titles(&mut source, &clock);
    assert!(rows.iter().any(|t| t == "first-new"), "redo puts them back");
}

/// A deck command names the item that the last snapshot showed. When another client
/// changed the deck since, the command fails and does nothing.
#[test]
fn a_deck_command_on_a_changed_deck_fails() {
    let d = dir();
    let songs = vec![
        track(1, "One", &wav(&d, "one", 5.0), 5.0),
        track(2, "Two", &wav(&d, "two", 5.0), 5.0),
    ];
    let (mut source, clock) = source(songs);
    let view = wait_for(&mut source, &clock, |v| {
        v.decks[0].state == DeckState::Playing
    });
    assert_eq!(view.decks[0].state, DeckState::Playing, "deck A plays");
    // Another client skips deck A behind the back of the UI.
    let _ = source
        .playout()
        .send(Command::DeckSkip {
            deck: 0,
            expect_item: None,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while source.playout().snapshot().decks[0].item == Some(radio_model::ItemId(1))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let stale = source.edit(Edit::Deck {
        deck: 0,
        command: DeckCommand::Skip,
    });
    assert!(stale.is_err(), "a stale skip is refused: {stale:?}");
}
