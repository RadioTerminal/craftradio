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
