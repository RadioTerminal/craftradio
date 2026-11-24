//! Pairing with a real station server on this computer: the address and the key give a
//! working Icecast push, and every way it can go wrong says so in words.

use crate::fixtures::*;
use crate::icecast::{Shared, icecast_note, refusing_port};
use base64::Engine as _;
use radio_engine::Command;
use radio_model::PairingDocument;
use radio_playout::Clock;
use radio_server::{Config, Server, ServerHandle, hash_key};
use radio_ui_egui::view::{Edit, PairState, Secret, StationView, Status, ViewSource};
use radio_ui_engine::{EngineSource, MemoryStore, SecretStore, check_address};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DAWN_KEY: &str = "invented-dawn-key-9Qx4";
const WRONG_KEY: &str = "invented-wrong-key-2Lr8";

/// A station server on a free port of this computer, with the station "dawn". Its public
/// URL is its own address, so the push the document names reaches it. The address it gives
/// is the one a person types.
fn server() -> (ServerHandle, String) {
    let port = refusing_port();
    let config = format!(
        "listen = \"127.0.0.1:{port}\"\npublic_url = \"http://127.0.0.1:{port}\"\n\n\
         [[station]]\nslug = \"dawn\"\nname = \"Dawn Chorus\"\nkey_hash = \"{}\"\n",
        hash_key(DAWN_KEY),
    );
    let server = Server::start(Config::parse(&config).unwrap()).unwrap();
    (server, format!("http://127.0.0.1:{port}"))
}

/// A source over a fresh station with a memory keychain the test can read.
fn station() -> (EngineSource, radio_playout::ManualClock, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let source = source.with_secrets(Arc::new(Shared(store.clone())));
    (source, clock, store)
}

/// The keychain entry of dawn's push on the server at `address`.
fn dawn_entry(address: &str) -> String {
    let host_port = address.trim_start_matches("http://");
    format!("icecast:source@{host_port}/dawn/stream")
}

/// `GET <address><path>` with `key` as the Basic password, if any: the status and the body.
fn get(address: &str, path: &str, key: Option<&str>) -> (u16, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(5)))
            .build(),
    );
    let mut request = agent.get(format!("{address}{path}"));
    if let Some(key) = key {
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("source:{key}"));
        request = request.header("Authorization", format!("Basic {basic}"));
    }
    let mut response = request.call().unwrap();
    let status = response.status().as_u16();
    (status, response.body_mut().read_to_string().unwrap())
}

/// True when the server lists dawn's pushed stream as live.
fn dawn_is_live(address: &str) -> bool {
    let (_, body) = get(address, "/status-json.xsl", None);
    body.contains("/dawn/stream")
}

/// Takes snapshots until the server says dawn is live (or not), for at most twenty seconds.
fn wait_live(
    source: &mut EngineSource,
    clock: &radio_playout::ManualClock,
    address: &str,
    live: bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let _ = source.snapshot(clock.now());
        if dawn_is_live(address) == live || Instant::now() > deadline {
            return dawn_is_live(address);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn settings_text(view: &StationView) -> String {
    serde_json::to_string(&view.settings).unwrap()
}

fn setting(view: &StationView, part: &str, key: &str) -> Option<serde_json::Value> {
    view.settings.get(part).and_then(|s| s.get(key)).cloned()
}

fn pair(source: &mut EngineSource, address: &str, key: &str) -> Result<usize, String> {
    source.edit(Edit::Pair {
        address: address.into(),
        key: Secret::new(key),
    })
}

/// The phase 2 done check: a fresh station, the server's address and the key, and nothing
/// else. The server sees the push, and the state says Paired.
#[test]
fn address_and_key_pair_and_icecast_connects() {
    let (_server, address) = server();
    let (mut source, clock, _store) = station();
    let view = source.snapshot(clock.now());
    assert_eq!(view.pairing.state, PairState::NotPaired);
    assert_eq!(icecast_note(&view), "Not set up");

    // A trailing slash, as people type it.
    assert_eq!(pair(&mut source, &format!("{address}/"), DAWN_KEY), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired && v.output.icecast == Status::Connected
    });
    assert_eq!(view.pairing.state, PairState::Paired);
    assert!(view.pairing.paired);
    assert_eq!(
        view.pairing.address, address,
        "the address without the slash"
    );
    assert_eq!(view.output.icecast, Status::Connected);
    assert!(
        wait_live(&mut source, &clock, &address, true),
        "the server sees the station's push"
    );
    assert_eq!(
        setting(&view, "server", "address"),
        Some(address.clone().into())
    );
}

/// A wrong key: the state says so, nothing is written, and the app does not try again by
/// itself, so the server's lockout (five wrong keys) never closes on the right one.
#[test]
fn a_wrong_key_says_key_refused_and_does_not_retry() {
    let (_server, address) = server();
    let (mut source, clock, store) = station();
    assert_eq!(pair(&mut source, &address, WRONG_KEY), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::KeyRefused
    });
    assert_eq!(view.pairing.state, PairState::KeyRefused);
    assert!(!view.pairing.paired, "a refused first try pairs nothing");
    assert!(view.pairing.label().contains("refused the key"));
    assert_eq!(setting(&view, "server", "address"), None);
    assert_eq!(setting(&view, "icecast", "host"), None);
    assert_eq!(store.get(&dawn_entry(&address)), Ok(None), "no key stored");

    // Many snapshots later, the server still takes the right key: no retry walked its lockout.
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        let _ = source.snapshot(clock.now());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(get(&address, "/api/station", Some(DAWN_KEY)).0, 200);
}

