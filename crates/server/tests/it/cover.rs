//! `PUT /api/station/cover` and `GET /<slug>/cover`: the station's app sets the cover of
//! what plays with its key, and the listener page reads it, unkeyed.

use crate::server::{MORNING_KEY, Tone, on_air};
use crate::support::{Head, PATIENCE, client, read_head, read_to_end_within, request};
use base64::Engine;
use radio_server::{Config, MAX_COVER, Server, hash_key};
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Instant;

const SLUG: &str = "morning";
/// Invented bytes: the server keeps what it is sent and does not decode it.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\ninvented-cover-bytes";
const JPEG: &[u8] = b"\xff\xd8\xffinvented-other-cover";

fn config(state_dir: Option<&Path>) -> Config {
    let state_dir = state_dir
        .map(|dir| format!("state_dir = {:?}\n", dir.display().to_string()))
        .unwrap_or_default();
    Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n{state_dir}\n\
         [[station]]\nslug = \"{SLUG}\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\n",
        hash_key(MORNING_KEY)
    ))
    .unwrap()
}

fn authorization(key: &str) -> String {
    let pair = base64::engine::general_purpose::STANDARD.encode(format!("source:{key}"));
    format!("Authorization: Basic {pair}\r\n")
}

/// `PUT <path>` with `key`, `content_type` and `body`; `length` overrides the
/// `Content-Length` (`None` sends none).
fn put_to(
    addr: SocketAddr,
    path: &str,
    key: &str,
    content_type: &str,
    body: &[u8],
    length: Option<usize>,
) -> (Head, Vec<u8>) {
    let mut stream = client(addr);
    let length = length
        .map(|length| format!("Content-Length: {length}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "PUT {path} HTTP/1.1\r\nHost: radio.example.test\r\n{}Content-Type: {content_type}\r\n\
         {length}\r\n",
        authorization(key)
    )
    .unwrap();
    // A server that answers before the whole body is sent may close early.
    let _ = stream.write_all(body);
    let head = read_head(&mut stream);
    let body = read_to_end_within(&mut stream, PATIENCE);
    (head, body)
}

fn put(addr: SocketAddr, key: &str, content_type: &str, body: &[u8]) -> u16 {
    put_to(
        addr,
        "/api/station/cover",
        key,
        content_type,
        body,
        Some(body.len()),
    )
    .0
    .status
}

/// `<method> /morning/cover`, with an `If-None-Match` when given.
fn get(addr: SocketAddr, method: &str, if_none_match: Option<&str>) -> (Head, Vec<u8>) {
    let mut stream = client(addr);
    let tag = if_none_match
        .map(|tag| format!("If-None-Match: {tag}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "{method} /{SLUG}/cover HTTP/1.1\r\nHost: radio.example.test\r\n{tag}\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    let body = read_to_end_within(&mut stream, PATIENCE);
    (head, body)
}

#[test]
fn a_cover_put_with_the_key_is_served_with_its_type_and_tag() {
    let server = Server::start(config(None)).unwrap();
    let addr = server.local_addr();
    let (none, _) = get(addr, "GET", None);
    assert_eq!(none.status, 404, "no cover before one is set");
    assert_eq!(none.header("Access-Control-Allow-Origin"), Some("*"));

    // The type is the server's own word, whatever the case and parameters sent.
    assert_eq!(
        put(addr, MORNING_KEY, "Image/PNG; charset=binary", PNG),
        204
    );
    let (head, body) = get(addr, "GET", None);
    assert_eq!(head.status, 200);
    assert_eq!(body, PNG);
    assert_eq!(head.header("Content-Type"), Some("image/png"));
    assert_eq!(head.header("Cache-Control"), Some("no-cache"));
    assert_eq!(head.header("X-Content-Type-Options"), Some("nosniff"));
    assert_eq!(head.header("Access-Control-Allow-Origin"), Some("*"));
    let tag = head.header("ETag").expect("an ETag").to_owned();

    let (same, body) = get(addr, "GET", Some(&tag));
    assert_eq!(same.status, 304, "the page's copy is the cover");
    assert!(body.is_empty());
    let (head_only, body) = get(addr, "HEAD", None);
    assert_eq!(head_only.status, 200);
    assert!(body.is_empty(), "a HEAD gets no body");
    assert_eq!(
        head_only.header("Content-Length"),
        Some(PNG.len().to_string().as_str())
    );

    assert_eq!(put(addr, MORNING_KEY, "image/jpeg", JPEG), 204);
    let (next, body) = get(addr, "GET", Some(&tag));
    assert_eq!(next.status, 200, "an old tag gets the new cover");
    assert_eq!(body, JPEG);
    assert_eq!(next.header("Content-Type"), Some("image/jpeg"));
    assert_ne!(
        next.header("ETag"),
        Some(tag.as_str()),
        "a new cover, a new tag"
    );
}

#[test]
fn an_empty_body_clears_the_cover() {
    let server = Server::start(config(None)).unwrap();
    let addr = server.local_addr();
    assert_eq!(put(addr, MORNING_KEY, "image/png", PNG), 204);
    // No type is needed to clear.
    assert_eq!(put(addr, MORNING_KEY, "application/octet-stream", b""), 204);
    assert_eq!(get(addr, "GET", None).0.status, 404);
}

#[test]
fn the_end_of_the_source_clears_the_cover() {
    let server = Server::start(config(None)).unwrap();
    let addr = server.local_addr();
    let pusher = on_air(addr, SLUG, MORNING_KEY, Tone::Ogg);
    assert_eq!(put(addr, MORNING_KEY, "image/png", PNG), 204);
    assert_eq!(get(addr, "GET", None).0.status, 200);
    drop(pusher);
    let deadline = Instant::now() + PATIENCE;
    while get(addr, "GET", None).0.status != 404 {
        assert!(
            Instant::now() < deadline,
            "the cover outlived the source that played it"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn covers_of_another_type_too_large_or_without_a_length_are_refused() {
    let server = Server::start(config(None)).unwrap();
    let addr = server.local_addr();
    assert_eq!(put(addr, MORNING_KEY, "image/png", PNG), 204);
    assert_eq!(put(addr, MORNING_KEY, "image/gif", b"GIF89a-invented"), 415);
    assert_eq!(put(addr, MORNING_KEY, "text/html", b"<p>invented</p>"), 415);
    let (too_large, _) = put_to(
        addr,
        "/api/station/cover",
        MORNING_KEY,
        "image/png",
        b"",
        Some(MAX_COVER + 1),
    );
    assert_eq!(too_large.status, 413);
    let (no_length, _) = put_to(
        addr,
        "/api/station/cover",
        MORNING_KEY,
        "image/png",
        b"",
        None,
    );
    assert_eq!(no_length.status, 411);
    let (_, body) = get(addr, "GET", None);
    assert_eq!(body, PNG, "a refused PUT leaves the cover as it was");

    // The largest cover allowed is taken.
    let largest = vec![7u8; MAX_COVER];
    assert_eq!(put(addr, MORNING_KEY, "image/webp", &largest), 204);
    let (head, body) = get(addr, "GET", None);
    assert_eq!(head.header("Content-Type"), Some("image/webp"));
    assert_eq!(body.len(), MAX_COVER);
}

#[test]
fn a_wrong_key_is_401_counts_toward_the_lockout_and_changes_nothing() {
    let server = Server::start(config(None)).unwrap();
    let addr = server.local_addr();
    let mut refused = 0;
    loop {
        let status = put(addr, "invented-wrong-key", "image/png", PNG);
        if status == 429 {
            break;
        }
        assert_eq!(status, 401);
        refused += 1;
        assert!(refused <= 10, "wrong keys never locked the address out");
    }
    assert!(refused >= 1);
    assert_eq!(get(addr, "GET", None).0.status, 404, "no cover was set");
    // Locked out: the right key waits too.
    assert_eq!(put(addr, MORNING_KEY, "image/png", PNG), 429);
}

#[test]
fn the_cover_is_not_an_output_mount() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()))).unwrap();
    let addr = server.local_addr();
    let body = br#"[{"mount": "cover", "format": "opus", "kbps": 64}]"#;
    let (head, reason) = put_to(
        addr,
        "/api/station/outputs",
        MORNING_KEY,
        "application/json",
        body,
        Some(body.len()),
    );
    assert_eq!(head.status, 400, "{}", String::from_utf8_lossy(&reason));
    // The path stays the cover's: unkeyed, it names no stream.
    let (_, head) = request(addr, "GET", &format!("/{SLUG}/cover"));
    assert_eq!(head.status, 404);
}
