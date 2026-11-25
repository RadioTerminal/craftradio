use crate::fixtures::*;
use radio_playout::Clock;
use radio_ui_egui::view::{Edit, IcecastView, Status, StreamFormat, ViewSource};
use radio_ui_engine::{MemoryStore, SecretStore, icecast_config};

/// Shares one memory store between the source and the test.
pub(crate) struct Shared(pub(crate) std::sync::Arc<MemoryStore>);

impl SecretStore for Shared {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        self.0.get(key)
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.set(key, secret)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.delete(key)
    }
}

/// A port on this computer where nobody listens, so a source that dials it never connects.
pub(crate) fn refusing_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port()
}

/// The Icecast password goes to the secret store. The settings and the snapshot never carry
/// it; the snapshot only says that it is set.
#[test]
fn the_icecast_password_stays_out_of_the_settings_and_the_snapshot() {
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store.clone())));
    let secret = "hunter-test-only";
    let mount = IcecastView {
        host: "127.0.0.1".into(),
        port: refusing_port(),
        mount: "/live".into(),
        user: "source".into(),
        password: secret.into(),
        password_set: false,
        keychain_error: None,
        tls: false,
        format: StreamFormat::Opus,
        kbps: 96,
        public_url: String::new(),
    };
    assert_eq!(source.edit(Edit::SetIcecast(mount)), Ok(1));
    // The keychain answers on the background lane.
    let view = wait_for(&mut source, &clock, |v| v.icecast.password_set);
    assert!(
        view.icecast.password.is_empty(),
        "no password in the snapshot"
    );
    assert!(view.icecast.password_set);
    assert_eq!(view.icecast.host, "127.0.0.1");
    let settings = serde_json::to_string(&view.settings).unwrap();
    assert!(!settings.contains(secret), "no password in the settings");
    assert_ne!(
        view.output.icecast,
        Status::Connected,
        "a mount where nobody listens is not Connected"
    );

    let state = source.playout().snapshot().state;
    let cfg = icecast_config(&state, &Shared(store), "Test")
        .unwrap()
        .expect("a mount");
    assert_eq!(cfg.password, secret, "the start reads it from the store");
}

/// A password belongs to its mount. After another client changes the host, the new host has
/// no password, and the start sends none to it.
#[test]
fn a_changed_host_gets_no_password_of_the_old_mount() {
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store.clone())));
    let mut mount = IcecastView {
        host: "127.0.0.1".into(),
        port: refusing_port(),
        mount: "/live".into(),
        user: "source".into(),
        password: "only-for-the-old-host".into(),
        password_set: false,
        keychain_error: None,
        tls: false,
        format: StreamFormat::Opus,
        kbps: 96,
        public_url: String::new(),
    };
    assert_eq!(source.edit(Edit::SetIcecast(mount.clone())), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| v.icecast.password_set);
    // Another client (the CLI or MCP) changes only the host setting.
    let _ = &mut mount;
    let _ = source
        .playout()
        .dispatch(radio_engine::Command::SettingsSet {
            part: radio_ui_engine::ICECAST_PART.into(),
            key: "host".into(),
            value: serde_json::json!("localhost"),
        })
        .unwrap();
    let view = wait_for(&mut source, &clock, |v| v.icecast.host == "localhost");
    assert!(!view.icecast.password_set, "the new host has no password");
    let state = source.playout().snapshot().state;
    assert!(
        icecast_config(&state, &Shared(store), "Test").is_err(),
        "the start finds no password for the new host"
    );
}