/// The settings of a pairing with `address`, written as `server.pair` writes them.
fn paired_with(source: &mut EngineSource, address: &str, document: PairingDocument) {
    source
        .playout()
        .dispatch(Command::ServerPair {
            address: address.into(),
            document,
        })
        .unwrap();
}

/// A pairing document, field for field as a server at `address` could write it.
fn document_for(address: &str) -> PairingDocument {
    let host_port = address.trim_start_matches("http://");
    let (host, port) = host_port.split_once(':').unwrap();
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "slug": "dawn",
        "name": "Dawn Chorus",
        "public_url": address,
        "icecast": {"host": host, "port": port.parse::<u16>().unwrap(), "tls": false,
                    "mount": "/dawn/stream", "user": "source"},
        "stream_url": format!("{address}/dawn/stream"),
        "outputs": []
    }))
    .unwrap()
}

/// A server that takes connections and never answers. At start the app keeps the last paired
/// settings, the snapshots stay quick while it waits, and the state says unreachable.
#[test]
fn a_server_that_is_down_says_unreachable_and_keeps_settings() {
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", silent.local_addr().unwrap());
    let (mut source, clock, store) = station();
    paired_with(&mut source, &address, document_for(&address));
    store.set(&dawn_entry(&address), DAWN_KEY).unwrap();

    let deadline = Instant::now() + Duration::from_secs(25);
    let view = loop {
        let asked = Instant::now();
        let view = source.snapshot(clock.now());
        assert!(
            asked.elapsed() < Duration::from_secs(1),
            "a snapshot waited on the server"
        );
        if matches!(view.pairing.state, PairState::Unreachable(_)) || Instant::now() > deadline {
            break view;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let PairState::Unreachable(reason) = &view.pairing.state else {
        panic!("not unreachable: {:?}", view.pairing);
    };
    assert!(!reason.is_empty());
    assert!(view.pairing.paired);
    assert!(
        view.pairing
            .label()
            .starts_with("Paired — server unreachable")
    );
    assert_eq!(
        setting(&view, "server", "address"),
        Some(address.clone().into())
    );
    assert_eq!(
        setting(&view, "icecast", "mount"),
        Some("/dawn/stream".into()),
        "the last paired settings stay"
    );
    assert_eq!(store.get(&dawn_entry(&address)), Ok(Some(DAWN_KEY.into())));
    drop(silent);
}

/// At start the app asks the paired server again, and what it hands out overwrites a hand
/// edit of a paired setting.
#[test]
fn the_app_refetches_at_start_and_overwrites_hand_edits() {
    let (_server, address) = server();
    let (status, body) = get(&address, "/api/station", Some(DAWN_KEY));
    assert_eq!(status, 200);
    let document: PairingDocument = serde_json::from_str(&body).unwrap();
    let stream_url = document.stream_url.clone();
    let (mut source, clock, store) = station();
    // The station as a last run left it: paired, the key in the keychain, and a hand edit.
    paired_with(&mut source, &address, document);
    store.set(&dawn_entry(&address), DAWN_KEY).unwrap();
    for (part, key) in [("icecast", "public_url"), ("webrtc", "public_stream")] {
        source
            .playout()
            .dispatch(Command::SettingsSet {
                part: part.into(),
                key: key.into(),
                value: "https://hand.example.org/edited".into(),
            })
            .unwrap();
    }

    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired
            && setting(v, "webrtc", "public_stream") == Some(stream_url.clone().into())
    });
    assert_eq!(view.pairing.state, PairState::Paired);
    assert_eq!(
        setting(&view, "webrtc", "public_stream"),
        Some(stream_url.clone().into())
    );
    assert_eq!(
        setting(&view, "icecast", "public_url"),
        Some(stream_url.into())
    );
    assert!(!settings_text(&view).contains("hand.example.org"));
}

