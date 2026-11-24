//! `GET /api/station`: a station's key gets that station's pairing document.

use crate::support::{Head, PATIENCE, client, read_head, read_to_end_within};
use base64::Engine;
use radio_server::pairing::{Document, PublicUrl};
use radio_server::{Config, Server, ServerHandle, hash_key};
use std::io::Write;
use std::net::SocketAddr;

const DAWN_KEY: &str = "invented-dawn-key-7Kp2";
const DUSK_KEY: &str = "invented-dusk-key-3Wm5";

/// A server with the stations "dawn" (two outputs, not in name order) and "dusk" (none).
fn config(public_url: &str) -> String {
    format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"{public_url}\"\n\n\
         [[station]]\nslug = \"dawn\"\nname = \"Dawn Chorus\"\nkey_hash = \"{}\"\n\n\
         [[station.output]]\nmount = \"opus\"\nformat = \"opus\"\nkbps = 64\n\n\
         [[station.output]]\nmount = \"mp3\"\nformat = \"mp3\"\nkbps = 128\n\n\
         [[station]]\nslug = \"dusk\"\nname = \"Dusk Signal\"\nkey_hash = \"{}\"\n",
        hash_key(DAWN_KEY),
        hash_key(DUSK_KEY),
    )
}

fn start() -> ServerHandle {
    Server::start(Config::parse(&config("https://radio.example.test")).unwrap()).unwrap()
}

