use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use egui_kittest::Harness;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::view::{Edit, MeterFrame, Row, StationView, ViewSource};
use radio_ui_egui::{MockSource, RadioApp};

/// The mock, counting the reads of the meters.
struct Counting {
    inner: MockSource,
    reads: Arc<AtomicUsize>,
}

impl ViewSource for Counting {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        self.inner.snapshot(now)
    }
    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        self.inner.edit(edit)
    }
    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
    fn meters(&self) -> Option<MeterFrame> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.meters()
    }
}

/// The meters come from the source on every frame, not only with the snapshot of each second.
#[test]
fn the_meters_are_read_once_per_frame() {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
    let reads = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&reads);
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 860.0))
        .build_eframe(move |_cc| {
            let source = Counting {
                inner: MockSource::new(tz),
                reads: count,
            };
            RadioApp::new(Box::new(source), Box::new(move || now))
        });
    harness.run_steps(2);
    let before = reads.load(Ordering::Relaxed);
    let frames = 10;
    harness.run_steps(frames);
    assert_eq!(reads.load(Ordering::Relaxed) - before, frames);
}

/// A step from silence to -6 dBFS shows on the same frame; after the sound stops, the bar
/// falls by at least 20 dB within 200 ms of frame time, at 60 and at 144 frames per second.
#[test]
fn a_meter_attacks_at_once_and_releases_fast() {
    use radio_ui_egui::view::Ballistics;
    for fps in [60.0_f32, 144.0] {
        let dt = 1.0 / fps;
        let silent = [-120.0; 2];
        let mut shown = Ballistics::default();
        shown.step(silent, silent, dt);
        shown.step([-6.0; 2], [-6.0; 2], dt);
        assert!(
            shown.level.iter().all(|l| (l - -6.0).abs() < 0.01),
            "the attack is not instant at {fps} fps: {:?}",
            shown.level
        );
        let mut t = 0.0;
        while t < 0.2 {
            shown.step(silent, silent, dt);
            t += dt;
        }
        assert!(
            shown.level.iter().all(|l| *l <= -26.0),
            "the bar fell too little in 200 ms at {fps} fps: {:?}",
            shown.level
        );
        assert!(
            shown.hold.iter().all(|h| (h - -6.0).abs() < 0.01),
            "the peak mark must hold for 1.5 s"
        );
    }
}
