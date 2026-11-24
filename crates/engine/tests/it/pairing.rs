//! Pairing with a station server: `server.pair` writes the settings its document offers and
//! `server.unpair` takes them away again.

use radio_engine::{Command, Engine, Tier, registry};
use radio_model::{PairingDocument, SERVER_PART};
use radio_server::pairing::{Document, IcecastTarget, OutputEntry};
use serde_json::{Value, json};

use super::temp_db;

const ADDRESS: &str = "https://radio.example.invalid";

/// The document a server writes for an invented station.
fn server_document() -> Document {
    Document {
        version: 1,
        slug: "night-owls".into(),
        name: "Night Owls FM".into(),
        public_url: ADDRESS.into(),
        icecast: IcecastTarget {
            host: "radio.example.invalid".into(),
            port: 8443,
            tls: true,
            mount: "/night-owls/stream".into(),
            user: "source".into(),
        },
        stream_url: format!("{ADDRESS}/night-owls/stream"),
        outputs: vec![
            OutputEntry {
                mount: "low".into(),
                format: "mp3".into(),
                kbps: 96,
                url: format!("{ADDRESS}/night-owls/low"),
            },
            OutputEntry {
                mount: "high".into(),
                format: "opus".into(),
                kbps: 192,
                url: format!("{ADDRESS}/night-owls/high"),
            },
        ],
    }
}

/// The server's document as the app reads it, through JSON as it travels.
fn document() -> PairingDocument {
    serde_json::from_value(serde_json::to_value(server_document()).unwrap()).unwrap()
}

fn pair(e: &mut Engine, document: PairingDocument) -> Result<(), radio_engine::EngineError> {
    e.dispatch(Command::ServerPair {
        address: ADDRESS.into(),
        document,
    })
    .map(|_| ())
}

fn setting(e: &Engine, part: &str, key: &str) -> Option<Value> {
    e.state().settings(part)?.get(key).cloned()
}

fn set(e: &mut Engine, part: &str, key: &str, value: Value) {
    e.dispatch(Command::SettingsSet {
        part: part.into(),
        key: key.into(),
        value,
    })
    .unwrap();
}

/// The settings pairing writes, by part and key.
const PAIRED: [(&str, &str); 13] = [
    ("icecast", "host"),
    ("icecast", "port"),
    ("icecast", "mount"),
    ("icecast", "user"),
    ("icecast", "tls"),
    ("icecast", "public_url"),
    ("webrtc", "public_stream"),
    (SERVER_PART, "address"),
    (SERVER_PART, "slug"),
    (SERVER_PART, "name"),
    (SERVER_PART, "stream_url"),
    (SERVER_PART, "outputs"),
    (SERVER_PART, "push"),
];

#[test]
fn the_app_reads_the_document_the_server_writes() {
    let written = serde_json::to_value(server_document()).unwrap();
    let read: PairingDocument = serde_json::from_value(written.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(read).unwrap(),
        written,
        "every field the server writes is read, and nothing is added"
    );
}

#[test]
fn pairing_writes_icecast_webrtc_and_server_settings() {
    let mut e = Engine::open_in_memory().unwrap();
    pair(&mut e, document()).unwrap();
    let doc = server_document();
    let stream = json!(doc.stream_url);
    let expected = [
        ("icecast", "host", json!(doc.icecast.host)),
        ("icecast", "port", json!(doc.icecast.port)),
        ("icecast", "mount", json!(doc.icecast.mount)),
        ("icecast", "user", json!(doc.icecast.user)),
        ("icecast", "tls", json!(doc.icecast.tls)),
        // The Icecast public address is the one listeners open: the stream, not the server.
        ("icecast", "public_url", stream.clone()),
        ("webrtc", "public_stream", stream.clone()),
        (SERVER_PART, "address", json!(ADDRESS)),
        (SERVER_PART, "slug", json!(doc.slug)),
        (SERVER_PART, "name", json!(doc.name)),
        (SERVER_PART, "stream_url", stream),
        (
            SERVER_PART,
            "outputs",
            serde_json::to_value(&doc.outputs).unwrap(),
        ),
        // The paired push, which names the keychain entry of the key.
        (
            SERVER_PART,
            "push",
            json!({
                "host": doc.icecast.host,
                "port": doc.icecast.port,
                "mount": doc.icecast.mount,
                "user": doc.icecast.user,
            }),
        ),
    ];
    assert_eq!(expected.len(), PAIRED.len());
    for (part, key, value) in expected {
        assert_eq!(setting(&e, part, key), Some(value), "{part}.{key}");
    }
    for part in ["icecast", "webrtc", SERVER_PART] {
        for key in e.state().settings(part).unwrap().keys() {
            assert!(
                PAIRED.contains(&(part, key.as_str())),
                "pairing writes {part}.{key} and nothing else"
            );
        }
    }
}

