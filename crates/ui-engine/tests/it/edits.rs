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
    // The song on air outlasts the test, so the new files never go on air during it.
    let song = track(1, "Song", &wav(&d, "song", 30.0), 30.0);
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
    let _ = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| r.title == "second-new")
    });
    let rows = titles(&mut source, &clock);
    assert!(
        rows.iter().any(|t| t == "first-new") && rows.iter().any(|t| t == "second-new"),
        "the new files are rows: {rows:?}"
    );

    assert_eq!(source.edit(Edit::Undo), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| {
        !v.rows.iter().any(|r| r.title.ends_with("-new"))
    });
    let rows = titles(&mut source, &clock);
    assert!(
        !rows.iter().any(|t| t.ends_with("-new")),
        "undo takes the files out: {rows:?}"
    );
    assert_eq!(source.edit(Edit::Redo), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| r.title == "first-new")
    });
    let rows = titles(&mut source, &clock);
    assert!(rows.iter().any(|t| t == "first-new"), "redo puts them back");
}

/// A deck command names the item that the last snapshot showed. When another client
/// changed the deck since, the command fails and does nothing.
#[test]
fn a_deck_command_on_a_changed_deck_fails() {
    let d = dir();
    // Six songs: the decks hold four, and deck A reloads with a song it did not hold.
    let songs = (1..=6)
        .map(|i| {
            track(
                i,
                &format!("Song {i}"),
                &wav(&d, &format!("s{i}"), 5.0),
                5.0,
            )
        })
        .collect();
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

/// A file added at the row after the one on air plays next: it is the next row, and the main
/// playlist goes on after it with no repeat of it.
#[test]
fn a_file_added_after_the_row_on_air_plays_next() {
    use radio_playout::Clock;
    let d = dir();
    let songs = (1..=3)
        .map(|i| {
            track(
                i,
                &format!("Song {i}"),
                &wav(&d, &format!("s{i}"), 5.0),
                5.0,
            )
        })
        .collect();
    let (mut source, clock) = source(songs);
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.len() > 3 && v.decks.iter().any(|d| d.state == DeckState::Playing)
    });
    let main = view.main_playlist.clone();
    let added = source.edit(Edit::AddFiles {
        playlist: main,
        paths: vec![wav(&d, "inserted", 2.0)],
        at: Some(1),
    });
    assert_eq!(added, Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.get(1).is_some_and(|r| r.title == "inserted")
    });
    let rows: Vec<&str> = view.rows.iter().take(4).map(|r| r.title.as_str()).collect();
    assert_eq!(rows.get(1), Some(&"inserted"), "{rows:?}");
    assert_ne!(
        rows.get(2),
        Some(&"inserted"),
        "it plays once here: {rows:?}"
    );
    let _ = clock.now();
}

/// A fader change goes out without a wait, so its error comes back with the next change.
#[test]
fn a_failed_fader_change_reports_its_error() {
    use radio_ui_egui::view::StripChange;
    let (mut source, clock) = source(Vec::new());
    let _ = wait_for(&mut source, &clock, |_| true);
    let bad = |source: &mut radio_ui_engine::EngineSource| {
        source.edit(Edit::SetStrip {
            strip: "No such strip".into(),
            change: StripChange::Gain(-3.0),
        })
    };
    assert_eq!(bad(&mut source), Ok(1), "the first change goes out at once");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut answer = Ok(1);
    while answer.is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(60));
        answer = bad(&mut source);
    }
    assert!(
        answer.is_err(),
        "the error of the merged command comes back"
    );
}

/// An edit never waits on the files: adding a folder of 100 files returns at once, and the
/// files arrive later.
#[test]
fn adding_a_folder_of_100_files_returns_at_once() {
    let d = dir();
    let big = d.join("Hundred");
    std::fs::create_dir_all(&big).unwrap();
    let first = wav(&big, "f000", 2.0);
    for k in 1..100 {
        std::fs::copy(&first, big.join(format!("f{k:03}.wav"))).unwrap();
    }
    let (mut source, clock) = source(Vec::new());
    let view = wait_for(&mut source, &clock, |_| true);
    let started = Instant::now();
    let added = source.edit(Edit::AddFiles {
        playlist: view.main_playlist.clone(),
        paths: vec![big.to_string_lossy().into_owned()],
        at: None,
    });
    let took = started.elapsed();
    assert_eq!(added, Ok(1));
    assert!(took < Duration::from_millis(5), "the edit took {took:?}");
    let view = wait_for(&mut source, &clock, |v| v.main.items.len() == 100);
    assert_eq!(view.main.items.len(), 100, "the files arrive later");
}
