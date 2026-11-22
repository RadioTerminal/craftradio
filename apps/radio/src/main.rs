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
use radio_ui_egui::{AppPrefs, MockSource, RadioApp, ToolWindow, ViewSource};
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
    /// Stream to listeners on this port (TCP and UDP), such as 8000 or 0.0.0.0:8000, for this
    /// run. Without it, the switch in the status line turns streaming on, on port 8000.
    #[arg(long, value_parser = parse_listen)]
    webrtc: Option<std::net::SocketAddr>,
    /// Listen for the CLI and MCP on this loopback port (docs/control.md). Without it, the
    /// control channel takes port 17470 or the next free one.
    #[arg(long, conflicts_with = "no_control")]
    control: Option<u16>,
    /// Start no control channel: the CLI and MCP cannot reach this station.
    #[arg(long)]
    no_control: bool,
}

/// A port such as `8000` (all addresses), or an address such as `127.0.0.1:8000`.
fn parse_listen(text: &str) -> Result<std::net::SocketAddr, String> {
    if let Ok(port) = text.parse::<u16>() {
        return Ok(std::net::SocketAddr::from(([0, 0, 0, 0], port)));
    }
    text.parse()
        .map_err(|_| format!("not a port or address: {text}"))
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
    window(args, Box::new(source), clock, "Radio — UI preview", None)
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
    if let Some(hold) = radio_ui_engine::peak_hold(engine.state()) {
        cfg.mixer.peak_hold = hold;
    }
    // The switch in the status line (the `webrtc.enabled` setting) starts streaming.
    // With `--webrtc`, streaming runs for this run in any case.
    let mut webrtc =
        radio_stream::WebRtcConfig::new(args.webrtc.unwrap_or(([0, 0, 0, 0], 8000).into()));
    webrtc.identity = station_identity(&dir);
    cfg.webrtc = Some(webrtc);
    cfg.webrtc_always = args.webrtc.is_some();
    cfg.station_name = STATION_NAME.to_owned();
    let fallback = dir.join("Fallback");
    cfg.fallback_folder = fallback.is_dir().then_some(fallback);
    let playout = Playout::start(engine, cfg).map_err(|e| format!("the playout failed: {e}"))?;
    println!("radio: station at {}", dir.join("station.db").display());
    let control = control_channel(args, &playout);
    if args.headless {
        return headless(playout, control, args.run_for, tz);
    }
    let mut source = EngineSource::new(
        playout,
        SourceOptions {
            tz,
            station_name: STATION_NAME.to_owned(),
        },
    )
    .with_secrets(Arc::new(secrets));
    let bridge = control.as_ref().map(window_bridge);
    if let Some(control) = control {
        source = source.with_companion(Box::new(control));
    }
    window(
        args,
        Box::new(source),
        Box::new(Timestamp::now),
        "Radio",
        bridge,
    )
}

/// The window's side of the control channel: its `ui.screenshot` requests, handed to the
/// window, which answers with a PNG.
struct WindowBridge {
    requests: std::sync::mpsc::Receiver<radio_ui_egui::ScreenshotRequest>,
    /// The window's context, to wake it for a request once the window exists.
    ctx: Arc<std::sync::OnceLock<eframe::egui::Context>>,
}

/// Attaches the window to `control`. A thread turns each `ui.screenshot` into a request to the
/// window and its PNG into the channel's answer; it ends with the channel.
fn window_bridge(control: &radio_control::Control) -> WindowBridge {
    use base64::Engine as _;
    let ctx: Arc<std::sync::OnceLock<eframe::egui::Context>> = Arc::default();
    let wake = {
        let ctx = Arc::clone(&ctx);
        move || {
            if let Some(ctx) = ctx.get() {
                ctx.request_repaint();
            }
        }
    };
    let incoming = control.attach_window(wake.clone());
    let (to_window, requests) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("control-window".into())
        .spawn(move || {
            for req in incoming {
                let (reply, shot) = std::sync::mpsc::channel();
                let sent = to_window
                    .send(radio_ui_egui::ScreenshotRequest { reply })
                    .is_ok();
                wake();
                let answer = match shot.recv_timeout(Duration::from_secs(9)) {
                    _ if !sent => serde_json::json!({"ok": false, "error": "the window is closed"}),
                    Ok(Ok(s)) => serde_json::json!({"ok": true, "result": {
                        "mime": "image/png",
                        "png": base64::engine::general_purpose::STANDARD.encode(&s.png),
                        "width": s.width,
                        "height": s.height,
                    }}),
                    Ok(Err(e)) => serde_json::json!({"ok": false, "error": e}),
                    Err(_) => {
                        serde_json::json!({"ok": false, "error": "the window did not capture"})
                    }
                };
                let _ = req.reply.send(answer);
            }
        });
    if let Err(e) = spawned {
        eprintln!("radio: no screenshots over the control channel: {e}");
    }
    WindowBridge { requests, ctx }
}

