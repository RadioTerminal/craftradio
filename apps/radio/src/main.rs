//! The desktop app. This is the only place that reads the clock.
//!
//! By default it runs the real station: the engine on `<data dir>/station.db` and the
//! playout on the default sound card. `--demo` shows the UI preview with mock data.

mod station;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use radio_playout::{OutputChoice, Playout, PlayoutConfig, SystemClock};
use radio_ui_egui::theme::size;
use radio_ui_egui::{MockSource, RadioApp, ToolWindow, ViewSource};
use radio_ui_engine::{EngineSource, KEYCHAIN_SERVICE, KeyringStore, SourceOptions};

/// The stream title of items that hide their own title.
const STATION_NAME: &str = "Radio";
/// The folder of the app in the platform data folder.
const APP_DIR: &str = "radio-station-manager";

/// Radio station manager.
#[derive(Parser)]
#[command(version, name = "craftradio")]
struct Args {
    /// Show the UI preview with mock data. It plays no audio and saves nothing.
    #[arg(long)]
    demo: bool,
    /// Run without a window.
    #[arg(long)]
    headless: bool,
    /// Stop after this time, such as 5s, 2m or 1h (for tests and checks).
    #[arg(long, value_parser = parse_duration)]
    run_for: Option<Duration>,
    /// On the first run, import this station file instead of the example station.
    #[arg(long)]
    station: Option<PathBuf>,
    /// Keep the station here instead of the platform data folder.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Play to no sound card; a timer drives the mixer.
    #[arg(long)]
    null_output: bool,
    /// Demo only: start the clock at this instant (RFC 3339), then let it run.
    #[arg(long)]
    now: Option<Timestamp>,
    /// Demo only: the station time zone (IANA name).
    #[arg(long, default_value = "Europe/Ljubljana")]
    tz: String,
    /// Show tool windows inside the main window, so that drag and drop works between all of them.
    #[arg(long)]
    single_window: bool,
    /// Show the time of each frame in the status line, and print it every 5 s.
    #[arg(long)]
    perf: bool,
    /// Open this tool window at the start: cartwall, mixer, clock, history or library.
    #[arg(long)]
    open: Vec<String>,
}

/// A length such as `500ms`, `5s`, `2m` or `1h`. A bare number counts seconds.
fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (num, unit) = text.split_at(split);
    let n: f64 = num.parse().map_err(|_| format!("not a length: {text}"))?;
    let secs = match unit {
        "" | "s" => n,
        "ms" => n / 1000.0,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        _ => return Err(format!("unknown unit {unit:?}; use ms, s, m or h")),
    };
    Duration::try_from_secs_f64(secs).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let args = Args::parse();
    let result = if args.demo { demo(&args) } else { real(&args) };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radio: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The clock of the demo: the system clock, or a clock that starts at `--now`.
fn demo_clock(now: Option<Timestamp>) -> Box<dyn Fn() -> Timestamp> {
    match now {
        Some(start) => {
            let launched = Instant::now();
            Box::new(move || {
                let run = SignedDuration::try_from(launched.elapsed()).unwrap_or_default();
                start.checked_add(run).unwrap_or(start)
            })
        }
        None => Box::new(Timestamp::now),
    }
}

fn demo(args: &Args) -> Result<(), String> {
    let tz =
        TimeZone::get(&args.tz).map_err(|e| format!("unknown time zone {:?}: {e}", args.tz))?;
    let clock = demo_clock(args.now);
    println!("radio: UI preview with mock data. Audio output is not connected.");
    let mut source = MockSource::new(tz);
    if args.headless {
        let until = args.run_for.map(|d| Instant::now() + d);
        let view = source.snapshot(clock());
        println!("radio: preview with {} rows, no audio", view.rows.len());
        while until.is_none_or(|u| Instant::now() < u) {
            std::thread::sleep(Duration::from_millis(100));
            let _ = source.snapshot(clock());
        }
        return Ok(());
    }
    window(args, Box::new(source), clock, "Radio — UI preview")
}

