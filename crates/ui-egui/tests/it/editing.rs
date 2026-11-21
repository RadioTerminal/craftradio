use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::{MockSource, RadioApp, ToolWindow};

const FRAMES: usize = 4;
/// A main row with room before the next fixed item.
const ROW: &str = "Juniper Vale – Satellites of Ulvenik";

fn harness() -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz database has Ljubljana");
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(move || now))
        });
    harness.run_steps(FRAMES);
    harness
}

fn count(h: &Harness<'_, RadioApp>, label: &str) -> usize {
    h.query_all_by_label(label).count()
}

/// Right-click the main pane row `label`: the lowest match, below the deck cards.
fn right_click_row(h: &mut Harness<'_, RadioApp>, label: &str) {
    let node = h
        .query_all_by_label(label)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the row is on screen");
    node.click_secondary();
    h.run_steps(FRAMES);
}

fn menu(h: &mut Harness<'_, RadioApp>, item: &str) {
    h.get_by_label_contains(item).click();
    h.run_steps(FRAMES);
}

#[test]
fn copy_then_paste_below_adds_a_row() {
    let mut h = harness();
    let before = count(&h, ROW);
    right_click_row(&mut h, ROW);
    menu(&mut h, "Copy");
    right_click_row(&mut h, ROW);
    menu(&mut h, "Paste below");
    assert!(
        count(&h, ROW) > before,
        "the pasted copy shows as another row"
    );
}

#[test]
fn the_clipboard_is_shared_between_the_library_and_the_main_pane() {
    let mut h = harness();
    h.state_mut().set_window(ToolWindow::Library, true);
    h.run_steps(FRAMES);
    menu(&mut h, "Dock into main window");
    let pasted = "Mira Solenne – Lanterns of Kelmora";
    let before = count(&h, pasted);
    h.get_by_label("☰ Lanterns of Kelmora").click_secondary();
    h.run_steps(FRAMES);
    menu(&mut h, "Copy");
    // Close the library, so the main pane shows the row again at full height.
    menu(&mut h, "Undock to a window");
    h.state_mut().set_window(ToolWindow::Library, false);
    h.run_steps(FRAMES);
    right_click_row(&mut h, ROW);
    menu(&mut h, "Paste below");
    assert!(
        count(&h, pasted) > before,
        "the library copy pastes into the main pane"
    );
}

#[derive(Debug)]
struct File(PathBuf);

impl egui::DroppedFile for File {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}

#[test]
fn files_dropped_on_the_main_pane_become_rows_and_others_are_skipped() {
    let dir = std::env::temp_dir().join(format!("radio-drop-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let names = [
        "radio-drop-alpha.mp3",
        "radio-drop-beta.flac",
        "radio-drop-notes.txt",
    ];
    for n in names {
        std::fs::write(dir.join(n), b"").expect("temp file");
    }
    let mut h = harness();
    let node = h
        .query_all_by_label(ROW)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the row is on screen");
    node.hover();
    h.run_steps(FRAMES);
    for n in names {
        let file: egui::DroppedFileHandle = Arc::new(File(dir.join(n)));
        h.input_mut().dropped_files.push(file);
    }
    h.run_steps(FRAMES);
    assert!(count(&h, "radio-drop-alpha") > 0, "the mp3 file is a row");
    assert!(count(&h, "radio-drop-beta") > 0, "the flac file is a row");
    assert_eq!(
        count(&h, "radio-drop-notes"),
        0,
        "the text file is not a row"
    );
    assert!(
        h.query_by_label_contains("skipped 1 non-audio").is_some(),
        "the toast says that one file was skipped"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_url_puts_a_stream_into_the_playlist() {
    let mut h = harness();
    h.get_by_label("    Jingles").click();
    h.run_steps(FRAMES);
    let stream = "https://radio.example.org/live";
    assert_eq!(count(&h, stream), 0);
    h.get_by_label("+ Add to Jingles").click();
    h.run_steps(FRAMES);
    menu(&mut h, "Add URL…");
    let field = h.get_by_label("URL");
    field.focus();
    field.type_text(stream);
    h.run_steps(FRAMES);
    h.get_by_label("Add").click();
    h.run_steps(FRAMES);
    assert!(
        count(&h, stream) > 0,
        "the stream is an item of Jingles now"
    );
    assert!(
        h.query_by_label_contains("added 1 stream to Jingles")
            .is_some(),
        "the message shows the count from the source"
    );
}
