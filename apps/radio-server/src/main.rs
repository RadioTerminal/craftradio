//! `craftradio-server`: the public station server. Takes Icecast pushes from the radio app
//! and serves them to listeners.

use clap::{Parser, Subcommand};
use radio_server::{Config, Server, hash_key, new_key};
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
            eprintln!(
                "{}: {}",
                path.display(),
                without_source_lines(&e.to_string())
            );
            return ExitCode::from(FAILED);
        }
    };
    let stations = config.stations.len();
    let server = match Server::start(config) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("could not start: {e}");
            return ExitCode::from(FAILED);
        }
    };
    eprintln!(
        "listening on {}, {stations} station(s)",
        server.local_addr()
    );
    // The server runs on its own threads; this one only keeps it alive.
    loop {
        std::thread::park();
    }
}

fn key() -> ExitCode {
    let key = match new_key() {
        Ok(key) => key,
        Err(e) => {
            eprintln!("could not make a key: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("key: {key}");
    println!("key_hash = \"{}\"", hash_key(&key));
    eprintln!(
        "Put the key_hash line in the station's config. The key is shown once; keep it for the app."
    );
    ExitCode::SUCCESS
}

/// A TOML parse error quotes the line it failed on, and that line may hold a key pasted in
/// by mistake. Keeps the error's own lines and drops the quoted ones (`5 | …`, `  | ^`).
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
