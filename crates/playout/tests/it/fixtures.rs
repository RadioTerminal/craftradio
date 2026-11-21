use jiff::Timestamp;
use radio_engine::{Command, Engine};
use radio_model::{Item, PlaylistId, Source, Timing};
use radio_playout::{ManualClock, OutputChoice, Playout, PlayoutConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub const SR: usize = 48_000;
/// One render step: 10 ms.
pub const STEP: usize = SR / 100;

/// Which channels a fixture tone sounds on, so tests can tell items apart in Program.
#[derive(Clone, Copy)]
pub enum Side {
    Left,
    Right,
    Both,
    Silent,
}

pub fn t0() -> Timestamp {
    "2026-11-21T13:00:00Z".parse().unwrap()
}

fn dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "radio-playout-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 48 kHz stereo WAV of a 440 Hz tone at -12 dBFS on `side`.
pub fn wav(name: &str, secs: f32, side: Side) -> String {
    let path = dir().join(format!("{name}.wav"));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for n in 0..(secs * SR as f32) as usize {
        let x = (n as f32 * 440.0 * std::f32::consts::TAU / SR as f32).sin() * 0.25;
        let (l, r) = match side {
            Side::Left => (x, 0.0),
            Side::Right => (0.0, x),
            Side::Both => (x, x),
            Side::Silent => (0.0, 0.0),
        };
        w.write_sample((l * 32767.0) as i16).unwrap();
        w.write_sample((r * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    path.to_string_lossy().into_owned()
}

fn secs(s: f32) -> Duration {
    Duration::from_secs_f32(s)
}

/// A file item that segues `segue` seconds in and ends at `len`, fading out over `fade`.
pub fn file_item(id: u64, title: &str, path: &str, len: f32, segue: f32, fade: f32) -> Item {
    let mut item = radio_testkit::track(id, title, "Test artist", 1);
    item.source = Source::File { path: path.into() };
    item.timing = Some(Timing {
        cue_in: Duration::ZERO,
        segue: secs(segue),
        cue_out: secs(len),
        intro_end: None,
        fade_in: Duration::ZERO,
        fade_out: secs(fade),
    });
    item
}

/// An engine whose main playlist holds `items`, in order.
pub fn engine_with(items: Vec<Item>) -> Engine {
    let mut engine = Engine::open_in_memory().unwrap();
    engine
        .dispatch(Command::PlaylistAddItems {
            playlist: PlaylistId(1),
            items,
            at: None,
            lock: true,
        })
        .unwrap();
    engine
}

/// An offline playout on a manual clock at [`t0`].
pub fn offline(engine: Engine, tune: impl FnOnce(&mut PlayoutConfig)) -> (Playout, ManualClock) {
    let clock = ManualClock::new(t0());
    let mut cfg = PlayoutConfig::new(Arc::new(clock.clone()), OutputChoice::Offline);
    // Fixture tracks last seconds, so a short horizon keeps each plan small.
    cfg.horizon = Duration::from_secs(60);
    tune(&mut cfg);
    (Playout::start(engine, cfg).unwrap(), clock)
}

/// Renders `secs` of Program in 10 ms steps, moving the clock in step.
pub fn run(p: &Playout, clock: &ManualClock, secs: f32) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..(secs * 100.0).round() as usize {
        out.extend(p.render(STEP).unwrap());
        clock.advance(Duration::from_millis(10));
    }
    out
}

/// The first frame from `from` on where channel `ch` is audible.
pub fn first_audible(program: &[f32], ch: usize, from: usize) -> Option<usize> {
    (from..program.len() / 2).find(|&f| program[f * 2 + ch].abs() > 0.01)
}

/// Adds a playlist named `name` with `items` and a rules `script`, and returns its id.
pub fn add_playlist(engine: &mut Engine, name: &str, items: Vec<Item>, script: &str) -> PlaylistId {
    let made = engine
        .dispatch(Command::PlaylistCreate { name: name.into() })
        .unwrap();
    let playlist = PlaylistId(made.data.and_then(|v| v.as_u64()).unwrap());
    engine
        .dispatch(Command::PlaylistAddItems {
            playlist,
            items,
            at: None,
            lock: true,
        })
        .unwrap();
    engine
        .dispatch(Command::PlaylistSetRulesScript {
            playlist,
            script: script.into(),
        })
        .unwrap();
    playlist
}

/// The loudest sample of channel `ch` in frames `from..to`.
pub fn peak(program: &[f32], ch: usize, from: usize, to: usize) -> f32 {
    (from..to.min(program.len() / 2))
        .map(|f| program[f * 2 + ch].abs())
        .fold(0.0, f32::max)
}
