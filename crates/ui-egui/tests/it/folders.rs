use radio_ui_egui::view::{Edit, FolderSource};
use radio_ui_egui::{MockSource, ViewSource};

#[test]
fn a_playlist_from_a_folder_of_three_audio_files_has_three_items() {
    let dir = std::env::temp_dir().join(format!("radio-folder-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for name in ["a.mp3", "b.flac", "c.wav", "notes.txt"] {
        std::fs::write(dir.join(name), b"").expect("temp file");
    }
    let path = dir.display().to_string();
    let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
    let mut source = MockSource::new(tz);
    let now = "2026-11-21T13:58:00Z".parse().expect("ts");
    source.snapshot(now);
    let read = source.edit(Edit::AddFolder { path: path.clone() });
    assert_eq!(read, Ok(3), "the source reads only the audio files");
    let folder = FolderSource {
        path: path.clone(),
        recursive: false,
        watch: false,
    };
    let name = source.playlist_from_folder(folder).expect("a playlist");
    let view = source.snapshot(now);
    let listed = view
        .folders
        .iter()
        .find(|f| f.path == path)
        .expect("the folder shows");
    assert_eq!(listed.files.len(), 3);
    assert!(!listed.truncated);
    let tab = view
        .tabs
        .iter()
        .find(|t| t.name == name)
        .expect("the new tab");
    assert_eq!(tab.items.len(), 3, "only the audio files become items");
    assert!(tab.source.is_some(), "the tab knows its folder");
    let _ = std::fs::remove_dir_all(&dir);
}
