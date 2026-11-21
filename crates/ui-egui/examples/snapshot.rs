//! Render the preview to PNG files without opening a window.
//!
//! Usage: `cargo run -p radio-ui-egui --example snapshot -- [out-dir] [RFC3339 instant]`
//! The defaults are `docs/screenshots` and `2026-11-21T13:58:00Z`.
//! Here egui draws tool windows as floating windows over the main window.

use std::process::ExitCode;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::theme::size;
use radio_ui_egui::{MockSource, RadioApp, ToolWindow};

/// Render at twice the point size, as a Retina display does.
const PIXELS_PER_POINT: f32 = 2.0;
const FRAMES: usize = 6;

/// What to open before a picture.
enum Scene {
    Main,
    Window(ToolWindow),
    Click(&'static [&'static str]),
    /// Right-click the lowest node with this label, then click the menu row, if any.
    Menu(&'static str, &'static str),
}

const ROW: &str = "Juniper Vale – Satellites of Ulvenik";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| "docs/screenshots".to_owned());
    let at = args
        .next()
        .unwrap_or_else(|| "2026-11-21T13:58:00Z".to_owned());
    let Ok(now) = at.parse::<Timestamp>() else {
        eprintln!("snapshot: {at:?} is not an RFC 3339 instant");
        return ExitCode::FAILURE;
    };
    let Ok(tz) = TimeZone::get("Europe/Ljubljana") else {
        eprintln!("snapshot: no time zone database");
        return ExitCode::FAILURE;
    };
    let scenes = [
        ("ui-preview", Scene::Main),
        ("cartwall", Scene::Window(ToolWindow::Cartwall)),
        ("mixer", Scene::Window(ToolWindow::Mixer)),
        (
            "mixer-settings",
            Scene::Click(&["Mixer", "⚙ Mixer settings"]),
        ),
        ("clock", Scene::Window(ToolWindow::Clock)),
        ("history", Scene::Window(ToolWindow::History)),
        ("library", Scene::Window(ToolWindow::Library)),
        ("rules-editor", Scene::Click(&["⚙ Music rules"])),
        ("rules-script", Scene::Click(&["⚙ Music rules", "Script"])),
        ("context-menu", Scene::Menu(ROW, "")),
        ("add-menu", Scene::Click(&["+ Add to Music"])),
        ("add-url", Scene::Click(&["+ Add to Music", "Add URL…"])),
        ("metadata", Scene::Menu(ROW, "Edit metadata")),
    ];
    for (name, scene) in scenes {
        let tz = tz.clone();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(size::WINDOW_W, size::WINDOW_H))
            .with_pixels_per_point(PIXELS_PER_POINT)
            .wgpu()
            .build_eframe(move |_cc| {
                RadioApp::new(Box::new(MockSource::new(tz)), Box::new(move || now))
            });
        harness.run_steps(FRAMES);
        match scene {
            Scene::Main => {}
            Scene::Window(w) => harness.state_mut().set_window(w, true),
            Scene::Menu(row, item) => {
                let node = harness
                    .query_all_by_label(row)
                    .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()));
                if let Some(node) = node {
                    node.click_secondary();
                }
                harness.run_steps(FRAMES);
                if !item.is_empty() {
                    harness.get_by_label_contains(item).click();
                    harness.run_steps(FRAMES);
                }
            }
            Scene::Click(labels) => {
                for label in labels {
                    harness.get_by_label(label).click();
                    harness.run_steps(FRAMES);
                }
            }
        }
        harness.run_steps(FRAMES);
        let out = format!("{dir}/{name}.png");
        match harness.render().map(|image| image.save(&out)) {
            Ok(Ok(())) => println!("snapshot: wrote {out}"),
            Ok(Err(e)) => {
                eprintln!("snapshot: could not write {out}: {e}");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("snapshot: render failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
