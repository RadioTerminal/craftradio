//! `radio-cli`: command-line access to the radio station planner.

use clap::{Parser, Subcommand};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_model::{Item, ItemId, ModelError, PlaylistId, Station};
use radio_scheduler::{
    DropReason, Entry, PlanInput, PlanOptions, PlayHistory, Timeline, Warning, plan,
};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    version,
    name = "craftradio-cli",
    about = "Radio station manager command line"
)]
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
    /// Run one engine command on the station in a folder, and print its outcome as JSON.
    ///
    /// The command is JSON: {"id": "schedule.add", "args": {...}}. `commands` lists the
    /// ids. Close the app first: a running app keeps its own copy of the station.
    Cmd {
        /// The folder that holds station.db: the --data-dir of the app.
        #[arg(long)]
        data_dir: PathBuf,
        /// The command as JSON.
        command: String,
    },
    /// List the engine commands: id, tier and summary.
    Commands,
}

fn main() -> ExitCode {
    // Clap's own error exit code (2) would collide with the dead-air code.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = u8::from(e.exit_code() != 0);
            let _ = e.print();
            return ExitCode::from(code);
        }
    };
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

/// Runs one engine command on `data_dir/station.db`. It never makes a new station.
fn run_command(data_dir: &std::path::Path, json: &str) -> Result<u8, String> {
    let db = data_dir.join("station.db");
    if !db.is_file() {
        return Err(format!(
            "no station at {}; start the app with --data-dir first",
            db.display()
        ));
    }
    let command: radio_engine::Command =
        serde_json::from_str(json).map_err(|e| format!("cannot read the command: {e}"))?;
    let mut engine = radio_engine::Engine::open(&db).map_err(|e| e.to_string())?;
    let outcome = engine.dispatch(command).map_err(|e| e.to_string())?;
    engine
        .flush()
        .map_err(|e| format!("the change is not saved: {e}"))?;
    let text = serde_json::to_string_pretty(&outcome)
        .map_err(|e| format!("cannot write the outcome: {e}"))?;
    writeln!(io::stdout().lock(), "{text}").map_err(|e| e.to_string())?;
    if let Some(note) = &outcome.note {
        eprintln!("note: {note}");
    }
    if outcome.audio_required && !outcome.applied_to_audio {
        eprintln!("note: no playout runs here, so no audio command reached the decks");
    }
    Ok(0)
}

fn run(cli: Cli) -> Result<u8, String> {
    match cli.command {
        Command::Cmd { data_dir, command } => run_command(&data_dir, &command),
        Command::Commands => {
            let mut out = io::stdout().lock();
            for spec in radio_engine::registry() {
                let tier = match spec.tier {
                    radio_engine::Tier::ReadOnly => "read-only",
                    radio_engine::Tier::OnAir => "on-air",
                };
                writeln!(out, "{:<28} {tier:<9} {}", spec.id, spec.summary)
                    .map_err(|e| e.to_string())?;
            }
            Ok(0)
        }
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
            let tz = TimeZone::get(&station.tz)
                .map_err(|_| format!("unknown time zone '{}'", station.tz))?;
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
            let printed = if json {
                serde_json::to_string_pretty(&timeline)
                    .map_err(|e| format!("cannot serialise timeline: {e}"))
                    .map(|out| writeln!(io::stdout().lock(), "{out}"))?
            } else {
                print_text(&station, &tz, &timeline)
            };
            match printed {
                Ok(()) => {}
                // `| head` closed the pipe: stop quietly.
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(exit_code(&timeline)),
                Err(e) => return Err(format!("cannot write output: {e}")),
            }
            let tz = TimeZone::get(&station.tz).unwrap_or(TimeZone::UTC);
            for w in &timeline.warnings {
                eprintln!("warning: {}", warning_text(&station, &tz, w));
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
    if let ModelError::UnknownTimeZone(tz) = error {
        return format!("unknown time zone '{tz}'");
    }
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

fn print_text(station: &Station, tz: &TimeZone, timeline: &Timeline) -> io::Result<()> {
    let mut out = io::stdout().lock();
    for entry in &timeline.entries {
        writeln!(out, "{}", entry_line(station, tz, entry))?;
        if let Some(fixed) = &entry.fixed {
            let fit = timeline
                .fits
                .iter()
                .find(|f| f.playlist == entry.playlist && f.target == fixed.target);
            match fit {
                Some(fit) => {
                    let mark = if fit.within_tolerance { '✓' } else { '✗' };
                    writeln!(out, "    fit: {:+.1} s {mark}", fit.error.as_secs_f64())?;
                }
                None => writeln!(out, "    fit: —")?,
            }
        }
    }
    Ok(())
}

fn clock(tz: &TimeZone, at: Timestamp) -> String {
    at.to_zoned(tz.clone()).strftime("%H:%M:%S").to_string()
}

fn playlist_name(station: &Station, id: PlaylistId) -> &str {
    station
        .playlists
        .iter()
        .find(|p| p.id == id)
        .map_or("?", |p| p.name.as_str())
}

fn warning_text(station: &Station, tz: &TimeZone, w: &Warning) -> String {
    match w {
        Warning::NoEligibleItem { playlist, at } => format!(
            "no eligible item in {} at {}",
            playlist_name(station, *playlist),
            clock(tz, *at)
        ),
        Warning::DeadAir { at, gap } => format!(
            "dead air at {} for {:.1} s",
            clock(tz, *at),
            gap.as_secs_f64()
        ),
        Warning::FixedConflict {
            playlist,
            wanted,
            got,
        } => format!(
            "{} wanted {} but starts at {}",
            playlist_name(station, *playlist),
            clock(tz, *wanted),
            clock(tz, *got)
        ),
        Warning::OutsideTolerance { playlist, error } => format!(
            "{} is outside its tolerance ({:+.1} s)",
            playlist_name(station, *playlist),
            error.as_secs_f64()
        ),
        Warning::LockedMoved { item, by } => format!(
            "locked item {} moved by {:.1} s",
            find_item(station, *item).map_or("?", |i| i.title.as_str()),
            by.as_secs_f64()
        ),
        Warning::EntryDropped {
            at,
            playlist,
            item,
            reason,
        } => {
            let why = match reason {
                DropReason::RotationAfterTrim => "moved by a trim and rejected by rotation rules",
                DropReason::AdjacencyAfterMove => "moved across a fixed event",
                DropReason::AdjacencyAfterReplan => "left next to its kind by a re-plan",
            };
            format!(
                "{}: dropped \"{}\" planned at {} ({why}, it would break an adjacency rule)",
                playlist_name(station, *playlist),
                find_item(station, *item).map_or("?", |i| i.title.as_str()),
                clock(tz, *at)
            )
        }
        Warning::UnknownTimeZone { tz } => format!("unknown time zone '{tz}'"),
    }
}

fn entry_line(station: &Station, tz: &TimeZone, entry: &Entry) -> String {
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
        clock(tz, entry.start),
        secs / 60,
        secs % 60
    )
}
