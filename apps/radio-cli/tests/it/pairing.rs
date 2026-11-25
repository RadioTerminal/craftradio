//! `craftradio-cli pair` and `unpair` against a real station server on this computer. The
//! keychain is one in memory (`CRAFTRADIO_TEST_MEMORY_KEYCHAIN`), never the OS keychain.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use radio_model::{SERVER_ADDRESS_KEY, SERVER_PART};
use radio_server::{Config, Server, ServerHandle, hash_key};

pub(crate) const DAWN_KEY: &str = "invented-dawn-key-7Hc2";
pub(crate) const WRONG_KEY: &str = "invented-wrong-key-4Tm9";

/// A station server on a free port of this computer, with the station "dawn", and the
/// address a person types for it.
fn server() -> (ServerHandle, String) {
    server_with(None)
}

/// [`server`] with a `state_dir`, where the server keeps outputs the app sets, when given.
pub(crate) fn server_with(state_dir: Option<&Path>) -> (ServerHandle, String) {
    let state_dir = state_dir
        .map(|dir| format!("state_dir = {:?}\n", dir.display().to_string()))
        .unwrap_or_default();
    // The port is free when picked; another test may take it before the server binds it,
    // so a taken port is picked again.
    for _ in 0..20 {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("a free port")
            .port();
        let config = format!(
            "listen = \"127.0.0.1:{port}\"\npublic_url = \"http://127.0.0.1:{port}\"\n\
             {state_dir}\n[[station]]\nslug = \"dawn\"\nname = \"Dawn Chorus\"\nkey_hash = \"{}\"\n",
            hash_key(DAWN_KEY),
        );
        if let Ok(server) = Server::start(Config::parse(&config).unwrap()) {
            return (server, format!("http://127.0.0.1:{port}"));
        }
    }
    panic!("no free port for the test server");
}

/// A folder with a fresh station.db.
pub(crate) fn station() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    radio_engine::Engine::open(dir.path().join("station.db"))
        .unwrap()
        .flush()
        .unwrap();
    dir
}

/// Runs the CLI with the memory keychain and `stdin` on standard input. The key never comes
/// from the test's own environment.
fn cli(args: &[&str], data_dir: &Path, stdin: &str) -> Output {
    cli_with(args, data_dir, stdin, &[])
}

/// [`cli`] with more environment variables.
pub(crate) fn cli_with(
    args: &[&str],
    data_dir: &Path,
    stdin: &str,
    env: &[(&str, &str)],
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_craftradio-cli"))
        .args(args)
        .envs(env.iter().copied())
        .arg("--data-dir")
        .arg(data_dir)
        .env("CRAFTRADIO_TEST_MEMORY_KEYCHAIN", "1")
        .env_remove("CRAFTRADIO_SERVER_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("craftradio-cli runs");
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.as_bytes()).unwrap();
    drop(input);
    child.wait_with_output().unwrap()
}

/// The paired address in the station's settings, if any.
pub(crate) fn paired_address(data_dir: &Path) -> Option<String> {
    let engine = radio_engine::Engine::open(data_dir.join("station.db")).unwrap();
    engine
        .state()
        .settings(SERVER_PART)
        .and_then(|s| s.get(SERVER_ADDRESS_KEY))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Every byte the station folder holds, as text, to look for the key in.
fn folder_text(data_dir: &Path) -> String {
    std::fs::read_dir(data_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).into_owned())
        .collect()
}

#[test]
fn cli_pair_reads_the_key_from_stdin_and_never_prints_it() {
    let (_server, address) = server();
    let dir = station();
    let out = cli(&["pair", &address], dir.path(), &format!("{DAWN_KEY}\n"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("Paired with"), "{stdout}");
    assert!(!stdout.contains(DAWN_KEY) && !stderr.contains(DAWN_KEY));
    assert_eq!(
        paired_address(dir.path()).as_deref(),
        Some(address.as_str())
    );
    // The push the server named, under the entry the app reads.
    let host_port = address.trim_start_matches("http://");
    assert!(
        stdout.contains(&format!("icecast:source@{host_port}/dawn/stream")),
        "{stdout}"
    );
    assert!(
        !folder_text(dir.path()).contains(DAWN_KEY),
        "the key is in the station files"
    );

    let out = cli(&["unpair"], dir.path(), "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(paired_address(dir.path()), None);
    let again = cli(&["unpair"], dir.path(), "");
    assert!(
        !again.status.success(),
        "a station that is not paired says so"
    );
    assert!(String::from_utf8_lossy(&again.stderr).contains("not paired"));
}

#[test]
fn cli_pair_with_a_wrong_key_exits_nonzero_with_key_refused() {
    let (_server, address) = server();
    let dir = station();
    let out = cli(&["pair", &address], dir.path(), &format!("{WRONG_KEY}\n"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("key refused"), "{stderr}");
    assert!(!stderr.contains(WRONG_KEY));
    assert_eq!(
        paired_address(dir.path()),
        None,
        "a refused key pairs nothing"
    );
}

#[test]
fn cli_pair_with_no_server_says_unreachable_and_with_no_key_pairs_nothing() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap()
        .port();
    let dir = station();
    let address = format!("http://127.0.0.1:{port}");
    let out = cli(&["pair", &address], dir.path(), &format!("{DAWN_KEY}\n"));
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unreachable: "), "{stderr}");

    let out = cli(&["pair", &address], dir.path(), "");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no station key"));

    // http:// to another computer would send the key in clear: refused in words.
    let out = cli(&["pair", "http://radio.example.org"], dir.path(), DAWN_KEY);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("https://"));
    assert_eq!(paired_address(dir.path()), None);
}

/// A proxy named in the environment never sees the request that carries the key: the
/// pairing goes straight to the server, even with every proxy variable pointing at a port
/// where nothing listens.
#[test]
fn cli_pair_ignores_a_proxy_in_the_environment() {
    let (_server, address) = server();
    let dir = station();
    let nowhere = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap();
    let proxy = format!("http://{nowhere}");
    let env: Vec<(&str, &str)> = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .into_iter()
    .map(|k| (k, proxy.as_str()))
    .chain([("NO_PROXY", ""), ("no_proxy", "")])
    .collect();
    let out = cli_with(
        &["pair", &address],
        dir.path(),
        &format!("{DAWN_KEY}\n"),
        &env,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        paired_address(dir.path()).as_deref(),
        Some(address.as_str())
    );
}
