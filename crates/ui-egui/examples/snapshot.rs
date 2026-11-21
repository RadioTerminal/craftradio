//! Render the preview window to a PNG without opening a window.
//!
//! Usage: `cargo run -p radio-ui-egui --example snapshot -- [out.png] [RFC3339 instant]`
//! The defaults are `docs/screenshots/ui-preview.png` and `2026-11-21T13:58:00Z`.

use std::process::ExitCode;

use egui_kittest::Harness;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::theme::size;
use radio_ui_egui::{MockSource, RadioApp};

/// Render at twice the point size, as a Retina display does.
const PIXELS_PER_POINT: f32 = 2.0;
const FRAMES: usize = 6;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .unwrap_or_else(|| "docs/screenshots/ui-preview.png".to_owned());
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
    let mut harness = Harness::builder()
        .with_size(egui::vec2(size::WINDOW_W, size::WINDOW_H))
        .with_pixels_per_point(PIXELS_PER_POINT)
        .wgpu()
        .build_eframe(move |_cc| {
            RadioApp::new(Box::new(MockSource::new(tz)), Box::new(move || now))
        });
    harness.run_steps(FRAMES);
    match harness.render().map(|image| image.save(&out)) {
        Ok(Ok(())) => {
            println!("snapshot: wrote {out}");
            ExitCode::SUCCESS
        }
        Ok(Err(e)) => {
            eprintln!("snapshot: could not write {out}: {e}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("snapshot: render failed: {e}");
            ExitCode::FAILURE
        }
    }
}
