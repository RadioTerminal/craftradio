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
