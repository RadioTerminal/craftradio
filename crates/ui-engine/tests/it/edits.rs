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
        command: DeckCommand::Fade(1.0),
    });
    assert!(stale.is_err(), "a stale fade is refused: {stale:?}");
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

/// Remove from Library takes every item of the file out of the playlists, as one undo step.
/// The library shows a file once, so a second item of the same file goes too. The file stays.
#[test]
fn a_library_file_leaves_its_playlists_and_undo_brings_it_back() {
    let d = dir();
    let mut songs: Vec<_> = (1..=8)
        .map(|n| {
            track(
                n,
                &format!("Song {n}"),
                &wav(&d, &format!("s{n}"), 2.0),
                2.0,
            )
        })
        .collect();
    let path = songs[7].source.clone();
    let mut twin = songs[7].clone();
    twin.id = radio_model::ItemId(9);
    songs.push(twin);
    let (mut source, clock) = source(songs);
    let view = wait_for(&mut source, &clock, |v| v.library.len() == 8);
    let last = view
        .library
        .iter()
        .find(|l| l.title == "Song 8")
        .expect("Song 8 is in the library")
        .id;
    assert_eq!(
        source.edit(Edit::RemoveLibraryItems(vec![last])),
        Ok(2),
        "both items of the file go"
    );
    let view = wait_for(&mut source, &clock, |v| v.library.len() == 7);
    assert!(
        view.library.iter().all(|l| l.title != "Song 8"),
        "no other item of the file takes its row"
    );
    if let radio_model::Source::File { path } = &path {
        assert!(
            std::path::Path::new(path).exists(),
            "the file stays on disk"
        );
    }
    assert!(source.edit(Edit::Undo).is_ok());
    let view = wait_for(&mut source, &clock, |v| v.library.len() == 8);
    assert!(
        view.library.iter().any(|l| l.title == "Song 8"),
        "undo brings it back"
    );
}

/// A change on the Processing tab reaches the engine, and the next view reads it back.
#[test]
fn a_dynamics_edit_reaches_the_engine_and_the_view() {
    let (mut source, clock) = source(Vec::new());
    let before = wait_for(&mut source, &clock, |_| true).dynamics;
    assert!(!before.compressor.enabled, "the compressor starts off");
    let mut next = before;
    next.compressor.enabled = true;
    next.limiter.ceiling_db = -2.0;
    assert_eq!(source.edit(Edit::SetDynamics(next)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| v.dynamics == next);
    assert_eq!(view.dynamics, next, "the view reads the stored setting");
    let mut bad = next;
    bad.compressor.ratio = 0.5;
    let _ = source.edit(Edit::SetDynamics(bad));
    // The engine answers later; the setting in force stays.
    std::thread::sleep(Duration::from_millis(200));
    let after = wait_for(&mut source, &clock, |_| true);
    assert_eq!(after.dynamics, next, "a bad ratio changes nothing");
}

/// A change of the Loudness section reaches the engine and the next view, and a bad
/// target changes nothing.
#[test]
fn a_loudness_edit_reaches_the_engine_and_the_view() {
    let (mut source, clock) = source(Vec::new());
    let before = wait_for(&mut source, &clock, |_| true).loudness;
    assert!(before.enabled, "normalisation starts on");
    let next = radio_model::LoudnessSettings {
        enabled: false,
        target_lufs: -23.0,
        ..before
    };
    assert_eq!(source.edit(Edit::SetLoudness(next)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| v.loudness == next);
    assert_eq!(view.loudness, next);
    let bad = radio_model::LoudnessSettings {
        target_lufs: -3.0,
        ..next
    };
    let _ = source.edit(Edit::SetLoudness(bad));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(wait_for(&mut source, &clock, |_| true).loudness, next);
}

/// A deck with a file shows the gain that playout applies, once the file is measured.
#[test]
fn a_deck_shows_the_measured_gain_of_its_file() {
    let dir = dir();
    // A short file, so a debug build measures it well inside the wait.
    let path = wav(&dir, "measured", 3.0);
    let (mut source, clock) = source(vec![track(1, "Amber Wire", &path, 3.0)]);
    let view = wait_for(&mut source, &clock, |v| {
        v.decks.iter().any(|d| {
            d.gain
                .is_some_and(|g| g.source == radio_model::GainSource::Measured)
        })
    });
    let gain = view
        .decks
        .iter()
        .find_map(|d| d.gain)
        .expect("a file deck has a gain");
    assert_eq!(gain.source, radio_model::GainSource::Measured);
    assert!(gain.loudness.is_some());
}

/// A trim set in the metadata editor goes to the engine, and the row reads it back.
#[test]
fn a_trim_from_the_metadata_editor_reaches_the_engine() {
    let dir = dir();
    let path = wav(&dir, "trimmed", 20.0);
    let (mut source, clock) = source(vec![track(1, "Amber Wire", &path, 20.0)]);
    let view = wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    let row = view.rows[0].clone();
    assert_eq!(
        row.meta.gain_trim_db,
        Some(0.0),
        "a file row has no trim yet"
    );
    let mut meta = row.meta.clone();
    meta.gain_trim_db = Some(-2.5);
    let target = radio_ui_egui::view::MetaTarget::Row(row.id);
    assert_eq!(source.edit(Edit::SetMeta(vec![(target, meta)])), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows
            .first()
            .is_some_and(|r| r.meta.gain_trim_db == Some(-2.5))
    });
    assert_eq!(view.rows[0].meta.gain_trim_db, Some(-2.5));
}

/// Markers set in the metadata editor go to the engine as set by hand, and read back.
#[test]
fn markers_from_the_metadata_editor_reach_the_engine_as_set_by_hand() {
    use radio_ui_egui::view::Markers;
    let dir = dir();
    let path = wav(&dir, "marked", 20.0);
    let (mut source, clock) = source(vec![track(1, "Amber Wire", &path, 20.0)]);
    let view = wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    let row = view.rows[0].clone();
    let before = row.meta.markers.expect("a file row has markers to edit");
    assert!(!before.by_hand);
    let mut meta = row.meta.clone();
    meta.markers = Some(Markers {
        intro_end_s: Some(4.0),
        outro_start_s: Some(15.0),
        by_hand: true,
    });
    let target = radio_ui_egui::view::MetaTarget::Row(row.id);
    assert_eq!(source.edit(Edit::SetMeta(vec![(target, meta)])), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows
            .first()
            .and_then(|r| r.meta.markers)
            .is_some_and(|m| m.by_hand)
    });
    let m = view.rows[0].meta.markers.unwrap();
    assert_eq!((m.intro_end_s, m.outro_start_s), (Some(4.0), Some(15.0)));
}

