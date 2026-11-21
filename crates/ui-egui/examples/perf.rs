//! Measure the frame time while the pointer moves over the main list, without a window.
//!
//! Usage: `cargo run --release -p radio-ui-egui --example perf -- [main rows]`
//! The default is 2000 main rows. The result is the mean and the worst frame time.

use std::time::Instant;

use egui_kittest::Harness;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::theme::size;
use radio_ui_egui::{MockSource, RadioApp, ViewSource};

const FRAMES: usize = 300;
const WARMUP: usize = 10;
/// Main rows per hour of the mock, roughly. It decides how many hours give the rows asked for.
const MAIN_ROWS_PER_HOUR: usize = 12;

fn main() {
    let want: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(2000);
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
    let hours = i64::try_from(want.div_ceil(MAIN_ROWS_PER_HOUR)).unwrap_or(1);
    let mut probe = MockSource::with_hours(tz.clone(), hours);
    let view = probe.snapshot(now);
    let main = view.rows.iter().filter(|r| r.is_main).count();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(size::WINDOW_W, size::WINDOW_H))
        .build_eframe(move |_cc| {
            RadioApp::new(
                Box::new(MockSource::with_hours(tz, hours)),
                Box::new(move || now),
            )
        });
    harness.run_steps(WARMUP);
    // The pointer sweeps up and down over the main list.
    let top = 300.0;
    let bottom = size::WINDOW_H - 80.0;
    let mut times = Vec::with_capacity(FRAMES);
    for k in 0..FRAMES {
        let phase = (k % 100) as f32 / 100.0;
        let y = top + (bottom - top) * phase;
        harness.event(egui::Event::PointerMoved(egui::pos2(400.0, y)));
        let start = Instant::now();
        harness.step();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let worst = times.iter().copied().fold(0.0, f64::max);
    println!(
        "rows {} (main {main}) · frames {FRAMES} · mean {mean:.2} ms · worst {worst:.2} ms",
        view.rows.len()
    );
}