/// The key was rotated on the server: at start the stored key is refused, the push waits
/// instead of walking the lockout, and the new key, for the same mount, applies at once.
#[test]
fn a_refused_key_at_start_holds_the_push_until_a_new_key_pairs() {
    let (_server, address) = server();
    let (mut source, clock, store) = station();
    paired_with(&mut source, &address, document_for(&address));
    store.set(&dawn_entry(&address), WRONG_KEY).unwrap();

    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::KeyRefused && icecast_note(v) != "Connected"
    });
    assert_eq!(view.pairing.state, PairState::KeyRefused);
    assert!(view.pairing.paired);
    assert_eq!(view.pairing.label(), "Paired — key refused");
    assert_ne!(view.output.icecast, Status::Connected);

    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired && v.output.icecast == Status::Connected
    });
    assert_eq!(view.output.icecast, Status::Connected);
    assert!(wait_live(&mut source, &clock, &address, true));
    assert_eq!(store.get(&dawn_entry(&address)), Ok(Some(DAWN_KEY.into())));
}

/// Unpairing while the push is connected: the push closes, the strip says Icecast is not set
/// up, the settings pairing wrote go, and so does the key.
#[test]
fn unpair_removes_the_key_and_closes_icecast() {
    let (_server, address) = server();
    let (mut source, clock, store) = station();
    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| {
        v.output.icecast == Status::Connected
    });
    assert!(wait_live(&mut source, &clock, &address, true));
    assert_eq!(store.get(&dawn_entry(&address)), Ok(Some(DAWN_KEY.into())));

    assert_eq!(source.edit(Edit::Unpair), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        icecast_note(v) == "Not set up" && !v.pairing.paired
    });
    assert_eq!(icecast_note(&view), "Not set up");
    assert_eq!(view.pairing.state, PairState::NotPaired);
    assert_eq!(setting(&view, "server", "address"), None);
    assert!(
        !wait_live(&mut source, &clock, &address, false),
        "the push closed"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while store.get(&dawn_entry(&address)) != Ok(None) && Instant::now() < deadline {
        let _ = source.snapshot(clock.now());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        store.get(&dawn_entry(&address)),
        Ok(None),
        "the key is gone"
    );
    // Unpairing again changes nothing and says why.
    assert!(source.edit(Edit::Unpair).is_err());
}

/// One rule for the address: a scheme, `http://` only to this computer, the server only.
/// Each refusal says what to type instead, and nothing is asked of any server.
#[test]
fn an_address_without_a_scheme_or_http_to_a_remote_host_is_refused_in_words() {
    let (mut source, clock, _store) = station();
    for (typed, words) in [
        ("radio.example.org", "https://"),
        ("http://radio.example.org", "this computer"),
        ("https://radio.example.org/station", "no path"),
        ("https://radio.example.org?x=1", "no path"),
        ("https://someone@radio.example.org", "user name"),
        ("https://radio.example.org:99999", "port"),
        ("", "address"),
    ] {
        let refused = pair(&mut source, typed, DAWN_KEY).expect_err(typed);
        assert!(refused.contains(words), "{typed}: {refused}");
    }
    let view = source.snapshot(clock.now());
    assert_eq!(view.pairing.state, PairState::NotPaired);

    assert_eq!(
        check_address("https://Radio.Example.org/").as_deref(),
        Ok("https://radio.example.org")
    );
    for local in [
        "http://localhost:8000",
        "http://127.0.0.1:8000",
        "http://[::1]",
    ] {
        assert_eq!(check_address(local).as_deref(), Ok(local));
    }
}

