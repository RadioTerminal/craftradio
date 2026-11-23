use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::view::{
    DeviceRole, Edit, MeterFrame, NO_DEVICE, Row, StationView, StripChange, ViewSource,
};
use radio_ui_egui::{MockSource, RadioApp};

const FRAMES: usize = 4;

/// The mock with an open monitor output, keeping each edit it gets.
struct Recording {
    inner: MockSource,
    edits: Arc<Mutex<Vec<Edit>>>,
}

impl ViewSource for Recording {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let mut view = self.inner.snapshot(now);
        view.monitor_open = true;
        view
    }
    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        if let Ok(mut edits) = self.edits.lock() {
            edits.push(edit.clone());
        }
        self.inner.edit(edit)
    }
    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
    fn meters(&self) -> Option<MeterFrame> {
        self.inner.meters()
    }
}

fn harness(edits: Arc<Mutex<Vec<Edit>>>) -> Harness<'static, RadioApp> {
    let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
    let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 860.0))
        .build_eframe(move |_cc| {
            let source = Recording {
                inner: MockSource::new(tz),
                edits,
            };
            RadioApp::new(Box::new(source), Box::new(move || now))
        });
    harness.run_steps(FRAMES);
    harness
}

fn click(harness: &mut Harness<'_, RadioApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(FRAMES);
}

#[test]
fn snd_toggle_sends_a_strip_edit() {
    let edits = Arc::default();
    let mut harness = harness(Arc::clone(&edits));
    click(&mut harness, "Mixer");
    click(&mut harness, "SND A");
    let edits = edits.lock().unwrap();
    assert!(edits.contains(&Edit::SetStrip {
        strip: "A".into(),
        change: StripChange::Snd(true),
    }));
}

#[test]
fn snd_on_program_sends_a_strip_edit_for_out() {
    let edits = Arc::default();
    let mut harness = harness(Arc::clone(&edits));
    click(&mut harness, "Mixer");
    click(&mut harness, "SND Program");
    assert!(edits.lock().unwrap().contains(&Edit::SetStrip {
        strip: "Out".into(),
        change: StripChange::Snd(true),
    }));
}

#[test]
fn a_down_output_strip_shows_its_state_in_words() {
    let mut harness = harness(Arc::default());
    click(&mut harness, "Mixer");
    for name in [
        "Sound card: Preview — no audio",
        "Icecast: Not connected",
        "WebRTC: Off",
    ] {
        assert!(
            harness.query_all_by_label(name).next().is_some(),
            "no strip says {name}"
        );
    }
}

#[test]
fn choosing_none_for_the_program_device_asks_for_no_sound_card() {
    let edits = Arc::default();
    let mut harness = harness(Arc::clone(&edits));
    click(&mut harness, "Mixer");
    click(&mut harness, "Mixer preferences");
    click(&mut harness, "Program device");
    click(&mut harness, "None");
    assert!(edits.lock().unwrap().contains(&Edit::SetDevice {
        role: DeviceRole::Out,
        device: NO_DEVICE.into(),
    }));
}

#[test]
fn the_program_cog_opens_the_devices_page() {
    let mut harness = harness(Arc::default());
    click(&mut harness, "Mixer");
    click(&mut harness, "Program preferences");
    assert!(
        harness
            .query_all_by_label("Monitor device (SND)")
            .next()
            .is_some(),
        "Devices shows the monitor device"
    );
}

#[test]
fn muting_program_asks_by_its_name() {
    let mut harness = harness(Arc::default());
    click(&mut harness, "Mixer");
    click(&mut harness, "Mute Program");
    assert!(
        harness
            .query_all_by_label_contains("Mute Program?")
            .next()
            .is_some()
    );
}