/// TLS is on for another computer and off for this one, unless a person chose. The start
/// uses the choice.
#[test]
fn tls_defaults_on_for_a_remote_host_and_the_start_honours_the_choice() {
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store.clone())));
    let set = |source: &mut radio_ui_engine::EngineSource, key: &str, value| {
        let _ = source
            .playout()
            .dispatch(radio_engine::Command::SettingsSet {
                part: radio_ui_engine::ICECAST_PART.into(),
                key: key.into(),
                value,
            })
            .unwrap();
    };
    set(&mut source, "host", serde_json::json!("stream.example.org"));
    let view = wait_for(&mut source, &clock, |v| {
        v.icecast.host == "stream.example.org"
    });
    assert!(view.icecast.tls, "a remote host gets TLS");

    set(&mut source, "host", serde_json::json!("127.0.0.1"));
    let view = wait_for(&mut source, &clock, |v| v.icecast.host == "127.0.0.1");
    assert!(!view.icecast.tls, "this computer gets no TLS");

    // A person turns TLS off for a remote host, with a password set. The host is one that
    // never resolves (RFC 6761), so the source that now dials reaches no server.
    let mut mount = view.icecast.clone();
    mount.host = "stream.invalid".into();
    mount.port = refusing_port();
    mount.mount = "/live".into();
    mount.password = "test-only".into();
    mount.tls = false;
    assert_eq!(source.edit(Edit::SetIcecast(mount)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.icecast.password_set && v.icecast.host == "stream.invalid"
    });
    assert!(!view.icecast.tls, "the choice beats the default");
    assert!(view.icecast.password_in_clear(), "the warning shows");
    let state = source.playout().snapshot().state;
    let cfg = icecast_config(&state, &Shared(store), "Test")
        .unwrap()
        .expect("a mount");
    assert!(!cfg.tls, "the start honours the choice");
}

/// The public address listeners open reaches the WebRTC station, so the web player can fall
/// back to it. An address that is not one is refused and changes nothing.
#[test]
fn the_public_icecast_address_reaches_the_webrtc_station_and_a_bad_one_is_refused() {
    let (mut source, clock) = source(Vec::new());
    let mut mount = source.snapshot(jiff::Timestamp::UNIX_EPOCH).icecast;
    mount.host = "stream.example.org".into();
    mount.mount = "/live".into();
    mount.public_url = "  https://listen.example.org/live  ".into();
    assert_eq!(source.edit(Edit::SetIcecast(mount.clone())), Ok(1));
    let view = wait_for(&mut source, &clock, |v| !v.icecast.public_url.is_empty());
    assert_eq!(view.icecast.public_url, "https://listen.example.org/live");
    let state = source.playout().snapshot().state;
    let mut cfg = radio_stream::WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    radio_ui_engine::webrtc_settings(&state, &mut cfg);
    assert_eq!(
        cfg.public_stream.as_deref(),
        Some("https://listen.example.org/live")
    );

    for bad in [
        "listen.example.org/live",
        "https://user:pw@listen.example.org/live",
    ] {
        mount.public_url = bad.into();
        assert!(
            source.edit(Edit::SetIcecast(mount.clone())).is_err(),
            "{bad}"
        );
    }
    let state = source.playout().snapshot().state;
    radio_ui_engine::webrtc_settings(&state, &mut cfg);
    assert_eq!(
        cfg.public_stream.as_deref(),
        Some("https://listen.example.org/live"),
        "a refused address leaves the old one"
    );

    mount.public_url = String::new();
    assert_eq!(source.edit(Edit::SetIcecast(mount)), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| v.icecast.public_url.is_empty());
    let state = source.playout().snapshot().state;
    radio_ui_engine::webrtc_settings(&state, &mut cfg);
    assert_eq!(cfg.public_stream, None, "an empty address shares no stream");
}

/// What the Icecast test server saw of a source.
#[derive(Debug, PartialEq)]
enum Seen {
    /// A source arrived, with the Authorization header it sent.
    Arrived(String),
    Left,
}

/// An Icecast server on this computer that takes each source, says 200 and reads it until it
/// leaves.
fn icecast_server() -> (u16, std::sync::mpsc::Receiver<Seen>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                if conn.read_exact(&mut byte).is_err() {
                    break;
                }
                header.push(byte[0]);
            }
            let auth = String::from_utf8_lossy(&header)
                .lines()
                .find(|l| l.starts_with("Authorization:"))
                .unwrap_or_default()
                .to_owned();
            let _ = conn.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
            if tx.send(Seen::Arrived(auth)).is_err() {
                return;
            }
            let mut bytes = [0; 4096];
            while matches!(conn.read(&mut bytes), Ok(n) if n > 0) {}
            if tx.send(Seen::Left).is_err() {
                return;
            }
        }
    });
    (port, rx)
}