fn real(args: &Args) -> Result<(), String> {
    let dir = match &args.data_dir {
        Some(d) => d.clone(),
        None => dirs::data_dir()
            .ok_or("the platform has no data folder; pass --data-dir")?
            .join(APP_DIR),
    };
    let (mut engine, seeded) = station::open(&dir, args.station.as_deref())?;
    for line in station::reconcile(&mut engine) {
        println!("radio: {line}");
    }
    // Seeding and catching up are no edits of a person, so Undo starts empty.
    engine.clear_history();
    match &seeded {
        station::Seeded::No if args.station.is_some() => {
            eprintln!("radio: the station exists already; --station applies to a first run only");
        }
        station::Seeded::No => {}
        station::Seeded::Example { music, kept } => println!(
            "radio: first run: the example station, {kept} items found. Put audio files in {}",
            music.display()
        ),
        station::Seeded::File { path, items } => println!(
            "radio: first run: imported {} with {items} items",
            path.display()
        ),
    }
    let tz = TimeZone::get(&engine.state().station.tz).unwrap_or(TimeZone::UTC);
    let secrets = KeyringStore::new(KEYCHAIN_SERVICE);
    let icecast = match radio_ui_engine::icecast_config(engine.state(), &secrets, STATION_NAME) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("radio: Icecast stays off: {e}");
            None
        }
    };
    let device = engine
        .state()
        .settings(radio_ui_engine::AUDIO_PART)
        .and_then(|s| s.get("out"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let output = if args.null_output {
        OutputChoice::Null
    } else {
        OutputChoice::Device { name: device }
    };
    let mut cfg = PlayoutConfig::new(Arc::new(SystemClock), output);
    cfg.icecast = icecast;
    cfg.station_name = STATION_NAME.to_owned();
    let fallback = dir.join("Fallback");
    cfg.fallback_folder = fallback.is_dir().then_some(fallback);
    let playout = Playout::start(engine, cfg).map_err(|e| format!("the playout failed: {e}"))?;
    println!("radio: station at {}", dir.join("station.db").display());
    if args.headless {
        return headless(playout, args.run_for, tz);
    }
    let source = EngineSource::new(
        playout,
        SourceOptions {
            tz,
            station_name: STATION_NAME.to_owned(),
        },
    )
    .with_secrets(Arc::new(secrets));
    window(args, Box::new(source), Box::new(Timestamp::now), "Radio")
}

/// Runs the station without a window, and prints what an operator should know.
fn headless(
    playout: Playout,
    run_for: Option<Duration>,
    tz: jiff::tz::TimeZone,
) -> Result<(), String> {
    let alarms = playout.alarms().map_err(|e| e.to_string())?;
    let view = playout.snapshot();
    println!("radio: output {:?}", view.outputs.local);
    let mut source = EngineSource::new(
        playout,
        SourceOptions {
            tz,
            station_name: STATION_NAME.to_owned(),
        },
    );
    let _ = source.snapshot(Timestamp::now());
    println!("radio: folder watches started");
    let until = run_for.map(|d| Instant::now() + d);
    let mut gains: [Option<(u64, radio_model::ItemGain)>; 4] = Default::default();
    loop {
        let _ = source.snapshot(Timestamp::now());
        for notice in source.take_notices() {
            println!("radio: {notice}");
        }
        // The item gain that playout sent to each deck, when it changes.
        let view = source.playout().snapshot();
        for (i, d) in view.decks.iter().enumerate() {
            let now = d.item.zip(d.gain).map(|(id, g)| (id.0, g));
            if now != gains[i] {
                if let Some((_, g)) = now {
                    let loudness = g
                        .loudness
                        .map_or("unknown".to_owned(), |l| format!("{:.2} LUFS", l.lufs));
                    println!(
                        "radio: deck {} {:?}: gain {:+.2} dB ({:?}, {loudness}, target {:.1} LUFS, clamp {:?})",
                        ['A', 'B', 'C', 'D'][i],
                        d.title,
                        g.db,
                        g.source,
                        g.target_lufs,
                        g.clamp
                    );
                }
                gains[i] = now;
            }
        }
        let wait = match until {
            Some(u) => match u.checked_duration_since(Instant::now()) {
                Some(left) => left.min(Duration::from_millis(200)),
                None => break,
            },
            None => Duration::from_millis(200),
        };
        if let Ok(a) = alarms.recv_timeout(wait) {
            println!("radio: alarm {:?}", a.alarm);
        }
    }
    let view = source.playout().snapshot();
    println!("radio: output {:?}", view.outputs.local);
    let playing = view
        .decks
        .iter()
        .filter(|d| d.phase == radio_playout::DeckPhase::Playing)
        .count();
    println!(
        "radio: {playing} decks playing, {} planned entries",
        view.timeline.entries.len()
    );
    drop(source);
    Ok(())
}

fn window(
    args: &Args,
    source: Box<dyn ViewSource>,
    clock: Box<dyn Fn() -> Timestamp>,
    title: &str,
) -> Result<(), String> {
    let single_window = args.single_window;
    let perf = args.perf;
    let open: Vec<ToolWindow> = ToolWindow::ALL
        .into_iter()
        .filter(|w| args.open.iter().any(|o| o.eq_ignore_ascii_case(w.title())))
        .collect();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(title)
            // Matches the desktop file and icon name in packaging/linux, so docks show the icon.
            .with_app_id("io.github.radioterminal.craftradio")
            .with_inner_size([size::WINDOW_W, size::WINDOW_H])
            .with_min_inner_size([size::WINDOW_MIN_W, size::WINDOW_MIN_H]),
        ..Default::default()
    };
    eframe::run_native(
        "radio",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_embed_viewports(single_window);
            let mut app = RadioApp::new(source, clock).with_perf(perf);
            for w in open {
                app.set_window(w, true);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("the window failed: {e}"))
}