#[test]
fn pairing_again_overwrites_hand_edits() {
    let mut e = Engine::open_in_memory().unwrap();
    pair(&mut e, document()).unwrap();
    set(&mut e, "icecast", "host", json!("hand.example.invalid"));
    set(&mut e, "icecast", "port", json!(9000));
    set(&mut e, SERVER_PART, "slug", json!("typed-by-hand"));
    pair(&mut e, document()).unwrap();
    assert_eq!(
        setting(&e, "icecast", "host"),
        Some(json!("radio.example.invalid"))
    );
    assert_eq!(setting(&e, "icecast", "port"), Some(json!(8443)));
    assert_eq!(setting(&e, SERVER_PART, "slug"), Some(json!("night-owls")));
}

#[test]
fn unpair_restores_the_defaults_pairing_wrote_and_leaves_other_settings() {
    let mut e = Engine::open_in_memory().unwrap();
    set(&mut e, "icecast", "format", json!("mp3"));
    set(&mut e, "icecast", "kbps", json!(128));
    set(&mut e, "webrtc", "port", json!(8090));
    set(&mut e, "branding", "name", json!("Night Owls"));
    pair(&mut e, document()).unwrap();
    e.dispatch(Command::ServerUnpair).unwrap();
    for (part, key) in PAIRED {
        assert_eq!(
            setting(&e, part, key),
            None,
            "{part}.{key} is back to its default"
        );
    }
    assert_eq!(setting(&e, "icecast", "format"), Some(json!("mp3")));
    assert_eq!(setting(&e, "icecast", "kbps"), Some(json!(128)));
    assert_eq!(setting(&e, "webrtc", "port"), Some(json!(8090)));
    assert_eq!(setting(&e, "branding", "name"), Some(json!("Night Owls")));
}

#[test]
fn unpair_without_a_pairing_keeps_a_hand_typed_icecast_setup_and_says_so() {
    let mut e = Engine::open_in_memory().unwrap();
    set(&mut e, "icecast", "host", json!("hand.example.invalid"));
    set(&mut e, "icecast", "mount", json!("/live"));
    let before = e.state().ui_settings().clone();
    let outcome = e.dispatch(Command::ServerUnpair).unwrap();
    assert_eq!(e.state().ui_settings(), &before, "nothing changed");
    let note = outcome.note.expect("the outcome says why nothing changed");
    assert!(note.contains("not paired"), "{note}");
}

#[test]
fn a_newer_document_version_is_refused_and_changes_nothing() {
    let mut e = Engine::open_in_memory().unwrap();
    set(&mut e, "icecast", "host", json!("hand.example.invalid"));
    let before = e.state().ui_settings().clone();
    let mut newer = document();
    newer.version = 2;
    let err = pair(&mut e, newer).unwrap_err().to_string();
    assert!(err.contains("version 2"), "{err}");
    assert!(err.contains("update the app"), "{err}");
    assert_eq!(e.state().ui_settings(), &before, "no setting was written");
}

#[test]
fn the_commands_are_listed_for_the_cli_and_mcp() {
    let specs = registry();
    for id in ["server.pair", "server.unpair"] {
        let spec = specs.iter().find(|s| s.id == id).expect(id);
        assert!(!spec.summary.is_empty(), "{id} has a summary");
        assert_eq!(spec.tier, Tier::OnAir, "{id} changes settings");
    }
    let pair = specs.iter().find(|s| s.id == "server.pair").unwrap();
    let props = pair.args_schema.as_value()["properties"].clone();
    assert!(props.get("address").is_some() && props.get("document").is_some());
    assert!(
        !props.to_string().to_lowercase().contains("key\""),
        "the key is never an argument"
    );
}

#[test]
fn an_unpaired_station_stays_unpaired_after_a_restart() {
    let path = temp_db("unpair-restart");
    {
        let mut e = Engine::open(&path).unwrap();
        set(&mut e, "icecast", "format", json!("mp3"));
        pair(&mut e, document()).unwrap();
        e.flush().unwrap();
        e.dispatch(Command::ServerUnpair).unwrap();
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    for (part, key) in PAIRED {
        assert_eq!(setting(&e, part, key), None, "{part}.{key} stays removed");
    }
    assert_eq!(setting(&e, "icecast", "format"), Some(json!("mp3")));
}
