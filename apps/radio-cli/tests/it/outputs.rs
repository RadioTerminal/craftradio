//! `craftradio-cli outputs` and `outputs set` against a real station server on this
//! computer. The keychain is a test file outside the station folder
//! (`CRAFTRADIO_TEST_KEYCHAIN_FILE`), so `pair` and a later `outputs set` share it; never
//! the OS keychain.

use std::path::Path;
use std::process::Output;

use crate::pairing::{DAWN_KEY, WRONG_KEY, cli_with, paired_address, server_with, station};

/// Runs the CLI with the keychain in the file `keychain`.
fn cli(args: &[&str], data_dir: &Path, keychain: &Path, stdin: &str) -> Output {
    let keychain = keychain.to_str().unwrap();
    cli_with(
        args,
        data_dir,
        stdin,
        &[("CRAFTRADIO_TEST_KEYCHAIN_FILE", keychain)],
    )
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn outputs_set_then_list() {
    let state = tempfile::tempdir().unwrap();
    let (_server, address) = server_with(Some(state.path()));
    let dir = station();
    let keys = tempfile::tempdir().unwrap();
    let keychain = keys.path().join("keychain.json");
    let out = cli(
        &["pair", &address],
        dir.path(),
        &keychain,
        &format!("{DAWN_KEY}\n"),
    );
    assert!(out.status.success(), "{:?}", text(&out));
    let out = cli(&["outputs"], dir.path(), &keychain, "");
    assert!(out.status.success(), "{:?}", text(&out));
    assert_eq!(text(&out).0, "", "a new station has no outputs");

    let out = cli(
        &["outputs", "set", "low.opus=opus:32", "high.mp3=mp3:128"],
        dir.path(),
        &keychain,
        "",
    );
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(!stdout.contains(DAWN_KEY) && !stderr.contains(DAWN_KEY));

    let out = cli(&["outputs"], dir.path(), &keychain, "");
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            format!("low.opus opus 32 {address}/dawn/low.opus"),
            format!("high.mp3 mp3 128 {address}/dawn/high.mp3"),
        ],
        "{stdout}"
    );
    assert!(state.path().join("dawn.outputs.json").is_file());

    // A bitrate the server's encoder can't make: its reason, and nothing changes.
    let out = cli(
        &["outputs", "set", "low.opus=opus:9999"],
        dir.path(),
        &keychain,
        "",
    );
    let (_, stderr) = text(&out);
    assert!(!out.status.success());
    assert!(stderr.contains("9999"), "{stderr}");
    let out = cli(&["outputs"], dir.path(), &keychain, "");
    assert_eq!(text(&out).0.lines().count(), 2, "the list is as it was");

    // A shape the CLI can't read is refused before anything is sent.
    let out = cli(
        &["outputs", "set", "low.flac=flac:32"],
        dir.path(),
        &keychain,
        "",
    );
    assert!(!out.status.success());
    assert!(text(&out).1.contains("opus or mp3"), "{:?}", text(&out));

    // None clears them.
    let out = cli(&["outputs", "set"], dir.path(), &keychain, "");
    assert!(out.status.success(), "{:?}", text(&out));
    let out = cli(&["outputs"], dir.path(), &keychain, "");
    assert_eq!(text(&out).0, "");
}

#[test]
fn outputs_set_unpaired_fails() {
    let dir = station();
    let keys = tempfile::tempdir().unwrap();
    let keychain = keys.path().join("keychain.json");
    for args in [
        &["outputs", "set", "low.opus=opus:32"][..],
        &["outputs"][..],
    ] {
        let out = cli(args, dir.path(), &keychain, "");
        assert!(!out.status.success(), "{args:?}");
        assert!(text(&out).1.contains("not paired"), "{:?}", text(&out));
    }
    assert_eq!(paired_address(dir.path()), None);
}

/// The key in the keychain was rotated away on the server: `outputs set` says key refused
/// and exits non-zero, as `pair` does.
#[test]
fn outputs_set_with_a_refused_key_says_key_refused() {
    let state = tempfile::tempdir().unwrap();
    let (_server, address) = server_with(Some(state.path()));
    let dir = station();
    let keys = tempfile::tempdir().unwrap();
    let keychain = keys.path().join("keychain.json");
    let out = cli(
        &["pair", &address],
        dir.path(),
        &keychain,
        &format!("{DAWN_KEY}\n"),
    );
    assert!(out.status.success(), "{:?}", text(&out));
    let stored = std::fs::read_to_string(&keychain).unwrap();
    std::fs::write(&keychain, stored.replace(DAWN_KEY, WRONG_KEY)).unwrap();

    let out = cli(
        &["outputs", "set", "low.opus=opus:32"],
        dir.path(),
        &keychain,
        "",
    );
    let (_, stderr) = text(&out);
    assert!(!out.status.success());
    assert!(stderr.contains("key refused"), "{stderr}");
    assert!(!stderr.contains(WRONG_KEY));
}

/// A server without a `state_dir` keeps its outputs in its config: `outputs set` says so.
#[test]
fn outputs_set_on_a_server_without_a_state_dir_says_why() {
    let (_server, address) = server_with(None);
    let dir = station();
    let keys = tempfile::tempdir().unwrap();
    let keychain = keys.path().join("keychain.json");
    let out = cli(
        &["pair", &address],
        dir.path(),
        &keychain,
        &format!("{DAWN_KEY}\n"),
    );
    assert!(out.status.success(), "{:?}", text(&out));
    let out = cli(
        &["outputs", "set", "low.opus=opus:32"],
        dir.path(),
        &keychain,
        "",
    );
    assert!(!out.status.success());
    assert!(text(&out).1.contains("config"), "{:?}", text(&out));
}