fn local_mount(port: u16, password: &str) -> IcecastView {
    IcecastView {
        host: "127.0.0.1".into(),
        port,
        mount: "/live".into(),
        user: "source".into(),
        password: password.into(),
        password_set: false,
        keychain_error: None,
        tls: false,
        format: StreamFormat::Opus,
        kbps: 96,
        public_url: String::new(),
    }
}

pub(crate) fn icecast_note(view: &radio_ui_egui::view::StationView) -> String {
    view.output_meters
        .iter()
        .find(|m| m.kind == radio_ui_egui::view::OutputKind::Icecast)
        .map(|m| m.note.clone())
        .unwrap_or_default()
}

const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// A station that started without Icecast connects once a person saves a mount and its
/// password, without a restart.
#[test]
fn a_station_with_no_icecast_at_start_connects_once_set_up() {
    let (port, seen) = icecast_server();
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store)));
    let view = source.snapshot(clock.now());
    assert_eq!(icecast_note(&view), "Not set up");
    assert_ne!(view.output.icecast, Status::Connected);

    assert_eq!(
        source.edit(Edit::SetIcecast(local_mount(port, "first-test-only"))),
        Ok(1)
    );
    let view = wait_for(&mut source, &clock, |v| {
        v.output.icecast == Status::Connected
    });
    assert_eq!(view.output.icecast, Status::Connected);
    assert_eq!(icecast_note(&view), "Connected");
    assert!(matches!(
        seen.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));
}

/// A new password alone for the same mount reconnects with that password.
#[test]
fn a_new_password_for_the_mount_reconnects_with_it() {
    let (port, seen) = icecast_server();
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store)));
    assert_eq!(
        source.edit(Edit::SetIcecast(local_mount(port, "first-test-only"))),
        Ok(1)
    );
    let _ = wait_for(&mut source, &clock, |v| {
        v.output.icecast == Status::Connected
    });
    let Seen::Arrived(first) = seen.recv_timeout(PATIENCE).unwrap() else {
        panic!("no source arrived");
    };

    assert_eq!(
        source.edit(Edit::SetIcecast(local_mount(port, "second-test-only"))),
        Ok(1)
    );
    let deadline = std::time::Instant::now() + PATIENCE;
    let second = loop {
        let _ = source.snapshot(clock.now());
        match seen.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(Seen::Arrived(auth)) => break auth,
            Ok(Seen::Left) | Err(_) => {}
        }
        assert!(std::time::Instant::now() < deadline, "never reconnected");
    };
    assert_ne!(first, second, "the new password went to the server");
}

/// Clearing the host closes the connection and the strip says Icecast is not set up.
#[test]
fn clearing_the_mount_closes_icecast_and_the_strip_says_not_set_up() {
    let (port, seen) = icecast_server();
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store)));
    assert_eq!(
        source.edit(Edit::SetIcecast(local_mount(port, "test-only"))),
        Ok(1)
    );
    let _ = wait_for(&mut source, &clock, |v| {
        v.output.icecast == Status::Connected
    });
    assert!(matches!(
        seen.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));

    let mut cleared = local_mount(port, "");
    cleared.host = String::new();
    cleared.mount = String::new();
    assert_eq!(source.edit(Edit::SetIcecast(cleared)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| icecast_note(v) == "Not set up");
    assert_eq!(icecast_note(&view), "Not set up");
    assert_ne!(view.output.icecast, Status::Connected);
    assert_eq!(seen.recv_timeout(PATIENCE).unwrap(), Seen::Left);
}

