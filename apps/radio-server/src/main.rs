//! `craftradio-server`: the public station server. Takes Icecast pushes from the radio app
//! and serves them to listeners.

use clap::{Parser, Subcommand};
use radio_server::{Config, Server, hash_key, new_key};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The exit code for a config the server can't use or a server that can't start.
const FAILED: u8 = 2;

#[derive(Parser)]
#[command(version, name = "craftradio-server", about = "Radio station server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server with a config file until the process is stopped.
    Run {
        /// The server's config (TOML).
        #[arg(long)]
        config: PathBuf,
    },
    /// Make a new station key and the key_hash line for the config.
    Key,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Run { config } => run(&config),
        Command::Key => key(),
    }
}

fn run(path: &Path) -> ExitCode {
    let config = match Config::load(path) {
        Ok(config) => config,
        Err(e) => {
            say(format_args!(
                "{}: {}",
                path.display(),
                without_source_lines(&e.to_string())
            ));
            return ExitCode::from(FAILED);
        }
    };
    let stations = config.stations.len();
    let server = match Server::start(config) {
        Ok(server) => server,
        Err(e) => {
            say(format_args!("could not start: {e}"));
            return ExitCode::from(FAILED);
        }
    };
    say(format_args!(
        "listening on {}, {stations} station(s)",
        server.local_addr()
    ));
    // The server runs on its own threads; this one only keeps it alive.
    loop {
        std::thread::park();
    }
}

fn key() -> ExitCode {
    let key = match new_key() {
        Ok(key) => key,
        Err(e) => {
            say(format_args!("could not make a key: {e}"));
            return ExitCode::FAILURE;
        }
    };
    // A closed stdout is an error to report, not a panic as with `println!`.
    let mut out = io::stdout().lock();
    if let Err(e) = writeln!(out, "key: {key}\nkey_hash = \"{}\"", hash_key(&key)) {
        say(format_args!("could not print the key: {e}"));
        return ExitCode::FAILURE;
    }
    say(format_args!(
        "Put the key_hash line in the station's config. The key is shown once; keep it for the app."
    ));
    ExitCode::SUCCESS
}

/// Writes one line to stderr. Unlike `eprintln!`, a closed stderr is not a panic: the line
/// is dropped, as there is nowhere left to report it.
fn say(line: std::fmt::Arguments<'_>) {
    let _ = writeln!(io::stderr(), "{line}");
}

/// Removes only the source lines a TOML parse error quotes (`5 | …`, `  | ^`), which may hold
/// a key pasted in by mistake, and keeps the error's own lines. It does not redact anything
/// else: a serde or library message can still echo a value from the config, such as a slug.
fn without_source_lines(message: &str) -> String {
    message
        .lines()
        .filter(|line| !is_source_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_source_line(line: &str) -> bool {
    let gutter = line
        .trim_start()
        .trim_start_matches(|c: char| c.is_ascii_digit());
    gutter.trim_start().starts_with('|')
}
