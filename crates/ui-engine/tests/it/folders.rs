use crate::fixtures::*;
use radio_playout::Clock;
use radio_ui_egui::view::{Edit, FolderSource, ViewSource};
use std::time::{Duration, Instant};

/// A library folder is read on a worker with the limits of the source. A folder with more
/// files than the limit shows that it was cut.
#[test]
fn a_folder_reads_in_the_background_and_reports_its_limit() {
    let d = dir();
    let music = d.join("music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["a", "b", "c"] {
        wav(&music, name, 1.0);
    }
    let (source, clock) = source(Vec::new());
    let mut source = source.with_limits(radio_library::Limits {
        max_files: 2,
        ..radio_library::Limits::DEFAULT
    });
    let path = music.to_string_lossy().into_owned();
    assert!(source.edit(Edit::AddFolder { path: path.clone() }).is_ok());
    let view = wait_for(&mut source, &clock, |v| {
        v.folders.iter().any(|f| f.files.len() == 2)
    });
    let folder = view
        .folders
        .iter()
        .find(|f| f.path == path)
        .expect("the folder");
    assert_eq!(folder.files.len(), 2, "the limit holds");
    assert!(folder.truncated, "the folder says that it was cut");
    assert!(
        view.library.iter().any(|l| l.path.ends_with("a.wav")),
        "the files are in the library"
    );
    assert!(source.edit(Edit::Rescan { folder: path }).is_ok());
}

/// A folder playlist with watch on takes in a file that appears in its folder.
#[test]
fn a_watched_folder_playlist_takes_in_a_new_file() {
    let d = dir();
    let jingles = d.join("Jingles");
    std::fs::create_dir_all(&jingles).unwrap();
    wav(&jingles, "first", 1.0);
    let (mut source, clock) = source(Vec::new());
    let name = source
        .playlist_from_folder(FolderSource {
            path: jingles.to_string_lossy().into_owned(),
            recursive: false,
            watch: true,
        })
        .expect("a playlist from the folder");
    let count = |source: &mut radio_ui_engine::EngineSource| {
        let view = source.snapshot(clock.now());
        view.tabs
            .iter()
            .find(|t| t.name == name)
            .map_or(0, |t| t.items.len())
    };
    assert_eq!(count(&mut source), 1);
    wav(&jingles, "second", 1.0);
    let deadline = Instant::now() + Duration::from_secs(15);
    while count(&mut source) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(count(&mut source), 2, "the new file joins the playlist");
}

/// One file that appears in a watched folder joins its playlist once, however many snapshots
/// follow.
#[test]
fn one_new_file_in_a_watched_folder_adds_one_item() {
    let d = dir();
    let jingles = d.join("Watched");
    std::fs::create_dir_all(&jingles).unwrap();
    wav(&jingles, "first", 1.0);
    let (mut source, clock) = source(Vec::new());
    let name = source
        .playlist_from_folder(FolderSource {
            path: jingles.to_string_lossy().into_owned(),
            recursive: false,
            watch: true,
        })
        .expect("a playlist from the folder");
    let count = |source: &mut radio_ui_engine::EngineSource| {
        let view = source.snapshot(clock.now());
        view.tabs
            .iter()
            .find(|t| t.name == name)
            .map_or(0, |t| t.items.len())
    };
    assert_eq!(count(&mut source), 1);
    let second = wav(&jingles, "second", 1.0);
    for k in 0..200 {
        let _ = count(&mut source);
        std::thread::sleep(Duration::from_millis(20));
        // Other programs touch the file now and then, as a tag editor or an indexer does.
        if k % 40 == 39 {
            let bytes = std::fs::read(&second).unwrap();
            std::fs::write(&second, bytes).unwrap();
        }
    }
    assert_eq!(count(&mut source), 2, "one new file is one new item");
}

/// One file added through the UI to a playlist that watches the folder of the file is one
/// item, however many snapshots follow.
#[test]
fn one_file_added_to_a_watched_playlist_adds_one_item() {
    let d = dir();
    let music = d.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    wav(&music, "first", 1.0);
    let (mut source, clock) = source(Vec::new());
    let name = source
        .playlist_from_folder(FolderSource {
            path: music.to_string_lossy().into_owned(),
            recursive: false,
            watch: true,
        })
        .expect("a playlist from the folder");
    let count = |source: &mut radio_ui_engine::EngineSource| {
        let view = source.snapshot(clock.now());
        view.tabs
            .iter()
            .find(|t| t.name == name)
            .map_or(0, |t| t.items.len())
    };
    let elsewhere = d.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let track = wav(&elsewhere, "track", 1.0);
    assert_eq!(count(&mut source), 1);
    assert_eq!(
        source.edit(Edit::AddFiles {
            playlist: name.clone(),
            paths: vec![track],
            at: None,
        }),
        Ok(1)
    );
    for _ in 0..200 {
        let _ = count(&mut source);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(count(&mut source), 2, "one add is one item");
}

/// A folder of files with subfolders adds the files of the subfolders too.
#[test]
fn adding_a_folder_brings_its_subfolders() {
    let d = dir();
    let music = d.join("Albums");
    let album = music.join("First Album");
    std::fs::create_dir_all(&album).unwrap();
    wav(&music, "loose", 5.0);
    wav(&album, "track", 5.0);
    let (mut source, clock) = source(Vec::new());
    let view = wait_for(&mut source, &clock, |_| true);
    let added = source.edit(Edit::AddFiles {
        playlist: view.main_playlist.clone(),
        paths: vec![music.to_string_lossy().into_owned()],
        at: None,
    });
    assert_eq!(added, Ok(2), "the file in the subfolder comes too");
}

/// Remove duplicates keeps the first item of each file and goes through undo.
#[test]
fn remove_duplicates_keeps_one_item_per_file_and_undoes() {
    use radio_ui_egui::view::ViewSource as _;
    let d = dir();
    let path = wav(&d, "twice", 5.0);
    let (mut source, clock) = source(Vec::new());
    let view = wait_for(&mut source, &clock, |_| true);
    let main = view.main_playlist.clone();
    for _ in 0..3 {
        let _ = source.edit(Edit::AddFiles {
            playlist: main.clone(),
            paths: vec![path.clone()],
            at: None,
        });
        let _ = source.snapshot(clock.now());
    }
    assert_eq!(source.duplicates(&main), 2);
    assert_eq!(
        source.edit(Edit::RemoveDuplicates {
            playlist: main.clone()
        }),
        Ok(2)
    );
    let view = wait_for(&mut source, &clock, |v| v.main.items.len() == 1);
    assert_eq!(view.main.items.len(), 1, "one item per file stays");
    assert_eq!(source.edit(Edit::Undo), Ok(1));
    let view = wait_for(&mut source, &clock, |v| v.main.items.len() == 3);
    assert_eq!(view.main.items.len(), 3, "undo brings them back");
}
