//! Runs the built `craftradio-server` binary the way an operator would.

use radio_server::{hash_key, key_matches};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_craftradio-server");
/// How long a test waits for something that should happen at once.
const PATIENCE: Duration = Duration::from_secs(10);
const MORNING_KEY: &str = "invented-morning-key-4Hq8";

/// A running server process, killed and reaped when the test ends, passed or failed.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn write_config(dir: &Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("server.toml");
    std::fs::write(&path, text).unwrap();
    path
}

fn run_to_exit(config: &Path) -> Output {
    Command::new(BIN)
        .arg("run")
        .arg("--config")
        .arg(config)
        .output()
        .unwrap()
}

#[test]
fn key_prints_a_key_and_the_hash_that_matches_it() {
    let out = Command::new(BIN).arg("key").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "stdout: {stdout:?}");
    let key = lines[0].strip_prefix("key: ").unwrap();
    let hash = lines[1]
        .strip_prefix("key_hash = \"")
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap();
    assert!(!key.is_empty());
    assert!(key_matches(key, hash), "key and hash don't match");
    // The explanation goes to stderr, so stdout stays clean to parse.
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stderr.trim_end(),
        "Put the key_hash line in the station's config. The key is shown once; keep it for the app."
    );

    // Each run makes a new key.
    let again = Command::new(BIN).arg("key").output().unwrap();
    let again = String::from_utf8(again.stdout).unwrap();
    assert_ne!(again.lines().next(), Some(lines[0]));
}

#[test]
fn run_with_a_broken_config_exits_2_and_names_the_problem() {
    let dir = tempfile::tempdir().unwrap();
    let hash = hash_key(MORNING_KEY);
    let config = write_config(
        dir.path(),
        &format!(
            "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
             [[station]]\nslug = \"cloud-signal\"\nname = \"Cloud Signal\"\nkey_hash = \"{hash}\"\n\n\
             [[station]]\nslug = \"cloud-signal\"\nname = \"Cloud Signal Two\"\nkey_hash = \"{hash}\"\n"
        ),
    );
    let out = run_to_exit(&config);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("cloud-signal"), "stderr: {stderr:?}");
    assert!(
        !stderr.contains(&hash),
        "stderr shows a key hash: {stderr:?}"
    );
    assert!(!stderr.contains("listening on"), "stderr: {stderr:?}");
}

#[test]
fn a_key_pasted_into_the_config_is_not_echoed_in_the_error() {
    // The operator pastes the `key:` line instead of the `key_hash` line: a TOML syntax
    // error on the very line that holds the key.
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(
        dir.path(),
        &format!(
            "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
             [[station]]\nslug = \"morning\"\nname = \"Morning Station\"\nkey: {MORNING_KEY}\n"
        ),
    );
    let out = run_to_exit(&config);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stderr.trim().is_empty(), "no error printed");
    assert!(
        !stderr.contains(MORNING_KEY),
        "stderr shows the key: {stderr:?}"
    );
}

#[test]
fn run_serves_the_configured_stations() {
    let dir = tempfile::tempdir().unwrap();
    let hash = hash_key(MORNING_KEY);
    let config = write_config(
        dir.path(),
        &format!(
            "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
             [[station]]\nslug = \"morning\"\nname = \"Morning Station\"\nkey_hash = \"{hash}\"\n"
        ),
    );
    let mut child = Command::new(BIN)
        .arg("run")
        .arg("--config")
        .arg(&config)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let _running = Running(child);

    let (lines_tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + PATIENCE;
    let mut seen = Vec::new();
    let addr: SocketAddr = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = lines
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("no \"listening on\" line; stderr so far: {seen:?}"));
        if let Some(rest) = line.strip_prefix("listening on ") {
            let (addr, stations) = rest.split_once(", ").unwrap();
            assert_eq!(stations, "1 station(s)");
            break addr.parse().unwrap();
        }
        seen.push(line);
    };
    assert_ne!(
        addr.port(),
        0,
        "the line names the bound port, not the configured one"
    );

    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(PATIENCE)).unwrap();
    stream
        .write_all(b"GET /status-json.xsl HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let status = reply.lines().next().and_then(|line| line.split(' ').nth(1));
    assert_eq!(status, Some("200"), "reply: {reply:?}");
    assert!(reply.contains("icestats"), "reply: {reply:?}");
    assert!(!reply.contains(&hash), "reply shows a key hash");
}

/// A pipe whose read end is already gone, so every write to it fails at once.
fn closed_pipe() -> std::io::PipeWriter {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    writer
}

/// Rust exits with 101 when the main thread panics.
const PANICKED: i32 = 101;

#[test]
fn key_with_a_closed_stdout_fails_without_a_panic() {
    let out = Command::new(BIN)
        .arg("key")
        .stdout(closed_pipe())
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(PANICKED), "{out:?}");
    assert!(!out.status.success(), "a key nobody saw is not a success");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stderr.contains("panicked"), "stderr: {stderr:?}");

    // With stderr closed too, the report of the failure can't be written either.
    let out = Command::new(BIN)
        .arg("key")
        .stdout(closed_pipe())
        .stderr(closed_pipe())
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(PANICKED), "{out:?}");
    assert!(!out.status.success(), "a key nobody saw is not a success");
}

#[test]
fn key_with_a_closed_stderr_still_prints_the_key() {
    let out = Command::new(BIN)
        .arg("key")
        .stderr(closed_pipe())
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(PANICKED), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.contains("panicked"), "stdout: {stdout:?}");
    assert!(stdout.starts_with("key: "), "stdout: {stdout:?}");
}

#[test]
fn run_with_a_closed_stderr_still_exits_2_on_a_broken_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), "listen = \"not an address\"\n");
    let out = Command::new(BIN)
        .arg("run")
        .arg("--config")
        .arg(&config)
        .stderr(closed_pipe())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}
