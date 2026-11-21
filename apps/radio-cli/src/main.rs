//! `radio-cli`: command-line access to the radio station planner.

use clap::{Parser, Subcommand};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use radio_model::{Item, ItemId, ModelError, Station};
use radio_scheduler::{Entry, PlanInput, PlanOptions, PlayHistory, Timeline, Warning, plan};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "radio-cli", about = "Radio station manager command line")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the on-air timeline a station file produces.
    Plan {
        /// Station definition (TOML).
        station: PathBuf,
        /// Start of the plan as RFC 3339. Defaults to the current time.
        #[arg(long)]
        now: Option<Timestamp>,
        /// How many hours to plan.
        #[arg(long, default_value_t = 24)]
        hours: u64,
        /// Print the timeline as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<u8, String> {
    match cli.command {
        Command::Plan {
            station,
            now,
            hours,
            json,
        } => {
            let text = std::fs::read_to_string(&station)
                .map_err(|e| format!("cannot read {}: {e}", station.display()))?;
            let station: Station =
                toml::from_str(&text).map_err(|e| format!("cannot parse station file: {e}"))?;
            station.validate().map_err(|e| describe(&station, &e))?;
            // The command line is the only place that reads the clock.
            let now = now.unwrap_or_else(Timestamp::now);
            let history = PlayHistory::new();
            let timeline = plan(
                &PlanInput {
                    station: &station,
                    history: &history,
                    now,
                    horizon: Duration::from_secs(hours.saturating_mul(3600)),
                    pinned: &[],
                },
                &PlanOptions::default(),
                None,
            );
            if json {
                let out = serde_json::to_string_pretty(&timeline)
                    .map_err(|e| format!("cannot serialise timeline: {e}"))?;
                println!("{out}");
            } else {
                print_text(&station, &timeline);
            }
            for w in &timeline.warnings {
                eprintln!("warning: {w:?}");
            }
            Ok(exit_code(&timeline))
        }
    }
}

/// 2 when the plan has dead air or a fixed event outside its tolerance, else 0.
fn exit_code(timeline: &Timeline) -> u8 {
    let bad = timeline.warnings.iter().any(|w| {
        matches!(
            w,
            Warning::DeadAir { .. } | Warning::OutsideTolerance { .. }
        )
    });
    if bad { 2 } else { 0 }
}

/// The model's message, with the item's title when the error names an item.
fn describe(station: &Station, error: &ModelError) -> String {
    let id = match error {
        ModelError::BadTiming { item, .. } | ModelError::MissingPlannedLen(item) => Some(*item),
        _ => None,
    };
    match id.and_then(|id| find_item(station, id)) {
        Some(item) => format!("{error} (\"{}\")", item.title),
        None => error.to_string(),
    }
}

fn find_item(station: &Station, id: ItemId) -> Option<&Item> {
    station
        .playlists
        .iter()
        .flat_map(|p| &p.items)
        .find(|i| i.id == id)
}

fn print_text(station: &Station, timeline: &Timeline) {
    let tz = TimeZone::get(&station.tz).unwrap_or(TimeZone::UTC);
    for entry in &timeline.entries {
        println!("{}", entry_line(station, &tz, entry));
        if let Some(fixed) = &entry.fixed
            && let Some(fit) = timeline
                .fits
                .iter()
                .find(|f| f.playlist == entry.playlist && f.target == fixed.target)
        {
            let mark = if fit.within_tolerance { '✓' } else { '✗' };
            println!("    fit: {:+.1} s {mark}", fit.error.as_secs_f64());
        }
    }
}

fn entry_line(station: &Station, tz: &TimeZone, entry: &Entry) -> String {
    let local: Zoned = entry.start.to_zoned(tz.clone());
    let playlist = station
        .playlists
        .iter()
        .find(|p| p.id == entry.playlist)
        .map_or("?", |p| p.name.as_str());
    let what = match find_item(station, entry.item) {
        Some(Item {
            artist: Some(artist),
            title,
            ..
        }) => format!("{artist} – {title}"),
        Some(item) => item.title.clone(),
        None => format!("{:?}", entry.item),
    };
    let secs = entry.len.as_secs();
    let marker = if entry.fixed.is_some() { "⏱" } else { " " };
    format!(
        "{marker} {}  {playlist:<10}  {what}  {}:{:02}",
        local.strftime("%H:%M:%S"),
        secs / 60,
        secs % 60
    )
}