/// Starts the control channel of the CLI and MCP, unless `--no-control`. A channel that
/// cannot start is reported, and the station runs without it.
fn control_channel(args: &Args, playout: &Playout) -> Option<radio_control::Control> {
    if args.no_control {
        println!(
            "radio: no control channel (--no-control): the CLI and MCP cannot reach this station"
        );
        return None;
    }
    let sender = playout.command_sender()?;
    match radio_control::start(sender, args.control) {
        Ok(control) => {
            println!("radio: control channel on 127.0.0.1:{}", control.port());
            Some(control)
        }
        Err(e) => {
            eprintln!("radio: no control channel: {e}; the CLI and MCP cannot reach this station");
            None
        }
    }
}

/// Runs the station without a window, and prints what an operator should know.
fn headless(
    playout: Playout,
    control: Option<radio_control::Control>,
    run_for: Option<Duration>,
    tz: jiff::tz::TimeZone,
) -> Result<(), String> {
    let alarms = playout.alarms().map_err(|e| e.to_string())?;
    let view = playout.snapshot();
    println!("radio: output {:?}", view.outputs.local);
    let mut webrtc = None;
    let mut source = EngineSource::new(
        playout,
        SourceOptions {
            tz,
            station_name: STATION_NAME.to_owned(),
        },
    );
    if let Some(control) = control {
        source = source.with_companion(Box::new(control));
    }
    let _ = source.snapshot(Timestamp::now());
    println!("radio: folder watches started");
    let until = run_for.map(|d| Instant::now() + d);
    let mut gains: [Option<(u64, radio_model::ItemGain)>; 4] = Default::default();
    loop {
        let _ = source.snapshot(Timestamp::now());
        for notice in source.take_notices() {
            println!("radio: {notice}");
        }
        let now_webrtc = source.playout().snapshot().outputs.webrtc;
        if now_webrtc != webrtc {
            if let Some(status) = &now_webrtc {
                let was_listening =
                    matches!(webrtc, Some(radio_stream::WebRtcStatus::Listening { .. }));
                print_webrtc(status, !was_listening);
            }
            webrtc = now_webrtc;
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

/// The station key for the listen link. The first start makes it; later starts read it,
/// so the link stays the same.
fn station_identity(dir: &std::path::Path) -> Option<Vec<u8>> {
    let path = dir.join("webrtc-identity.pk8");
    if let Ok(key) = std::fs::read(&path)
        && radio_stream::Identity::new(&key).is_some()
    {
        return Some(key);
    }
    let key = radio_stream::Identity::generate()?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &key))
    {
        Ok(()) => Some(key),
        Err(e) => {
            eprintln!(
                "radio: the station key could not be saved ({e}); the listen link changes on each start"
            );
            Some(key)
        }
    }
}

/// Prints the WebRTC state, with the addresses that listeners open.
fn print_webrtc(status: &radio_stream::WebRtcStatus, addresses: bool) {
    use radio_stream::WebRtcStatus;
    match status {
        WebRtcStatus::Starting => {
            println!("radio: WebRTC starting (asking STUN for the public address)")
        }
        WebRtcStatus::Listening {
            url,
            public,
            listeners,
            link,
            relays,
            mapped,
            ..
        } => {
            println!("radio: WebRTC listening, {listeners} listeners, {relays} trackers connected");
            match mapped {
                Some(m) => {
                    println!("radio: the router forwards {m} to the station (UPnP or NAT-PMP)")
                }
                None if addresses => println!(
                    "radio: the router forwards no port (no UPnP or NAT-PMP yet): phones on mobile data may not connect"
                ),
                None => {}
            }
            if !addresses {
                return;
            }
            if let Some(link) = link {
                println!("radio: listen from anywhere: {link}");
            }
            println!("radio: listen on this network: {url}");
            if public.is_none() {
                println!(
                    "radio: WebRTC has no public address: no STUN server answered, so only this network can listen"
                );
            }
        }
        WebRtcStatus::Failed(e) => println!("radio: WebRTC failed: {e}"),
        WebRtcStatus::Stopped => println!("radio: WebRTC stopped"),
    }
}

fn window(
    args: &Args,
    source: Box<dyn ViewSource>,
    clock: Box<dyn Fn() -> Timestamp>,
    title: &str,
    bridge: Option<WindowBridge>,
) -> Result<(), String> {
    let single_window = args.single_window;
    let perf = args.perf;
    // The preview saves nothing; the station keeps this computer's preferences in config.toml.
    let prefs = if args.demo {
        AppPrefs::memory()
    } else {
        let (prefs, problem) = AppPrefs::open();
        if let Some(problem) = problem {
            eprintln!("radio: {problem}");
        }
        prefs
    };
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
            // Number fields write and read numbers the way the system's region does.
            radio_ui_egui::numbers::install_system(&cc.egui_ctx);
            let mut app = RadioApp::new(source, clock)
                .with_perf(perf)
                .with_preferences(prefs)
                .with_updates();
            if let Some(bridge) = bridge {
                let _ = bridge.ctx.set(cc.egui_ctx.clone());
                app = app.with_screenshots(bridge.requests);
            }
            for w in open {
                app.set_window(w, true);
            }
            // The menus go in the macOS menu bar; elsewhere, or if AppKit refuses them, the
            // window draws them.
            #[cfg(target_os = "macos")]
            if !app.install_native_menu(&cc.egui_ctx) {
                eprintln!("radio: the macOS menu bar refused the menus; they show in the window");
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("the window failed: {e}"))
}