/// A jingle that plays over a song shows on the song's row, with its playlist and side,
/// and takes no row of its own.
#[test]
fn an_over_item_shows_on_its_songs_row() {
    use radio_engine::Command;
    use radio_model::{MarkerSource, PlayMode, PlaylistId};
    let dir = dir();
    let path = wav(&dir, "over-song", 20.0);
    let songs: Vec<_> = (1..=4)
        .map(|i| {
            let mut s = track(i, &format!("Song {i}"), &path, 20.0);
            let t = s.timing.as_mut().unwrap();
            t.intro_end = Some(std::time::Duration::from_secs(5));
            t.markers = MarkerSource::Detected;
            s
        })
        .collect();
    let id = wav(&dir, "over-id", 2.0);
    let mut engine = radio_engine::Engine::open_in_memory().unwrap();
    engine
        .dispatch(Command::PlaylistAddItems {
            playlist: PlaylistId(1),
            items: songs,
            at: None,
            lock: false,
        })
        .unwrap();
    let made = engine
        .dispatch(Command::PlaylistCreate { name: "IDs".into() })
        .unwrap();
    let ids = PlaylistId(made.data.and_then(|v| v.as_u64()).unwrap());
    engine
        .dispatch(Command::PlaylistAddItems {
            playlist: ids,
            items: vec![track(50, "Station ID", &id, 2.0)],
            at: None,
            lock: true,
        })
        .unwrap();
    engine
        .dispatch(Command::PlaylistSetRulesScript {
            playlist: ids,
            script: r#"rule({every: {tracks: 1}, place: "between", take: 1, constraints: []});"#
                .into(),
        })
        .unwrap();
    engine
        .dispatch(Command::PlaylistSetPlays {
            playlist: ids,
            plays: PlayMode::Over { duck_db: -12 },
        })
        .unwrap();
    let (mut source, clock) = source_from(engine);
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| !r.over.is_empty())
    });
    let host = view.rows.iter().find(|r| !r.over.is_empty()).unwrap();
    let o = &host.over[0];
    assert_eq!(
        (o.playlist.as_str(), o.title.as_str()),
        ("IDs", "Test Artist – Station ID")
    );
    assert_eq!(o.side, radio_ui_egui::view::OverSide::Intro);
    assert!(
        !view.rows.iter().any(|r| r.title.ends_with("Station ID")),
        "the ID has a row of its own"
    );
}