/// The headless app: no window edits anything; the CLI or MCP sets the Icecast settings while
/// it runs, the keychain already holds the password, and the source connects without a
/// restart.
#[test]
fn a_headless_station_given_icecast_settings_by_the_cli_connects_without_a_restart() {
    let (port, seen) = icecast_server();
    let store = std::sync::Arc::new(MemoryStore::default());
    let key = format!("icecast:source@127.0.0.1:{port}/live");
    store.set(&key, "test-only").unwrap();
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store)));
    let view = source.snapshot(clock.now());
    assert_eq!(icecast_note(&view), "Not set up");

    for (k, v) in [
        ("host", serde_json::json!("127.0.0.1")),
        ("port", serde_json::json!(port)),
        ("mount", serde_json::json!("/live")),
        ("tls", serde_json::json!(false)),
    ] {
        let _ = source
            .playout()
            .dispatch(radio_engine::Command::SettingsSet {
                part: radio_ui_engine::ICECAST_PART.into(),
                key: k.into(),
                value: v,
            })
            .unwrap();
    }
    let view = wait_for(&mut source, &clock, |v| {
        v.output.icecast == Status::Connected
    });
    assert_eq!(view.output.icecast, Status::Connected);
    assert!(matches!(
        seen.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));
}

/// Restore page defaults on the Icecast page: the settings go back to their defaults in one
/// step, switched on again, and the password of the old mount leaves the keychain. A paired
/// station keeps them, because its pairing writes them.
#[test]
fn restoring_icecast_sets_it_back_and_forgets_the_password_unless_paired() {
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store.clone())));
    let port = refusing_port();
    let mount = IcecastView {
        host: "127.0.0.1".into(),
        port,
        mount: "/live".into(),
        user: "source".into(),
        password: "restore-test-only".into(),
        password_set: false,
        keychain_error: None,
        tls: false,
        format: StreamFormat::Opus,
        kbps: 96,
        public_url: "https://stream.example.org/live".into(),
    };
    assert_eq!(source.edit(Edit::SetIcecast(mount)), Ok(1));
    wait_for(&mut source, &clock, |v| v.icecast.password_set);
    source
        .edit(Edit::SetPartSetting {
            part: radio_model::ICECAST_PART.into(),
            key: radio_model::ICECAST_ENABLED_KEY.into(),
            value: false.into(),
        })
        .unwrap();
    let switch = |v: &radio_ui_egui::view::StationView| {
        v.settings
            .get(radio_model::ICECAST_PART)
            .and_then(|s| s.get(radio_model::ICECAST_ENABLED_KEY))
            .and_then(serde_json::Value::as_bool)
    };
    wait_for(&mut source, &clock, |v| switch(v) == Some(false));
    let entry = radio_ui_engine::mount_entry("source", "127.0.0.1", port, "/live");
    assert!(store.get(&entry).unwrap().is_some());

    // Paired: refused, in words, and nothing changes.
    source
        .edit(Edit::SetPartSetting {
            part: radio_model::SERVER_PART.into(),
            key: radio_model::SERVER_ADDRESS_KEY.into(),
            value: "https://server.example.org".into(),
        })
        .unwrap();
    wait_for(&mut source, &clock, |v| v.pairing.paired);
    assert_eq!(
        source.edit(Edit::RestoreIcecast),
        Err(radio_ui_egui::view::ICECAST_PAIRED.to_owned())
    );
    assert!(store.get(&entry).unwrap().is_some(), "the password stays");
    source
        .edit(Edit::SetPartSetting {
            part: radio_model::SERVER_PART.into(),
            key: radio_model::SERVER_ADDRESS_KEY.into(),
            value: "".into(),
        })
        .unwrap();
    wait_for(&mut source, &clock, |v| !v.pairing.paired);

    assert_eq!(source.edit(Edit::RestoreIcecast), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.icecast.host.is_empty()
            && store.get(&entry).is_ok_and(|p| p.is_none())
            && v.output.icecast == Status::NotConnected
    });
    assert!(view.icecast.mount.is_empty());
    assert_eq!(view.icecast.user, "source");
    assert!(view.icecast.public_url.is_empty());
    assert_ne!(view.icecast.kbps, 96, "the bitrate went back");
    assert!(view.icecast.tls, "TLS follows the host again");
    assert!(!view.icecast.password_set);
    assert_eq!(switch(&view), Some(true), "switched on again");
}
