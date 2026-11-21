use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_engine::{Command, Engine};
use radio_model::{Item, ItemId, PlaylistId, Source, Timing};
use radio_playout::{Clock, ManualClock, OutputChoice, Playout, PlayoutConfig};
use radio_ui_egui::view::{StationView, ViewSource};
use radio_ui_engine::{EngineSource, SourceOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const STATION: &str = "Test Radio";

pub fn t0() -> Timestamp {
    "2026-11-21T13:00:00Z".parse().unwrap()
}

/// A new temporary folder for this test.
pub fn dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "radio-ui-engine-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 48 kHz stereo WAV with a quiet tone, `secs` long.
pub fn wav(dir: &std::path::Path, name: &str, secs: f32) -> String {
    let path = dir.join(format!("{name}.wav"));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for n in 0..(secs * 48_000.0) as usize {
        let x = ((n as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
        w.write_sample(x).unwrap();
        w.write_sample(x).unwrap();
    }
    w.finalize().unwrap();
    path.to_string_lossy().into_owned()
}

/// A file item `secs` long with full cue points.
pub fn track(id: u64, title: &str, path: &str, secs: f32) -> Item {
    let len = Duration::from_secs_f32(secs);
    Item {
        id: ItemId(id),
        meta: radio_model::ItemMeta::default(),
        title: title.into(),
        artist: Some("Test Artist".into()),
        source: Source::File { path: path.into() },
        timing: Some(Timing {
            cue_in: Duration::ZERO,
            segue: len,
            cue_out: len,
            intro_end: None,
            fade_in: Duration::ZERO,
            fade_out: Duration::ZERO,
        }),
        planned_len: None,
        movable: true,
        trimmable: true,
        locked: false,
        valid_from: None,
        valid_to: None,
        max_per_day: None,
    }
}

/// A station with `items` in its main playlist, on a null output and a manual clock.
pub fn source(items: Vec<Item>) -> (EngineSource, ManualClock) {
    let mut engine = Engine::open_in_memory().unwrap();
    if !items.is_empty() {
        engine
            .dispatch(Command::PlaylistAddItems {
                playlist: PlaylistId(1),
                items,
                at: None,
            })
            .unwrap();
    }
    let clock = ManualClock::new(t0());
    let mut cfg = PlayoutConfig::new(Arc::new(clock.clone()), OutputChoice::Null);
    cfg.horizon = Duration::from_secs(600);
    cfg.station_name = STATION.into();
    let playout = Playout::start(engine, cfg).unwrap();
    let opts = SourceOptions {
        tz: TimeZone::UTC,
        station_name: STATION.into(),
    };
    (EngineSource::new(playout, opts), clock)
}

/// Takes snapshots until `done` holds, for at most five seconds.
pub fn wait_for(
    source: &mut EngineSource,
    clock: &ManualClock,
    done: impl Fn(&StationView) -> bool,
) -> StationView {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let view = source.snapshot(clock.now());
        if done(&view) || Instant::now() > deadline {
            return view;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