/// The key reaches the keychain and the server only: not the settings, the snapshot, a notice
/// or a debug print of the edit.
#[test]
fn the_key_never_reaches_settings_or_the_snapshot() {
    let (_server, address) = server();
    let (mut source, clock, _store) = station();
    let edit = Edit::Pair {
        address: address.clone(),
        key: Secret::new(DAWN_KEY),
    };
    assert!(!format!("{edit:?}").contains(DAWN_KEY));
    assert_eq!(source.edit(edit), Ok(1));
    let mut notices = Vec::new();
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired && v.icecast.password_set
    });
    notices.extend(source.take_notices());
    assert_eq!(view.pairing.state, PairState::Paired);
    assert!(view.icecast.password_set, "the push has its password");
    assert!(view.icecast.password.is_empty());
    assert!(!settings_text(&view).contains(DAWN_KEY));
    assert!(!format!("{view:?}").contains(DAWN_KEY));
    assert!(!notices.iter().any(|n| n.contains(DAWN_KEY)));
    let state = source.playout().snapshot().state;
    for part in ["icecast", "webrtc", "server"] {
        let stored = serde_json::to_string(&state.settings(part)).unwrap();
        assert!(!stored.contains(DAWN_KEY), "{part}");
    }
}

/// A server on this computer that answers every request with `status` and `body`.
fn answering(status: &'static str, body: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") && conn.read_exact(&mut byte).is_ok() {
                head.push(byte[0]);
            }
            let _ = write!(
                conn,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    address
}

/// A server of a newer version is refused in words and changes nothing; a server that locks
/// this computer out says it refuses for a while.
#[test]
fn a_newer_document_is_refused_in_words_and_changes_nothing() {
    let newer = answering("200 OK", r#"{"version": 2, "station": {"slug": "dawn"}}"#);
    let (mut source, clock, store) = station();
    assert_eq!(pair(&mut source, &newer, DAWN_KEY), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        matches!(v.pairing.state, PairState::Refused(_))
    });
    let PairState::Refused(reason) = &view.pairing.state else {
        panic!("not refused: {:?}", view.pairing);
    };
    assert!(reason.contains("newer CraftRadio"), "{reason}");
    assert!(!view.pairing.paired);
    assert_eq!(setting(&view, "server", "address"), None);
    assert_eq!(setting(&view, "icecast", "host"), None);
    assert_eq!(store.get(&dawn_entry(&newer)), Ok(None), "no key stored");

    let locked = answering("429 Too Many Requests", "too many wrong keys");
    assert_eq!(pair(&mut source, &locked, DAWN_KEY), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        matches!(v.pairing.state, PairState::Unreachable(_))
    });
    let PairState::Unreachable(reason) = &view.pairing.state else {
        panic!("not unreachable: {:?}", view.pairing);
    };
    assert!(reason.contains("for a while"), "{reason}");
}

/// The CLI or MCP unpairs while the window shows Paired: the window says not paired.
#[test]
fn an_unpair_by_another_client_says_not_paired() {
    let (_server, address) = server();
    let (mut source, clock, _store) = station();
    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired
    });
    source.playout().dispatch(Command::ServerUnpair).unwrap();
    let view = wait_for(&mut source, &clock, |v| !v.pairing.paired);
    assert_eq!(view.pairing.state, PairState::NotPaired);
    assert_eq!(view.pairing.label(), "Not paired");
}

/// After a refused first try here, another client pairs the station (the key already in the
/// keychain): the source asks that server once, says Paired, and the push connects.
#[test]
fn a_pairing_by_another_client_is_asked_once_and_connects() {
    let (_server, address) = server();
    let (mut source, clock, store) = station();
    assert_eq!(pair(&mut source, &address, WRONG_KEY), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::KeyRefused
    });
    store.set(&dawn_entry(&address), DAWN_KEY).unwrap();
    paired_with(&mut source, &address, document_for(&address));
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired && v.output.icecast == Status::Connected
    });
    assert_eq!(view.pairing.state, PairState::Paired);
    assert_eq!(view.output.icecast, Status::Connected);
    assert!(wait_live(&mut source, &clock, &address, true));
}
