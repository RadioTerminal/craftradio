//! The desktop app. This is the only place that reads the clock.

use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use radio_ui_egui::theme::size;
use radio_ui_egui::{MockSource, RadioApp};

/// Radio station manager (UI preview with mock data, no audio).
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Start the clock at this instant (RFC 3339), then let it run.
    #[arg(long)]
    now: Option<Timestamp>,
    /// The station time zone (IANA name).
    #[arg(long, default_value = "Europe/Ljubljana")]
    tz: String,
    /// Show tool windows inside the main window, so that drag and drop works between all of them.
    #[arg(long)]
    single_window: bool,
    /// Show the time of each frame in the status line.
    #[arg(long)]
    perf: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let single_window = args.single_window;
    let perf = args.perf;
    let tz = match TimeZone::get(&args.tz) {
        Ok(tz) => tz,
        Err(e) => {
            eprintln!("radio: unknown time zone {:?}: {e}", args.tz);
            return ExitCode::FAILURE;
        }
    };
    let clock: Box<dyn Fn() -> Timestamp> = match args.now {
        Some(start) => {
            let launched = Instant::now();
            Box::new(move || {
                let run = SignedDuration::try_from(launched.elapsed()).unwrap_or_default();
                start.checked_add(run).unwrap_or(start)
            })
        }
        None => Box::new(Timestamp::now),
    };
    println!("radio: UI preview with mock data. Audio output is not connected.");

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Radio — UI preview")
            .with_inner_size([size::WINDOW_W, size::WINDOW_H])
            .with_min_inner_size([size::WINDOW_MIN_W, size::WINDOW_MIN_H]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "radio",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_embed_viewports(single_window);
            Ok(Box::new(
                RadioApp::new(Box::new(MockSource::new(tz)), clock).with_perf(perf),
            ))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radio: the window failed: {e}");
            ExitCode::FAILURE
        }
    }
}
