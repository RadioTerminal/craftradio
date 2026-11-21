use radio_ui_egui::folders::list_audio;
use radio_ui_egui::view::FolderSource;
use radio_ui_egui::{MockSource, ViewSource};

#[test]
fn a_playlist_from_a_folder_of_three_audio_files_has_three_items() {
    let dir = std::env::temp_dir().join(format!("radio-folder-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for name in ["a.mp3", "b.flac", "c.wav", "notes.txt"] {
        std::fs::write(dir.join(name), b"").expect("temp file");
    }
    let files = list_audio(&dir, false).expect("the folder can be read");
    let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
    let mut source = MockSource::new(tz);
    let folder = FolderSource {
        path: dir.display().to_string(),
        recursive: false,
        watch: false,
    };
    let name = source
        .playlist_from_folder(folder, files)
        .expect("a playlist");
    let view = source.snapshot("2026-11-21T13:58:00Z".parse().expect("ts"));
    let tab = view
        .tabs
        .iter()
        .find(|t| t.name == name)
        .expect("the new tab");
    assert_eq!(tab.items.len(), 3, "only the audio files become items");
    assert!(tab.source.is_some(), "the tab knows its folder");
    let _ = std::fs::remove_dir_all(&dir);
}