/// `GET /api/station` with `key` as the Basic password of `user`, or with no
/// `Authorization` header; the reply head and body.
fn ask(addr: SocketAddr, user: &str, key: Option<&str>) -> (Head, Vec<u8>) {
    let auth = key
        .map(|key| {
            let pair = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{key}"));
            format!("Authorization: Basic {pair}\r\n")
        })
        .unwrap_or_default();
    let mut stream = client(addr);
    write!(
        stream,
        "GET /api/station HTTP/1.1\r\nHost: radio.example.test\r\n{auth}\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    let body = read_to_end_within(&mut stream, PATIENCE);
    (head, body)
}

fn document(addr: SocketAddr, key: &str) -> Document {
    let (head, body) = ask(addr, "source", Some(key));
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Type"), Some("application/json"));
    serde_json::from_slice(&body).expect("the pairing document is JSON")
}

#[test]
fn the_station_key_gets_its_pairing_document() {
    let server = start();
    let addr = server.local_addr();

    let dawn = document(addr, DAWN_KEY);
    assert_eq!(dawn.version, 1);
    assert_eq!(dawn.slug, "dawn");
    assert_eq!(dawn.name, "Dawn Chorus");
    assert_eq!(dawn.icecast.mount, "/dawn/stream");
    assert_eq!(dawn.icecast.host, "radio.example.test");
    assert!(dawn.icecast.tls);
    assert_eq!(dawn.icecast.port, 443);
    assert_eq!(dawn.icecast.user, "source");
    assert!(dawn.stream_url.starts_with(&dawn.public_url));
    assert!(dawn.stream_url.ends_with("/dawn/stream"));
    let mounts: Vec<&str> = dawn.outputs.iter().map(|o| o.mount.as_str()).collect();
    assert_eq!(mounts, ["opus", "mp3"], "outputs come in config order");
    let mp3 = &dawn.outputs[1];
    assert_eq!((mp3.format.as_str(), mp3.kbps), ("mp3", 128));
    assert!(mp3.url.ends_with("/dawn/mp3"));
    assert!(mp3.url.starts_with(&dawn.public_url));

    // Any user name goes with the key.
    let (head, body) = ask(addr, "anyone", Some(DUSK_KEY));
    assert_eq!(head.status, 200);
    let dusk: Document = serde_json::from_slice(&body).unwrap();
    assert_eq!(dusk.slug, "dusk");
    assert_eq!(dusk.icecast.mount, "/dusk/stream");
    assert!(dusk.stream_url.ends_with("/dusk/stream"));
    assert!(dusk.outputs.is_empty());
}

#[test]
fn a_wrong_key_is_refused_and_counts_toward_the_lockout() {
    let server = start();
    let addr = server.local_addr();
    let mut refused = 0;
    loop {
        let (head, body) = ask(addr, "source", Some("invented-wrong-key"));
        if head.status == 429 {
            assert!(head.header("Retry-After").is_some());
            break;
        }
        assert_eq!(head.status, 401);
        assert_eq!(
            head.header("WWW-Authenticate"),
            Some("Basic realm=\"craftradio\"")
        );
        assert!(!String::from_utf8_lossy(&body).contains("dawn"));
        refused += 1;
        assert!(refused <= 10, "wrong keys never locked the address out");
    }
    assert!(refused >= 1, "the first wrong key is answered 401");
    // Locked out: even the right key waits now, here and on a push.
    assert_eq!(ask(addr, "source", Some(DAWN_KEY)).0.status, 429);
}

#[test]
fn no_key_is_refused_without_counting() {
    let server = start();
    let addr = server.local_addr();
    for _ in 0..12 {
        let (head, _) = ask(addr, "source", None);
        assert_eq!(head.status, 401);
        assert_eq!(
            head.header("WWW-Authenticate"),
            Some("Basic realm=\"craftradio\"")
        );
    }
    assert_eq!(document(addr, DAWN_KEY).slug, "dawn");
}

#[test]
fn the_document_never_contains_the_key_or_its_hash() {
    let server = start();
    let addr = server.local_addr();
    let mut bodies = Vec::new();
    for key in [DAWN_KEY, DUSK_KEY] {
        let (head, body) = ask(addr, "source", Some(key));
        assert_eq!(head.status, 200);
        bodies.push(String::from_utf8(body).unwrap());
    }
    bodies.push(String::from_utf8(ask(addr, "source", Some("invented-wrong-key")).1).unwrap());
    for body in &bodies {
        for key in [DAWN_KEY, DUSK_KEY] {
            let hash = hash_key(key);
            let hex = hash.trim_start_matches("sha256:");
            assert!(!body.contains(key), "a reply holds a key");
            assert!(
                !body.to_lowercase().contains(hex),
                "a reply holds a key's hash"
            );
        }
    }
}

#[test]
fn the_push_target_follows_the_public_url() {
    let plain = PublicUrl::parse("http://radio.example.test/").unwrap();
    assert!(!plain.tls);
    assert_eq!(plain.port, 80);
    assert_eq!(plain.base, "http://radio.example.test");

    let ported = PublicUrl::parse("https://radio.example.test:8443").unwrap();
    assert!(ported.tls);
    assert_eq!(ported.port, 8443);
    assert_eq!(ported.host, "radio.example.test");

    let v6 = PublicUrl::parse("http://[2001:db8::7]:8000").unwrap();
    assert_eq!((v6.host.as_str(), v6.port), ("2001:db8::7", 8000));

    let config = Config::parse(&config("http://radio.example.test:8000/")).unwrap();
    let url = PublicUrl::parse(&config.public_url).unwrap();
    let doc = Document::for_station(&url, &config.stations[0]);
    assert_eq!(doc.icecast.port, 8000);
    assert!(!doc.icecast.tls);
    assert!(!doc.stream_url.contains("//dawn"), "no doubled slash");
    assert!(
        doc.stream_url
            .starts_with("http://radio.example.test:8000/")
    );
}

#[test]
fn a_public_url_the_app_could_not_push_to_is_refused_at_load() {
    for bad in [
        "radio.example.test",
        "ftp://radio.example.test",
        "https://radio.example.test/listen",
        "https://radio.example.test:0",
        "https://radio.example.test:99999",
        "https://",
    ] {
        let error = Config::parse(&config(bad)).err();
        assert!(
            error.is_some_and(|e| e.to_string().contains("public_url")),
            "{bad:?} was taken"
        );
    }
}
