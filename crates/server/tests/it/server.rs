use crate::framing::{OggSource, granule, pages};
use crate::support::{
    Head, PATIENCE, client, entry, hears, read_head, read_head_bytes, read_to_end_within, request,
    source_entry, sources, stats, tls_files,
};
use base64::Engine;
use craft_audio::mp3::{Config as Mp3Config, Encoder};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use radio_server::{
    Config, Format, MountStats, Server, ServerError, ServerHandle, StatsEntry, Timeouts, hash_key,
    status_json,
};
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) const MORNING_KEY: &str = "invented-morning-key-4Hq8";
const EVENING_KEY: &str = "invented-evening-key-9Zt3";

/// A server with the stations "morning" and "evening".
fn config(max_stream_listeners: usize, trusted_proxy: Option<&str>) -> Config {
    let proxy = trusted_proxy
        .map(|ip| format!("trusted_proxy = \"{ip}\"\n"))
        .unwrap_or_default();
    Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n{proxy}\n\
         [[station]]\nslug = \"morning\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\nmax_stream_listeners = {max_stream_listeners}\n\n\
         [[station]]\nslug = \"evening\"\nname = \"Evening Station\"\nkey_hash = \"{}\"\n",
        hash_key(MORNING_KEY),
        hash_key(EVENING_KEY),
    ))
    .unwrap()
}

fn start() -> ServerHandle {
    Server::start(config(200, None)).unwrap()
}

fn start_with(timeouts: Timeouts) -> ServerHandle {
    Server::start_with(config(200, None), timeouts).unwrap()
}

/// The `Authorization` line the app sends with `key`.
fn basic(key: &str) -> String {
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("source:{key}"));
    format!("Authorization: Basic {auth}\r\n")
}

/// Sends a source push's head the way the app's `connect()` does; `None` sends no
/// `Authorization` header.
fn send_push(
    addr: SocketAddr,
    slug: &str,
    key: Option<&str>,
    content_type: &str,
    extra: &str,
) -> TcpStream {
    let mut stream = client(addr);
    stream
        .write_all(push_head(slug, key, content_type, extra).as_bytes())
        .unwrap();
    stream
}

/// A source push's head as the app's `connect()` writes it.
fn push_head(slug: &str, key: Option<&str>, content_type: &str, extra: &str) -> String {
    let auth = key.map(basic).unwrap_or_default();
    format!(
        "PUT /{slug}/stream HTTP/1.0\r\nHost: radio.example.test\r\n{auth}\
         Content-Type: {content_type}\r\nContent-Length: 9223372036854775807\r\n\
         Ice-Name: Cloud Signal FM\r\nIce-Description: Invented test stream\r\nIce-Genre: Ambient\r\n\
         Ice-Bitrate: 96\r\n{extra}\r\n"
    )
}

/// Starts a source push and reads the reply head.
fn push_with(
    addr: SocketAddr,
    slug: &str,
    key: &str,
    content_type: &str,
    extra: &str,
) -> (TcpStream, Head) {
    let mut stream = send_push(addr, slug, Some(key), content_type, extra);
    let head = read_head(&mut stream);
    (stream, head)
}

/// Sends `GET /admin/metadata?<query>` the way the app's `update_mp3_title` does and
/// reads the reply head; `None` sends no `Authorization` header.
fn metadata_query(addr: SocketAddr, query: &str, key: Option<&str>) -> (TcpStream, Head) {
    let mut stream = client(addr);
    let auth = key.map(basic).unwrap_or_default();
    write!(
        stream,
        "GET /admin/metadata?{query} HTTP/1.0\r\nHost: radio.example.test\r\n{auth}Connection: close\r\n\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    (stream, head)
}

/// A title update for `/<slug>/stream`, encoded as the app encodes it.
pub(crate) fn set_title(
    addr: SocketAddr,
    slug: &str,
    title: &str,
    key: Option<&str>,
) -> (TcpStream, Head) {
    let query = format!(
        "mount={}&mode=updinfo&song={}",
        utf8_percent_encode(&format!("/{slug}/stream"), NON_ALPHANUMERIC),
        utf8_percent_encode(title, NON_ALPHANUMERIC),
    );
    metadata_query(addr, &query, key)
}

fn push(addr: SocketAddr, slug: &str, key: &str, content_type: &str) -> (TcpStream, Head) {
    push_with(addr, slug, key, content_type, "")
}

/// Pushes until the mount takes the source: the server notices a closed source a moment
/// after the close.
pub(crate) fn push_when_free(
    addr: SocketAddr,
    slug: &str,
    key: &str,
    content_type: &str,
) -> TcpStream {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (stream, head) = push(addr, slug, key, content_type);
        if head.status == 200 {
            return stream;
        }
        assert_eq!(head.status, 403, "only a busy mount refuses this source");
        assert!(Instant::now() < deadline, "the mount never became free");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn get(addr: SocketAddr, slug: &str) -> (TcpStream, Head) {
    request(addr, "GET", &format!("/{slug}/stream"))
}

/// Polls the stats until `slug`'s entry satisfies `done`, and returns that entry.
fn wait_for_entry(
    addr: SocketAddr,
    slug: &str,
    done: impl Fn(Option<&Value>) -> bool,
) -> Option<Value> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let entry = source_entry(&stats(addr), slug);
        if done(entry.as_ref()) {
            return entry;
        }
        assert!(
            Instant::now() < deadline,
            "the stats never showed it: {entry:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn status_of(head: (TcpStream, Head)) -> u16 {
    head.1.status
}

fn mp3_frames(count: usize) -> Vec<u8> {
    // MPEG-1 Layer III, 128 kbit/s, 48 kHz: 384 bytes a frame.
    let mut frame = vec![0xFF, 0xFB, 0x94, 0xC4];
    frame.resize(384, 0x55);
    frame.repeat(count)
}

#[test]
fn the_right_key_gets_an_explicit_200_and_the_wrong_one_401() {
    let server = start();
    let addr = server.local_addr();

    let (_wrong, head) = push(addr, "morning", "invented-wrong-key", "audio/ogg");
    assert_eq!(head.status, 401);
    assert_eq!(
        head.header("WWW-Authenticate"),
        Some("Basic realm=\"craftradio\"")
    );

    let mut right = send_push(addr, "morning", Some(MORNING_KEY), "audio/ogg", "");
    assert_eq!(read_head_bytes(&mut right), b"HTTP/1.0 200 OK\r\n\r\n");
}

#[test]
fn a_request_without_a_key_is_401_and_never_locks_the_address() {
    let server = start();
    let addr = server.local_addr();
    let (_source, head) = push(addr, "evening", EVENING_KEY, "audio/mpeg");
    assert_eq!(head.status, 200);
    for _ in 0..6 {
        let mut keyless = send_push(addr, "morning", None, "audio/ogg", "");
        let head = read_head(&mut keyless);
        assert_eq!(head.status, 401);
        assert_eq!(
            head.header("WWW-Authenticate"),
            Some("Basic realm=\"craftradio\"")
        );
        let (_keyless, head) = set_title(addr, "evening", "Cloud Signal", None);
        assert_eq!(head.status, 401);
    }
    assert_eq!(
        status_of(set_title(
            addr,
            "evening",
            "Cloud Signal",
            Some(EVENING_KEY)
        )),
        200
    );
    assert_eq!(
        status_of(push(addr, "morning", MORNING_KEY, "audio/ogg")),
        200
    );
}

#[test]
fn a_title_update_with_the_station_key_is_taken() {
    let server = start();
    let addr = server.local_addr();
    let (_source, head) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    assert_eq!(head.status, 200);

    let (mut taken, head) = set_title(addr, "morning", "Cloud Signal", Some(MORNING_KEY));
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Type"), Some("text/xml"));
    assert_eq!(
        read_to_end_within(&mut taken, PATIENCE),
        b"<?xml version=\"1.0\"?><iceresponse><message>Metadata update successful</message><return>1</return></iceresponse>"
    );

    let mount = utf8_percent_encode("/morning/stream", NON_ALPHANUMERIC).to_string();
    let refused = [
        // An unknown station.
        (
            "mount=%2Fnoon%2Fstream&mode=updinfo&song=Echo".to_owned(),
            404,
        ),
        // Not a stream path.
        ("mount=%2Fmorning&mode=updinfo&song=Echo".to_owned(), 404),
        (format!("mount={mount}&mode=updinfo"), 400),
        (format!("mount={mount}&mode=updmeta&song=Echo"), 400),
        (format!("mount={mount}&song=Echo"), 400),
        ("mode=updinfo&song=Echo".to_owned(), 400),
    ];
    for (query, status) in refused {
        assert_eq!(
            status_of(metadata_query(addr, &query, Some(MORNING_KEY))),
            status,
            "{query}"
        );
    }
    // A station with no source live.
    assert_eq!(
        status_of(set_title(addr, "evening", "Echo", Some(EVENING_KEY))),
        400
    );
}

#[test]
fn a_title_update_needs_the_key_of_that_station() {
    let server = start();
    let addr = server.local_addr();
    let (_source, _) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    let title = || source_entry(&stats(addr), "morning").unwrap()["title"].clone();
    assert_eq!(title(), "");

    let (_other, head) = set_title(addr, "morning", "Cloud Signal", Some(EVENING_KEY));
    assert_eq!(head.status, 401);
    assert_eq!(
        head.header("WWW-Authenticate"),
        Some("Basic realm=\"craftradio\"")
    );
    assert_eq!(title(), "", "a refused update changed the title");
    assert_eq!(
        status_of(set_title(
            addr,
            "morning",
            "Cloud Signal",
            Some("invented-wrong-key")
        )),
        401
    );
    assert_eq!(title(), "", "a refused update changed the title");
    assert_eq!(
        status_of(set_title(
            addr,
            "morning",
            "Cloud Signal",
            Some(MORNING_KEY)
        )),
        200
    );
    assert_eq!(title(), "Cloud Signal");
}

#[test]
fn five_wrong_keys_on_title_updates_lock_the_address() {
    let server = start();
    let addr = server.local_addr();
    let (_source, _) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    // An unknown mount is not a wrong key.
    for _ in 0..6 {
        let query = "mount=%2Fnoon%2Fstream&mode=updinfo&song=Echo";
        assert_eq!(
            status_of(metadata_query(addr, query, Some("invented-wrong-key"))),
            404
        );
    }
    assert_eq!(
        status_of(set_title(addr, "morning", "Echo", Some(MORNING_KEY))),
        200
    );

    for _ in 0..5 {
        assert_eq!(
            status_of(set_title(
                addr,
                "morning",
                "Echo",
                Some("invented-wrong-key")
            )),
            401
        );
    }
    let (_locked, head) = set_title(addr, "morning", "Echo", Some(MORNING_KEY));
    assert_eq!(head.status, 429);
    assert!(head.header("Retry-After").is_some());
    // The lock covers the source route too.
    assert_eq!(
        status_of(push(addr, "evening", EVENING_KEY, "audio/ogg")),
        429
    );
}

#[test]
fn an_unknown_content_type_is_415() {
    let server = start();
    let (_source, head) = push(server.local_addr(), "morning", MORNING_KEY, "text/plain");
    assert_eq!(head.status, 415);
    // The refused source did not take the mount.
    let (_source, head) = push(server.local_addr(), "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
}

#[test]
fn a_listener_hears_what_the_source_sends() {
    let server = start();
    let addr = server.local_addr();
    let (mut source, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    let mut ogg = OggSource::new("Cloud Signal", Some("Echo"));
    ogg.audio(200);
    let before = ogg.take();
    source.write_all(&before).unwrap();
    let last_before = granule(pages(&before).last().unwrap());

    let (mut listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Type"), Some("audio/ogg"));
    assert_eq!(head.header("Cache-Control"), Some("no-cache, no-store"));
    assert_eq!(head.header("Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(head.header("Content-Length"), None);

    ogg.audio(400);
    let after = ogg.take();
    // Small writes, as a live source sends them.
    for part in after.chunks(700) {
        source.write_all(part).unwrap();
    }

    let deadline = Instant::now() + PATIENCE;
    let mut heard = Vec::new();
    let mut buf = [0; 4096];
    while !pages(&heard).iter().any(|page| granule(page) > last_before) {
        assert!(
            Instant::now() < deadline,
            "no page sent after the join arrived"
        );
        let n = listener.read(&mut buf).unwrap();
        assert!(n > 0, "the stream ended early");
        heard.extend_from_slice(&buf[..n]);
    }
    assert!(heard.starts_with(b"OggS"));
}

#[test]
fn an_unknown_station_is_404_for_source_and_listener() {
    let server = start();
    let addr = server.local_addr();
    assert_eq!(status_of(push(addr, "noon", MORNING_KEY, "audio/ogg")), 404);
    assert_eq!(status_of(get(addr, "noon")), 404);
    // A known station with no source is 404 to a listener too.
    assert_eq!(status_of(get(addr, "morning")), 404);
}

#[test]
fn other_methods_and_paths_are_405_and_404() {
    let server = start();
    let addr = server.local_addr();
    assert_eq!(status_of(request(addr, "DELETE", "/morning/stream")), 405);
    assert_eq!(status_of(request(addr, "GET", "/morning")), 404);
    assert_eq!(status_of(request(addr, "GET", "/morning/stream/more")), 404);
    // PUT elsewhere is 404, never 405, which the app would take as "try SOURCE".
    assert_eq!(status_of(request(addr, "PUT", "/morning/other")), 404);
    let (_options, head) = request(addr, "OPTIONS", "/morning/stream");
    assert_eq!(head.status, 204);
    assert_eq!(head.header("Access-Control-Allow-Origin"), Some("*"));
}

#[test]
fn the_icecast_source_method_is_taken_like_put() {
    let server = start();
    let mut stream = client(server.local_addr());
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("source:{MORNING_KEY}"));
    write!(
        stream,
        "SOURCE /morning/stream HTTP/1.0\r\nAuthorization: Basic {auth}\r\nContent-Type: audio/mpeg\r\n\r\n"
    )
    .unwrap();
    assert_eq!(read_head(&mut stream).status, 200);
    let (_listener, head) = get(server.local_addr(), "morning");
    assert_eq!(head.header("Content-Type"), Some("audio/mpeg"));
}

#[test]
fn a_second_source_gets_403_while_the_first_is_live_and_200_after_it_ends() {
    let server = start();
    let addr = server.local_addr();
    let (first, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);

    let (mut second, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 403);
    let body = read_to_end_within(&mut second, PATIENCE);
    assert_eq!(body, b"this mount already has a source");
    // Another station's mount is not busy.
    assert_eq!(
        status_of(push(addr, "evening", EVENING_KEY, "audio/ogg")),
        200
    );

    drop(first);
    let _third = push_when_free(addr, "morning", MORNING_KEY, "audio/ogg");
}

#[test]
fn five_wrong_keys_then_the_right_key_is_429() {
    // The proxy is the loopback peer, so an `X-Forwarded-For` line gives a second client
    // address; std cannot bind a client socket to 127.0.0.2 before connecting.
    let server = Server::start(config(200, Some("127.0.0.1"))).unwrap();
    let addr = server.local_addr();
    for _ in 0..5 {
        assert_eq!(
            status_of(push(addr, "morning", "invented-wrong-key", "audio/ogg")),
            401
        );
    }
    let (_locked, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 429);
    // The lock covers every station, and a wrong key while locked is still 429.
    assert_eq!(
        status_of(push(addr, "evening", EVENING_KEY, "audio/ogg")),
        429
    );
    assert_eq!(
        status_of(push(addr, "morning", "invented-wrong-key", "audio/ogg")),
        429
    );

    let other = "X-Forwarded-For: 192.0.2.7\r\n";
    let (_other, head) = push_with(addr, "morning", MORNING_KEY, "audio/ogg", other);
    assert_eq!(head.status, 200, "another address is not locked");
}

#[test]
fn a_forwarded_address_is_ignored_without_a_trusted_proxy() {
    let server = start();
    let addr = server.local_addr();
    for _ in 0..5 {
        assert_eq!(
            status_of(push(addr, "morning", "invented-wrong-key", "audio/ogg")),
            401
        );
    }
    // A client cannot step out of its lockout by naming another address.
    let forged = "X-Forwarded-For: 192.0.2.7\r\n";
    let (_forged, head) = push_with(addr, "morning", MORNING_KEY, "audio/ogg", forged);
    assert_eq!(head.status, 429);
}

#[test]
fn an_unknown_station_is_not_a_wrong_key() {
    let server = start();
    let addr = server.local_addr();
    for _ in 0..6 {
        assert_eq!(
            status_of(push(addr, "noon", "invented-wrong-key", "audio/ogg")),
            404
        );
    }
    assert_eq!(
        status_of(push(addr, "morning", MORNING_KEY, "audio/ogg")),
        200
    );
}

#[test]
fn a_wrong_key_on_one_station_does_not_open_another() {
    let server = start();
    let addr = server.local_addr();
    assert_eq!(
        status_of(push(addr, "evening", MORNING_KEY, "audio/ogg")),
        401
    );
    assert_eq!(
        status_of(push(addr, "morning", EVENING_KEY, "audio/ogg")),
        401
    );
    assert_eq!(
        status_of(push(addr, "evening", EVENING_KEY, "audio/ogg")),
        200
    );
}

#[test]
fn a_source_that_goes_silent_frees_the_mount() {
    let server = start_with(Timeouts {
        source_silence: Duration::from_millis(200),
        ..Timeouts::default()
    });
    let addr = server.local_addr();
    let (mut source, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    let mut ogg = OggSource::new("Cloud Signal", None);
    let first = ogg.take();
    source.write_all(pages(&first)[0]).unwrap();
    let (mut listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);

    // The source stays connected but sends nothing more.
    let started = Instant::now();
    read_to_end_within(&mut listener, Duration::from_secs(2));
    assert!(started.elapsed() < Duration::from_secs(2));
    let (_next, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    drop(source);
}

#[test]
fn a_source_that_returns_with_another_format_ends_old_listeners() {
    let server = start();
    let addr = server.local_addr();
    let (mut source, _) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    let mut ogg = OggSource::new("Cloud Signal", None);
    ogg.audio(100);
    source.write_all(&ogg.take()).unwrap();
    let (mut old_listener, head) = get(addr, "morning");
    assert_eq!(head.header("Content-Type"), Some("audio/ogg"));
    ogg.audio(100);
    source.write_all(&ogg.take()).unwrap();
    drop(source);

    let mut mp3 = push_when_free(addr, "morning", MORNING_KEY, "audio/mpeg");
    mp3.write_all(&mp3_frames(50)).unwrap();
    let (_new_listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Type"), Some("audio/mpeg"));
    mp3.write_all(&mp3_frames(50)).unwrap();

    let heard = read_to_end_within(&mut old_listener, PATIENCE);
    let whole: usize = pages(&heard).iter().map(|page| page.len()).sum();
    assert_eq!(whole, heard.len(), "the old listener got only Ogg pages");
}

#[test]
fn the_listener_cap_answers_503() {
    let server = Server::start(config(1, None)).unwrap();
    let addr = server.local_addr();
    let (mut source, _) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    let (first, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    assert_eq!(status_of(get(addr, "morning")), 503);
    // The cap is per station.
    let (_evening, _) = push(addr, "evening", EVENING_KEY, "audio/mpeg");
    assert_eq!(status_of(get(addr, "evening")), 200);

    // The server learns the first listener left when a write to it fails.
    drop(first);
    let deadline = Instant::now() + PATIENCE;
    while status_of(get(addr, "morning")) != 200 {
        assert!(Instant::now() < deadline, "the place was never freed");
        source.write_all(&mp3_frames(4)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_slow_head_is_closed_at_the_deadline() {
    let server = start_with(Timeouts {
        head: Duration::from_millis(200),
        ..Timeouts::default()
    });
    let addr = server.local_addr();
    let mut slow = client(addr);
    slow.write_all(b"GET /mor").unwrap();

    // Served while the slow head waits.
    assert_eq!(status_of(request(addr, "OPTIONS", "/morning/stream")), 204);

    let started = Instant::now();
    let rest = read_to_end_within(&mut slow, Duration::from_secs(2));
    assert!(rest.is_empty() || rest.starts_with(b"HTTP/1.0 4"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn stop_ends_live_sources_and_listeners_at_once() {
    let server = start();
    let addr = server.local_addr();
    let (mut source, _) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    source.write_all(&mp3_frames(10)).unwrap();
    let (mut listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    let mut slow = client(addr);
    slow.write_all(b"GET /mor").unwrap();

    let started = Instant::now();
    server.stop();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stop took too long"
    );
    read_to_end_within(&mut listener, Duration::from_secs(2));
    read_to_end_within(&mut source, Duration::from_secs(2));
    read_to_end_within(&mut slow, Duration::from_secs(2));
    assert!(
        TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err(),
        "a stopped server no longer listens"
    );
    drop(server);
}

/// A server with one station, "morning", whose every request comes through the trusted
/// proxy at 127.0.0.1, so no limit per address applies; heads wait `head`.
fn behind_proxy(max_stream_listeners: usize, head: Duration) -> ServerHandle {
    behind_proxy_with(
        max_stream_listeners,
        Timeouts {
            head,
            ..Timeouts::default()
        },
    )
}

/// As [`behind_proxy`], with every timeout given.
fn behind_proxy_with(max_stream_listeners: usize, timeouts: Timeouts) -> ServerHandle {
    let config = Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\
         trusted_proxy = \"127.0.0.1\"\n\n\
         [[station]]\nslug = \"morning\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\nmax_stream_listeners = {max_stream_listeners}\n",
        hash_key(MORNING_KEY),
    ))
    .unwrap();
    Server::start_with(config, timeouts).unwrap()
}

/// Clients that send part of a head and nothing more; each holds a connection thread.
fn idle_clients(addr: SocketAddr, count: usize) -> Vec<TcpStream> {
    (0..count)
        .map(|_| {
            let mut stream = client(addr);
            stream.write_all(b"GET /mor").unwrap();
            stream
        })
        .collect()
}

/// Everything the server answers an `OPTIONS` request, which is empty when it closed the
/// connection unanswered.
fn options_reply(addr: SocketAddr) -> Vec<u8> {
    let mut stream = client(addr);
    let _ = stream.write_all(b"OPTIONS /morning/stream HTTP/1.0\r\n\r\n");
    read_to_end_within(&mut stream, PATIENCE)
}

/// Polls `OPTIONS` until its reply starts with `status_line`.
fn wait_for_options_reply(addr: SocketAddr, status_line: &[u8], why: &str) {
    let deadline = Instant::now() + PATIENCE;
    while !options_reply(addr).starts_with(status_line) {
        assert!(Instant::now() < deadline, "{why}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn connections_over_the_thread_cap_cut_off_the_oldest_head() {
    // One station that takes no listeners: 32 public connection threads and 2 kept for
    // its source. Idle heads never time out during the test, so only the server cutting
    // one off or the clients it drops can free a thread.
    let server = behind_proxy(0, Duration::from_secs(600));
    let addr = server.local_addr();
    // Connections are accepted, and given a thread, in the order they were made.
    let mut idle = idle_clients(addr, 34);

    // A connection over the cap is let in, in place of the oldest head still coming; the
    // public part is full, so it is refused with a status line.
    assert!(options_reply(addr).starts_with(b"HTTP/1.0 503"));
    assert!(
        read_to_end_within(&mut idle[0], PATIENCE).is_empty(),
        "the oldest head was answered"
    );
    idle[1].set_nonblocking(true).unwrap();
    let mut byte = [0];
    assert_eq!(
        idle[1].read(&mut byte).map_err(|e| e.kind()).err(),
        Some(ErrorKind::WouldBlock),
        "a younger head was cut off too"
    );

    // With the cut-off head and two more gone, the public part has room again.
    drop(idle.pop());
    drop(idle.pop());
    wait_for_options_reply(addr, b"HTTP/1.0 204", "the freed thread was never reused");
}

#[test]
fn idle_heads_do_not_keep_a_listener_out_for_good() {
    // A cap of 1 listener: 33 public connection threads and 2 for the source.
    let server = behind_proxy(1, Duration::from_millis(300));
    let addr = server.local_addr();
    let (mut source, head) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    assert_eq!(head.status, 200);
    source.write_all(&mp3_frames(4)).unwrap();
    // More idle heads than the pool holds: the oldest are cut off to make room and the
    // rest time out, none of them answered.
    for mut stream in idle_clients(addr, 40) {
        assert!(read_to_end_within(&mut stream, PATIENCE).is_empty());
    }

    // The idle heads time out, and a real listener gets its place.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let mut listener = client(addr);
        let _ = listener.write_all(b"GET /morning/stream HTTP/1.0\r\n\r\n");
        let mut status_line = [0; 12];
        if listener.read_exact(&mut status_line).is_ok() && &status_line == b"HTTP/1.0 200" {
            break;
        }
        assert!(Instant::now() < deadline, "the listener was never served");
        source.write_all(&mp3_frames(4)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_source_still_gets_in_when_the_public_connections_are_full() {
    // A cap of 2 listeners: 34 public connection threads and 2 for the source.
    let server = behind_proxy(2, Duration::from_secs(600));
    let addr = server.local_addr();
    // A listener needs a live source; this one comes back later.
    let (mut first, head) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    assert_eq!(head.status, 200);
    first.write_all(&mp3_frames(4)).unwrap();
    let (_listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    // The source, the listener and 32 idle heads fill the public part.
    let _idle = idle_clients(addr, 32);

    // The station has room for a second listener, but the server has none.
    let (mut refused, head) = get(addr, "morning");
    assert_eq!(head.status, 503);
    assert_eq!(
        read_to_end_within(&mut refused, PATIENCE),
        b"the server has as many connections as it allows; try again later"
    );
    assert_eq!(status_of(request(addr, "OPTIONS", "/morning/stream")), 503);
    assert_eq!(status_of(request(addr, "GET", "/status-json.xsl")), 503);
    // A push without the key is refused as ever.
    let mut keyless = send_push(addr, "morning", None, "audio/mpeg", "");
    assert_eq!(read_head(&mut keyless).status, 401);
    drop(keyless);

    // The source that drops and comes back gets in, by a place kept for it: one more idle
    // head takes the public place it leaves. (Refusals that linger hold no public place.)
    drop(first);
    let _one_more = idle_clients(addr, 1);
    let mut back = push_when_free(addr, "morning", MORNING_KEY, "audio/mpeg");
    back.write_all(&mp3_frames(4)).unwrap();
    // And still nobody else does.
    assert_eq!(status_of(get(addr, "morning")), 503);
}

#[test]
fn a_source_still_gets_in_when_idle_heads_fill_every_connection() {
    // Through the trusted proxy, so no limit per address applies. "morning" takes 2
    // listeners and "evening" none: 34 public connection threads, and 2 kept for each
    // station's source.
    let config = Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\
         trusted_proxy = \"127.0.0.1\"\n\n\
         [[station]]\nslug = \"morning\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\nmax_stream_listeners = 2\n\n\
         [[station]]\nslug = \"evening\"\nname = \"Evening Station\"\nkey_hash = \"{}\"\nmax_stream_listeners = 0\n",
        hash_key(MORNING_KEY),
        hash_key(EVENING_KEY),
    ))
    .unwrap();
    let timeouts = Timeouts {
        head: Duration::from_secs(600),
        ..Timeouts::default()
    };
    let server = Server::start_with(config, timeouts).unwrap();
    let addr = server.local_addr();
    let (mut morning, head) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    assert_eq!(head.status, 200);
    morning.write_all(&mp3_frames(4)).unwrap();
    let (_listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    // 32 idle heads fill the public part, and 4 more held open fill the sources' places:
    // every connection thread is taken, and no head in it shows a key.
    let mut idle = idle_clients(addr, 36);

    // A source with its key still gets in, in place of the oldest idle head.
    let (mut evening, head) = push(addr, "evening", EVENING_KEY, "audio/mpeg");
    assert_eq!(head.status, 200);
    evening.write_all(&mp3_frames(4)).unwrap();
    assert!(read_to_end_within(&mut idle[0], PATIENCE).is_empty());
    // Anyone else who comes is refused, each in place of another idle head, and a source
    // whose head is read is never cut off.
    assert_eq!(status_of(get(addr, "morning")), 503);
    for _ in 0..4 {
        assert_eq!(status_of(request(addr, "OPTIONS", "/morning/stream")), 503);
    }
    evening.set_nonblocking(true).unwrap();
    let mut byte = [0];
    assert_eq!(
        evening.read(&mut byte).map_err(|e| e.kind()).err(),
        Some(ErrorKind::WouldBlock),
        "the live source was cut off"
    );
}

#[test]
fn refusals_that_linger_make_room_for_a_source_and_a_listener() {
    // A cap of 1 listener: 33 public connection threads and 2 for the source. Refusals
    // linger for the whole test unless the server cuts them off.
    let server = behind_proxy_with(
        1,
        Timeouts {
            head: Duration::from_secs(600),
            linger: Duration::from_secs(600),
            ..Timeouts::default()
        },
    );
    let addr = server.local_addr();
    // Whole requests, answered and then held open without a byte more: every connection
    // thread is taken by a refusal that lingers, and no head is still arriving.
    let lingering: Vec<TcpStream> = (0..35)
        .map(|_| {
            let (stream, head) = get(addr, "noon");
            assert!(matches!(head.status, 404 | 503), "{}", head.status);
            stream
        })
        .collect();

    // The source with its key still gets in, and is answered.
    let mut source = send_push(addr, "morning", Some(MORNING_KEY), "audio/mpeg", "");
    assert_eq!(read_head_bytes(&mut source), b"HTTP/1.0 200 OK\r\n\r\n");
    source.write_all(&mp3_frames(4)).unwrap();
    // An answered refusal holds no public place, so a listener gets in too.
    let (_listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    drop(lingering);
}

/// Ogg pages of about 60 KB each, the largest a page holds, after the stream's header
/// pages: big chunks, so a slow reader takes long over each.
fn big_ogg_pages(count: usize) -> Vec<u8> {
    let sink = crate::framing::Sink::default();
    let mut muxer = radio_stream::OggMuxer::new(sink.clone());
    muxer.start("Cloud Signal", None, 312).unwrap();
    for n in 0..count {
        muxer.packet(vec![(n % 251) as u8; 60_000], false).unwrap();
    }
    sink.take()
}

#[test]
fn a_listener_that_falls_behind_frees_its_connection_thread() {
    // A cap of 2 listeners: 34 public connection threads and 2 for the source.
    let server = behind_proxy_with(
        2,
        Timeouts {
            head: Duration::from_secs(600),
            listener_write: Duration::from_millis(500),
            ..Timeouts::default()
        },
    );
    let addr = server.local_addr();
    let (mut source, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    source.write_all(&big_ogg_pages(4)).unwrap();

    // A listener that keeps reading, but more slowly than the source sends: every read
    // comes well inside the write timeout, so only falling behind can end it.
    let (mut slow, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut buf = vec![0; 32 << 10];
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if matches!(slow.read(&mut buf), Ok(0) | Err(_)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };
    // The source, the slow listener and 32 idle heads fill the public part.
    let _idle = idle_clients(addr, 32);
    assert_eq!(status_of(get(addr, "morning")), 503);

    // The source sends far faster than the listener reads. Once its queue is full the
    // listener is dropped, and its thread ends: a new listener gets its place.
    let pages = big_ogg_pages(8);
    let deadline = Instant::now() + PATIENCE;
    loop {
        source.write_all(&pages).unwrap();
        let (_listener, head) = get(addr, "morning");
        if head.status == 200 {
            break;
        }
        assert_eq!(head.status, 503);
        assert!(
            Instant::now() < deadline,
            "the slow listener still holds its connection thread"
        );
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    reader.join().unwrap();
}

#[test]
fn heads_still_coming_are_limited_per_address() {
    let server = start_with(Timeouts {
        head: Duration::from_secs(600),
        ..Timeouts::default()
    });
    let addr = server.local_addr();
    let mut idle = idle_clients(addr, 8);
    assert!(
        options_reply(addr).is_empty(),
        "a ninth head from one address got a reply"
    );
    // A head that closes frees its place.
    drop(idle.pop());
    wait_for_options_reply(addr, b"HTTP/1.0 204", "the freed head was never taken");
    // A connection whose head is read no longer counts, though it stays open.
    let (_source, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    assert!(
        options_reply(addr).starts_with(b"HTTP/1.0 204"),
        "a finished head still counted against its address"
    );
    let _eighth = idle_clients(addr, 1);
    assert!(
        options_reply(addr).is_empty(),
        "a ninth head from one address got a reply"
    );
}

#[test]
fn stats_list_live_mounts_with_their_titles() {
    let server = start();
    let addr = server.local_addr();
    assert!(sources(&stats(addr)).is_empty(), "no station is live yet");

    // An Ogg source's title and artist come from its tags.
    let (mut ogg_source, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    let mut ogg = OggSource::new("Cloud Signal", Some("Echo"));
    ogg.audio(200);
    ogg_source.write_all(&ogg.take()).unwrap();
    let morning = wait_for_entry(addr, "morning", |entry| {
        entry.is_some_and(|entry| entry["title"] == "Cloud Signal")
    })
    .unwrap();
    assert_eq!(morning["artist"], "Echo");
    assert_eq!(
        morning["listenurl"],
        "https://radio.example.test/morning/stream"
    );
    assert_eq!(morning["server_name"], "Cloud Signal FM");
    assert_eq!(morning["server_type"], "audio/ogg");
    assert_eq!(morning["listeners"], 0);
    // The source's Ice-Bitrate.
    assert_eq!(morning["bitrate"], 96);
    let mut keys: Vec<&str> = morning
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "artist",
            "bitrate",
            "listeners",
            "listenurl",
            "server_name",
            "server_type",
            "title"
        ],
        "the public stats show nothing else"
    );
    assert!(
        source_entry(&stats(addr), "evening").is_none(),
        "a station with no source is absent"
    );

    // An MP3 source's title comes from its title update; with no Ice-Name the station's
    // own name stands in.
    let mut mp3_source = client(addr);
    write!(
        mp3_source,
        "PUT /evening/stream HTTP/1.0\r\n{}Content-Type: audio/mpeg\r\n\r\n",
        basic(EVENING_KEY)
    )
    .unwrap();
    assert_eq!(read_head(&mut mp3_source).status, 200);
    let evening = source_entry(&stats(addr), "evening").expect("a live station is listed");
    assert_eq!(evening["title"], "");
    assert_eq!(
        status_of(set_title(
            addr,
            "evening",
            "Paper Lanterns",
            Some(EVENING_KEY)
        )),
        200
    );
    let evening = source_entry(&stats(addr), "evening").unwrap();
    assert_eq!(evening["title"], "Paper Lanterns");
    assert_eq!(evening["artist"], "");
    assert_eq!(evening["server_name"], "Evening Station");
    assert_eq!(evening["server_type"], "audio/mpeg");
    assert!(
        evening.get("bitrate").is_none(),
        "a source that gives no bitrate is listed without one"
    );
    assert_eq!(sources(&stats(addr)).len(), 2);

    let (_listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    let before = morning["listeners"].as_u64().unwrap();
    let after = source_entry(&stats(addr), "morning").unwrap()["listeners"]
        .as_u64()
        .unwrap();
    assert!(after > before, "a listener who joined is counted");

    // A source that leaves takes its entry with it.
    drop(mp3_source);
    wait_for_entry(addr, "evening", |entry| entry.is_none());
}

#[test]
fn a_title_with_quotes_and_control_characters_keeps_the_json_valid() {
    let server = start();
    let addr = server.local_addr();
    let (_source, _) = push(addr, "morning", MORNING_KEY, "audio/mpeg");

    let title = "He said \"hi\"\u{1}\n";
    assert_eq!(
        status_of(set_title(addr, "morning", title, Some(MORNING_KEY))),
        200
    );
    assert_eq!(
        source_entry(&stats(addr), "morning").unwrap()["title"],
        title
    );

    // Bytes that are not UTF-8 still give a title, and the stats still parse.
    let query = "mount=%2Fmorning%2Fstream&mode=updinfo&song=%FF%FE";
    assert_eq!(
        status_of(metadata_query(addr, query, Some(MORNING_KEY))),
        200
    );
    let entry = source_entry(&stats(addr), "morning").unwrap();
    let replaced = entry["title"].as_str().expect("the title is a string");
    assert_ne!(replaced, title, "the update was taken");

    // So does the document built by hand, and a public URL's trailing slash is not doubled.
    let mounts = [StatsEntry {
        path: "morning/stream".to_owned(),
        stats: MountStats {
            live: true,
            format: Some(Format::Ogg),
            listeners: 3,
            name: "Morning \"Station\"\u{7f}".to_owned(),
            title: title.to_owned(),
            artist: "Echo\\".to_owned(),
            kbps: None,
        },
    }];
    let built: Value =
        serde_json::from_str(&status_json("https://radio.example.test/", &mounts)).unwrap();
    let entry = &sources(&built)[0];
    assert_eq!(
        entry["listenurl"],
        "https://radio.example.test/morning/stream"
    );
    assert_eq!(entry["title"], title);
    assert_eq!(entry["server_name"], "Morning \"Station\"\u{7f}");
    assert_eq!(entry["artist"], "Echo\\");
}

#[test]
fn a_title_is_cut_to_512_characters() {
    let server = start();
    let addr = server.local_addr();
    let (_source, _) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    // Two-byte characters, so a cut counted in bytes shows.
    let long: String = "Wölkenlied ".repeat(100);
    assert_eq!(
        status_of(set_title(addr, "morning", &long, Some(MORNING_KEY))),
        200
    );
    let entry = source_entry(&stats(addr), "morning").unwrap();
    let title = entry["title"].as_str().unwrap();
    // The cap is the spec's value.
    assert_eq!(title.chars().count(), 512);
    assert!(long.starts_with(title), "the title is cut, not changed");

    // An Ogg source's tags are cut the same way, title and artist both.
    let (mut ogg_source, head) = push(addr, "evening", EVENING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    let long_artist: String = "Échotal ".repeat(100);
    let mut ogg = OggSource::new(&long, Some(&long_artist));
    ogg.audio(200);
    ogg_source.write_all(&ogg.take()).unwrap();
    let entry = wait_for_entry(addr, "evening", |entry| {
        entry.is_some_and(|entry| !entry["title"].as_str().unwrap_or_default().is_empty())
    })
    .unwrap();
    let title = entry["title"].as_str().unwrap();
    let artist = entry["artist"].as_str().unwrap();
    assert_eq!(title.chars().count(), 512);
    assert!(long.starts_with(title), "the tag title is cut, not changed");
    assert_eq!(artist.chars().count(), 512);
    assert!(
        long_artist.starts_with(artist),
        "the tag artist is cut, not changed"
    );
}

#[test]
fn a_source_name_is_cut_to_512_characters() {
    let server = start();
    let addr = server.local_addr();
    // Two-byte characters, so a cut counted in bytes shows.
    let long: String = "Wölkenlied ".repeat(60);
    let mut source = client(addr);
    write!(
        source,
        "PUT /morning/stream HTTP/1.0\r\n{}Content-Type: audio/mpeg\r\nIce-Name: {long}\r\n\r\n",
        basic(MORNING_KEY)
    )
    .unwrap();
    assert_eq!(read_head(&mut source).status, 200);
    let entry = source_entry(&stats(addr), "morning").unwrap();
    let name = entry["server_name"].as_str().unwrap();
    assert_eq!(name.chars().count(), 512);
    assert!(long.starts_with(name), "the name is cut, not changed");
}

#[test]
fn stats_built_for_a_mount_with_no_source_leave_it_out() {
    let idle = MountStats {
        live: false,
        format: None,
        listeners: 0,
        name: "Evening Station".to_owned(),
        title: String::new(),
        artist: String::new(),
        kbps: None,
    };
    let built: Value = serde_json::from_str(&status_json(
        "https://radio.example.test",
        &[StatsEntry {
            path: "evening/stream".to_owned(),
            stats: idle,
        }],
    ))
    .unwrap();
    assert!(sources(&built).is_empty());
}

/// Set in the child process [`a_connection_holds_one_file_descriptor`] runs itself in.
#[cfg(unix)]
const FD_CHILD: &str = "RADIO_SERVER_TEST_FD_CHILD";

/// The process's open file descriptors.
#[cfg(unix)]
fn open_descriptors() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

#[cfg(unix)]
#[test]
fn a_connection_holds_one_file_descriptor() {
    // Other tests open sockets while this one counts, so it counts in a process of its own.
    if std::env::var_os(FD_CHILD).is_none() {
        let name = "server::a_connection_holds_one_file_descriptor";
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--test-threads=1"])
            .env(FD_CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the child ran no test: {stdout}"
        );
        return;
    }
    let server = start();
    let addr = server.local_addr();
    let (mut source, head) = push(addr, "morning", MORNING_KEY, "audio/mpeg");
    assert_eq!(head.status, 200);
    source.write_all(&mp3_frames(4)).unwrap();
    let before = open_descriptors();

    const LISTENERS: usize = 40;
    let listeners: Vec<TcpStream> = (0..LISTENERS)
        .map(|_| {
            let (stream, head) = get(addr, "morning");
            assert_eq!(head.status, 200);
            stream
        })
        .collect();
    wait_for_entry(addr, "morning", |entry| {
        entry.is_some_and(|entry| entry["listeners"] == LISTENERS)
    });
    // The test's own client sockets are in this process too: one each.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let server_side = open_descriptors().saturating_sub(before + listeners.len());
        // One descriptor each, with room for a stats request still closing; two each
        // would be twice as many.
        if server_side <= LISTENERS + LISTENERS / 4 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{server_side} descriptors for {LISTENERS} listeners"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn with_tls(cert: &std::path::Path, key: &std::path::Path) -> Config {
    let mut config = config(200, None);
    config.tls_cert = Some(cert.to_owned());
    config.tls_key = Some(key.to_owned());
    config
}

type TlsClient = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// A TLS client that trusts only `root`.
fn tls_client(addr: SocketAddr, root: &rustls::pki_types::CertificateDer<'static>) -> TlsClient {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
    let connection = rustls::ClientConnection::new(std::sync::Arc::new(config), name).unwrap();
    rustls::StreamOwned::new(connection, client(addr))
}

/// Reads a TLS stream until it ends, and whether it ended with `close_notify`; fails the
/// test after `limit`.
fn read_tls_to_end_within(stream: &mut TlsClient, limit: Duration) -> (Vec<u8>, bool) {
    let deadline = Instant::now() + limit;
    stream
        .sock
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let mut out = Vec::new();
    let mut buf = [0; 4096];
    loop {
        assert!(
            Instant::now() < deadline,
            "the server did not close within {limit:?}"
        );
        match stream.read(&mut buf) {
            Ok(0) => return (out, true),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return (out, false),
        }
    }
}

#[test]
fn with_certificate_files_the_server_speaks_tls() {
    let files = tls_files();
    let server = Server::start(with_tls(&files.cert, &files.key)).unwrap();
    let addr = server.local_addr();

    let mut source = tls_client(addr, &files.root);
    source
        .write_all(push_head("morning", Some(MORNING_KEY), "audio/ogg", "").as_bytes())
        .unwrap();
    assert_eq!(read_head_bytes(&mut source), b"HTTP/1.0 200 OK\r\n\r\n");
    let mut ogg = OggSource::new("Cloud Signal", Some("Echo"));
    ogg.audio(200);
    let before = ogg.take();
    source.write_all(&before).unwrap();
    source.flush().unwrap();
    let last_before = granule(pages(&before).last().unwrap());

    let mut listener = tls_client(addr, &files.root);
    listener
        .write_all(b"GET /morning/stream HTTP/1.0\r\nHost: radio.example.test\r\n\r\n")
        .unwrap();
    let head = read_head(&mut listener);
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Type"), Some("audio/ogg"));
    ogg.audio(400);
    for part in ogg.take().chunks(700) {
        source.write_all(part).unwrap();
    }
    source.flush().unwrap();
    let deadline = Instant::now() + PATIENCE;
    let mut heard = Vec::new();
    let mut buf = [0; 4096];
    while !pages(&heard).iter().any(|page| granule(page) > last_before) {
        assert!(Instant::now() < deadline, "no page arrived over TLS");
        let n = listener.read(&mut buf).unwrap();
        assert!(n > 0, "the stream ended early");
        heard.extend_from_slice(&buf[..n]);
    }
    assert!(heard.starts_with(b"OggS"));

    // Plain HTTP on the TLS port gets no HTTP reply.
    let mut plain = client(addr);
    let _ = plain.write_all(b"GET /status-json.xsl HTTP/1.0\r\n\r\n");
    let reply = read_to_end_within(&mut plain, PATIENCE);
    assert!(!reply.starts_with(b"HTTP/"), "plain HTTP was answered");

    // The source ends cleanly, and so, at once, does the listener's stream.
    source.conn.send_close_notify();
    source.flush().unwrap();
    let (_, clean) = read_tls_to_end_within(&mut listener, PATIENCE);
    assert!(clean, "the listener's stream ended without close_notify");
}

#[test]
fn stop_ends_tls_sources_and_listeners_at_once() {
    let files = tls_files();
    let server = Server::start(with_tls(&files.cert, &files.key)).unwrap();
    let addr = server.local_addr();
    let mut source = tls_client(addr, &files.root);
    source
        .write_all(push_head("morning", Some(MORNING_KEY), "audio/mpeg", "").as_bytes())
        .unwrap();
    assert_eq!(read_head(&mut source).status, 200);
    source.write_all(&mp3_frames(10)).unwrap();
    source.flush().unwrap();
    let mut listener = tls_client(addr, &files.root);
    listener
        .write_all(b"GET /morning/stream HTTP/1.0\r\n\r\n")
        .unwrap();
    assert_eq!(read_head(&mut listener).status, 200);

    let started = Instant::now();
    server.stop();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stop took too long"
    );
    read_tls_to_end_within(&mut listener, Duration::from_secs(2));
    read_tls_to_end_within(&mut source, Duration::from_secs(2));
}

#[test]
fn a_tls_handshake_that_never_finishes_is_closed_at_the_deadline() {
    let files = tls_files();
    let server = Server::start_with(
        with_tls(&files.cert, &files.key),
        Timeouts {
            head: Duration::from_millis(300),
            ..Timeouts::default()
        },
    )
    .unwrap();
    let mut slow = client(server.local_addr());
    // The start of a TLS record that never completes, one byte at a time, each well
    // inside the head timeout.
    let mut writer = slow.try_clone().unwrap();
    let dribble = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(3);
        for byte in [22, 3, 3, 0, 200].into_iter().chain(std::iter::repeat(1)) {
            if Instant::now() > until || writer.write_all(&[byte]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let started = Instant::now();
    read_to_end_within(&mut slow, Duration::from_secs(2));
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(slow);
    dribble.join().unwrap();
}

#[test]
fn a_missing_certificate_file_is_a_start_error() {
    let files = tls_files();
    let missing = files.dir.path().join("missing.pem");
    let error = Server::start(with_tls(&missing, &files.key))
        .err()
        .expect("a missing certificate does not start");
    let ServerError::Tls(message) = error else {
        panic!("not a TLS error: {error}");
    };
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );

    // A file with no certificate in it.
    let error = Server::start(with_tls(&files.key, &files.key))
        .err()
        .unwrap();
    assert!(
        matches!(&error, ServerError::Tls(message) if message.contains(&files.key.display().to_string())),
        "{error}"
    );

    // A key that does not parse: the error names the file and quotes none of it.
    let garbage = files.dir.path().join("garbage.pem");
    let body = "aW52ZW50ZWQtbm90LWEta2V5LWF0LWFsbC0xMjM0NTY3ODkw";
    std::fs::write(
        &garbage,
        format!("-----BEGIN PRIVATE KEY-----\n{body}\n-----END PRIVATE KEY-----\n"),
    )
    .unwrap();
    let error = Server::start(with_tls(&files.cert, &garbage))
        .err()
        .unwrap();
    let ServerError::Tls(message) = &error else {
        panic!("not a TLS error: {error}");
    };
    assert!(
        message.contains(&garbage.display().to_string()),
        "{message}"
    );
    assert!(!message.contains(body), "{message}");
    let bad_start = files.dir.path().join("bad-start.pem");
    std::fs::write(&bad_start, format!("-----BEGIN {body}\n")).unwrap();
    let error = Server::start(with_tls(&files.cert, &bad_start))
        .err()
        .unwrap();
    assert!(!error.to_string().contains(body), "{error}");
}

/// How long a listener of an output may take to hear the tone.
pub(crate) const EAR_PATIENCE: Duration = Duration::from_secs(20);

/// A server whose station "morning" has an Opus output at `/morning/opus` (64 kbit/s) and
/// an MP3 one at `/morning/mp3` (128 kbit/s), and whose station "evening" has none.
pub(crate) fn with_outputs(max_stream_listeners: usize) -> ServerHandle {
    let config = Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
         [[station]]\nslug = \"morning\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\nmax_stream_listeners = {max_stream_listeners}\n\n\
         [[station.output]]\nmount = \"opus\"\nformat = \"opus\"\nkbps = 64\n\n\
         [[station.output]]\nmount = \"mp3\"\nformat = \"mp3\"\nkbps = 128\n\n\
         [[station]]\nslug = \"evening\"\nname = \"Evening Station\"\nkey_hash = \"{}\"\n",
        hash_key(MORNING_KEY),
        hash_key(EVENING_KEY),
    ))
    .unwrap();
    Server::start(config).unwrap()
}

/// What a [`Pusher`] sends.
#[derive(Clone, Copy)]
pub(crate) enum Tone {
    /// Ogg/Opus, as the app sends it, titled "Cloud Signal" by "Echo".
    Ogg,
    /// MP3 at this sample rate, stereo.
    Mp3 { rate: u32 },
}

/// Sends a tone down a source connection whose push was taken, 20 ms at a time in real
/// time, from a thread, keeping a copy of every byte sent. Dropping it closes the
/// connection, which ends the source.
pub(crate) struct Pusher {
    stop: Arc<AtomicBool>,
    packets: Arc<AtomicUsize>,
    sent: Arc<Mutex<Vec<u8>>>,
    thread: Option<JoinHandle<()>>,
}

impl Pusher {
    pub(crate) fn start(mut source: TcpStream, tone: Tone) -> Pusher {
        let stop = Arc::new(AtomicBool::new(false));
        let packets = Arc::new(AtomicUsize::new(0));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let (stop, packets, sent) =
                (Arc::clone(&stop), Arc::clone(&packets), Arc::clone(&sent));
            std::thread::spawn(move || {
                let mut ogg = OggSource::new("Cloud Signal", Some("Echo"));
                let mut mp3 = match tone {
                    Tone::Mp3 { rate } => {
                        Some((Encoder::new(Mp3Config::cbr(rate, 2, 128)).unwrap(), rate))
                    }
                    Tone::Ogg => None,
                };
                let started = Instant::now();
                let mut frames = 0usize;
                let mut ticks = 0u32;
                while !stop.load(Ordering::Acquire) {
                    let bytes = match &mut mp3 {
                        None => {
                            ogg.audio(20);
                            ogg.take()
                        }
                        Some((encoder, rate)) => {
                            let count = *rate as usize / 50;
                            let tone: Vec<f32> = (frames..frames + count)
                                .map(|i| {
                                    (i as f32 / *rate as f32 * 440.0 * std::f32::consts::TAU).sin()
                                        * 0.2
                                })
                                .collect();
                            frames += count;
                            encoder.encode(&[&tone, &tone])
                        }
                    };
                    // Recorded first, so a listener can never hear a byte not in the copy.
                    sent.lock().unwrap().extend_from_slice(&bytes);
                    if source.write_all(&bytes).is_err() {
                        break;
                    }
                    packets.fetch_add(1, Ordering::AcqRel);
                    ticks += 1;
                    let next = started + Duration::from_millis(20) * ticks;
                    std::thread::sleep(next.saturating_duration_since(Instant::now()));
                }
            })
        };
        Pusher {
            stop,
            packets,
            sent,
            thread: Some(thread),
        }
    }

    /// Waits until `more` packets more have been sent.
    pub(crate) fn wait_packets(&self, more: usize) {
        let target = self.packets.load(Ordering::Acquire) + more;
        let deadline = Instant::now() + PATIENCE + Duration::from_millis(20 * more as u64);
        while self.packets.load(Ordering::Acquire) < target {
            assert!(Instant::now() < deadline, "the pusher fell behind");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Every byte sent so far.
    fn sent(&self) -> Vec<u8> {
        self.sent.lock().unwrap().clone()
    }
}

impl Drop for Pusher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Pushes `tone` to `slug` and keeps pushing it.
pub(crate) fn on_air(addr: SocketAddr, slug: &str, key: &str, tone: Tone) -> Pusher {
    let content_type = match tone {
        Tone::Ogg => "audio/ogg",
        Tone::Mp3 { .. } => "audio/mpeg",
    };
    let (source, head) = push(addr, slug, key, content_type);
    assert_eq!(head.status, 200);
    let pusher = Pusher::start(source, tone);
    pusher.wait_packets(5);
    pusher
}

/// Reads and throws away what a listener is sent, from a thread, until dropped; counts the
/// bytes.
pub(crate) struct Drain {
    bytes: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drain {
    pub(crate) fn start(mut stream: TcpStream) -> Drain {
        let bytes = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (bytes, stop) = (Arc::clone(&bytes), Arc::clone(&stop));
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
                let mut buf = [0; 4096];
                while !stop.load(Ordering::Acquire) {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            bytes.fetch_add(n, Ordering::AcqRel);
                        }
                        Err(e)
                            if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                        Err(_) => break,
                    }
                }
            })
        };
        Drain {
            bytes,
            stop,
            thread: Some(thread),
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Acquire)
    }

    /// Whether the server ended the connection.
    pub(crate) fn ended(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for Drain {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Polls the stats until they list `count` mounts, and returns them.
fn wait_for_sources(addr: SocketAddr, count: usize) -> Vec<Value> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let listed = sources(&stats(addr)).clone();
        if listed.len() == count {
            return listed;
        }
        assert!(
            Instant::now() < deadline,
            "the stats never listed {count} mounts: {listed:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn get_output(addr: SocketAddr, slug: &str, mount: &str) -> (TcpStream, Head) {
    request(addr, "GET", &format!("/{slug}/{mount}"))
}

#[test]
fn an_output_is_served_at_its_mount() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    for (mount, content_type, format) in [
        ("opus", "audio/ogg", Format::Ogg),
        ("mp3", "audio/mpeg", Format::Mp3),
    ] {
        let (stream, head) = get_output(addr, "morning", mount);
        assert_eq!(head.status, 200, "/morning/{mount}");
        assert_eq!(head.header("Content-Type"), Some(content_type));
        assert_eq!(head.header("Access-Control-Allow-Origin"), Some("*"));
        assert!(
            hears(stream, format, EAR_PATIENCE),
            "the {mount} output plays the tone"
        );
    }
}

#[test]
fn an_output_of_a_station_off_air_is_404() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    assert_eq!(status_of(get_output(addr, "morning", "opus")), 404);
    // A mount no station has is 404 on the air or off it, as is an output of another
    // station.
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    let _evening = on_air(addr, "evening", EVENING_KEY, Tone::Ogg);
    assert_eq!(status_of(get_output(addr, "morning", "low")), 404);
    assert_eq!(status_of(get_output(addr, "evening", "opus")), 404);
}

#[test]
fn the_pushed_mount_is_served_as_pushed() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    let (opus, head) = get_output(addr, "morning", "opus");
    assert_eq!(head.status, 200);
    let opus = Drain::start(opus);

    let (mut listener, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    let deadline = Instant::now() + PATIENCE;
    let mut heard = Vec::new();
    let mut buf = [0; 4096];
    while pages(&heard).len() < 100 {
        assert!(Instant::now() < deadline, "the stream stalled");
        let n = listener.read(&mut buf).unwrap();
        assert!(n > 0, "the stream ended early");
        heard.extend_from_slice(&buf[..n]);
    }
    let sent = pusher.sent();
    for page in pages(&heard) {
        assert!(
            sent.windows(page.len()).any(|window| window == page),
            "a /stream listener got a page the source never sent"
        );
    }
    assert!(opus.bytes() > 0, "the output was transcoded meanwhile");
}

#[test]
fn the_stats_list_the_pushed_mount_first_then_the_outputs() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Mp3 { rate: 48_000 });
    // The outputs are listed once they have begun.
    wait_for_sources(addr, 3);
    let (stream, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    let _pushed = Drain::start(stream);
    let (stream, head) = get_output(addr, "morning", "opus");
    assert_eq!(head.status, 200);
    let _opus = Drain::start(stream);
    assert_eq!(
        status_of(set_title(
            addr,
            "morning",
            "Paper Lanterns",
            Some(MORNING_KEY)
        )),
        200
    );

    let listed = wait_for_sources(addr, 3);
    let urls: Vec<&str> = listed
        .iter()
        .map(|source| source["listenurl"].as_str().unwrap())
        .collect();
    assert_eq!(
        urls,
        [
            "https://radio.example.test/morning/stream",
            "https://radio.example.test/morning/opus",
            "https://radio.example.test/morning/mp3",
        ]
    );
    // The bitrates: the source's Ice-Bitrate, then each output's own.
    let bitrates: Vec<u64> = listed
        .iter()
        .map(|source| source["bitrate"].as_u64().expect("a bitrate"))
        .collect();
    assert_eq!(bitrates, [96, 64, 128]);
    let listeners: Vec<u64> = listed
        .iter()
        .map(|source| source["listeners"].as_u64().unwrap())
        .collect();
    assert_eq!(listeners, [1, 1, 0]);
    let types: Vec<&str> = listed
        .iter()
        .map(|source| source["server_type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["audio/mpeg", "audio/ogg", "audio/mpeg"]);
    for source in &listed {
        assert_eq!(source["title"], "Paper Lanterns", "{source}");
        assert_eq!(source["server_name"], "Cloud Signal FM", "{source}");
    }
    // The listener page still finds the pushed mount.
    assert_eq!(
        source_entry(&stats(addr), "morning").unwrap()["listenurl"],
        "https://radio.example.test/morning/stream"
    );
    assert_eq!(
        entry(&stats(addr), "morning", "mp3").unwrap()["bitrate"],
        128
    );
}

#[test]
fn a_station_s_limit_covers_its_outputs() {
    let server = with_outputs(2);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    wait_for_sources(addr, 3);
    let (opus, head) = get_output(addr, "morning", "opus");
    assert_eq!(head.status, 200);
    let _opus = Drain::start(opus);
    let (pushed, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    let _pushed = Drain::start(pushed);

    for mount in ["stream", "opus", "mp3"] {
        let (mut refused, head) = get_output(addr, "morning", mount);
        assert_eq!(head.status, 503, "a third listener on /morning/{mount}");
        let body = read_to_end_within(&mut refused, PATIENCE);
        assert!(
            String::from_utf8_lossy(&body).contains("as many listeners"),
            "{}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[test]
fn outputs_of_a_44_1_khz_source_say_why_they_are_off() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Mp3 { rate: 44_100 });
    for mount in ["opus", "mp3"] {
        let (mut refused, head) = get_output(addr, "morning", mount);
        assert_eq!(head.status, 503, "/morning/{mount}");
        let body = String::from_utf8(read_to_end_within(&mut refused, PATIENCE)).unwrap();
        assert!(body.contains("44100"), "{body}");
        assert!(body.contains("48000"), "{body}");
        assert!(!body.to_lowercase().contains("ok"), "{body}");
    }
    let listed = sources(&stats(addr)).clone();
    assert_eq!(
        listed.len(),
        1,
        "only the pushed mount is listed: {listed:?}"
    );
    assert_eq!(
        listed[0]["listenurl"],
        "https://radio.example.test/morning/stream"
    );
    // The pushed stream is still served.
    assert_eq!(status_of(get(addr, "morning")), 200);
}

#[test]
fn an_output_that_has_not_begun_says_so() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    // A source that sends nothing: its outputs never begin.
    let (_source, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
    let started = Instant::now();
    let (mut waiting, head) = get_output(addr, "morning", "opus");
    assert_eq!(head.status, 503);
    let body = String::from_utf8(read_to_end_within(&mut waiting, PATIENCE)).unwrap();
    assert!(body.contains("starting"), "{body}");
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "the output was given time to begin"
    );
    let listed = sources(&stats(addr)).clone();
    assert_eq!(
        listed.len(),
        1,
        "only the pushed mount is listed: {listed:?}"
    );
}

/// Sends a keyed request with `method` to `path`, with `key`, and gives its status.
fn keyed(addr: SocketAddr, method: &str, path: &str, key: &str) -> u16 {
    let mut stream = client(addr);
    write!(
        stream,
        "{method} {path} HTTP/1.0\r\nHost: radio.example.test\r\n{}Content-Type: audio/ogg\r\n\r\n",
        basic(key)
    )
    .unwrap();
    read_head(&mut stream).status
}

#[test]
fn a_push_to_an_output_path_is_not_a_keyed_request() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    for _ in 0..6 {
        assert_eq!(
            keyed(addr, "PUT", "/morning/opus", "invented-wrong-key"),
            404
        );
        assert_eq!(
            keyed(addr, "SOURCE", "/morning/mp3", "invented-wrong-key"),
            404
        );
        let query = "mount=%2Fmorning%2Fopus&mode=updinfo&song=Echo";
        assert_eq!(
            status_of(metadata_query(addr, query, Some("invented-wrong-key"))),
            404
        );
    }
    // Not even the right key makes an output path a source route.
    assert_eq!(keyed(addr, "PUT", "/morning/opus", MORNING_KEY), 404);
    assert_eq!(
        status_of(push(addr, "morning", MORNING_KEY, "audio/ogg")),
        200
    );
}

#[test]
fn a_new_source_right_after_the_old_one_is_heard_on_the_outputs() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let old = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    let (stream, head) = get_output(addr, "morning", "mp3");
    assert_eq!(head.status, 200);
    assert!(hears(stream, Format::Mp3, EAR_PATIENCE));

    // The old source leaves and a new one, in another format, comes at once.
    drop(old);
    let source = push_when_free(addr, "morning", MORNING_KEY, "audio/mpeg");
    let _new = Pusher::start(source, Tone::Mp3 { rate: 48_000 });
    for (mount, format) in [("opus", Format::Ogg), ("mp3", Format::Mp3)] {
        let (stream, head) = get_output(addr, "morning", mount);
        assert_eq!(
            head.status, 200,
            "/morning/{mount} after the new source came"
        );
        assert!(
            hears(stream, format, EAR_PATIENCE),
            "the {mount} output plays the new source"
        );
    }
}

#[test]
fn stop_returns_with_outputs_being_listened_to() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    let mut listeners = Vec::new();
    for mount in ["opus", "mp3"] {
        let (mut stream, head) = get_output(addr, "morning", mount);
        assert_eq!(head.status, 200);
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("the output sends");
        listeners.push(stream);
    }

    let started = Instant::now();
    server.stop();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stop took too long"
    );
    for listener in &mut listeners {
        read_to_end_within(listener, Duration::from_secs(2));
    }
}

#[test]
fn an_output_that_failed_says_so_at_once_and_the_rest_play() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    // Something else holds the MP3 output's mount, so it can't begin for the next source.
    let held = server
        .outputs("morning")
        .unwrap()
        .mount("mp3")
        .unwrap()
        .begin(Format::Mp3, "Morning Station".into(), None)
        .unwrap();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    // Until the transcoder has decoded its first audio, the stats can list the held MP3
    // mount itself; wait until it has marked that output failed.
    let outputs = server.outputs("morning").unwrap();
    let deadline = Instant::now() + PATIENCE;
    while !outputs.failed("mp3") {
        assert!(
            Instant::now() < deadline,
            "the MP3 output was never marked failed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // From then on the MP3 mount is not listed, so two mounts are the pushed one and Opus.
    let listed = wait_for_sources(addr, 2);
    let urls: Vec<&str> = listed
        .iter()
        .map(|source| source["listenurl"].as_str().unwrap())
        .collect();
    assert_eq!(
        urls,
        [
            "https://radio.example.test/morning/stream",
            "https://radio.example.test/morning/opus",
        ],
        "the failed output is not listed"
    );

    let started = Instant::now();
    let (mut failed, head) = get_output(addr, "morning", "mp3");
    assert_eq!(head.status, 503);
    let body = String::from_utf8(read_to_end_within(&mut failed, PATIENCE)).unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "a failed output was answered after {:?}",
        started.elapsed()
    );
    assert!(body.contains("failed"), "{body}");
    assert!(body.contains("/morning/stream"), "{body}");
    assert!(!body.contains("starting"), "{body}");

    let (opus, head) = get_output(addr, "morning", "opus");
    assert_eq!(head.status, 200);
    assert!(
        hears(opus, Format::Ogg, EAR_PATIENCE),
        "the other output plays"
    );
    let (pushed, head) = get(addr, "morning");
    assert_eq!(head.status, 200);
    assert!(
        hears(pushed, Format::Ogg, EAR_PATIENCE),
        "the pushed stream plays"
    );
    drop(held);
}

#[test]
fn a_new_source_finds_the_old_outputs_ended() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let old = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    wait_for_sources(addr, 3);

    drop(old);
    // The new source is taken the moment the mount is free, and sends nothing, so no output
    // of its own begins.
    let deadline = Instant::now() + PATIENCE;
    let _new = loop {
        let (stream, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
        if head.status == 200 {
            break stream;
        }
        assert_eq!(head.status, 403, "only a busy mount refuses this source");
        assert!(Instant::now() < deadline, "the mount never became free");
        std::thread::sleep(Duration::from_millis(5));
    };
    let listed = sources(&stats(addr)).clone();
    assert_eq!(
        listed.len(),
        1,
        "the old source's outputs were still live when the new source came: {listed:?}"
    );
    assert_eq!(
        listed[0]["listenurl"],
        "https://radio.example.test/morning/stream"
    );
}

#[test]
fn an_ice_bitrate_that_is_not_a_positive_whole_number_is_not_listed() {
    let server = start();
    let addr = server.local_addr();
    for bitrate in ["0", "96.5", "fast"] {
        let mut source = client(addr);
        write!(
            source,
            "PUT /morning/stream HTTP/1.0\r\n{}Content-Type: audio/mpeg\r\nIce-Bitrate: {bitrate}\r\n\r\n",
            basic(MORNING_KEY)
        )
        .unwrap();
        assert_eq!(read_head(&mut source).status, 200);
        let entry = source_entry(&stats(addr), "morning").unwrap();
        assert!(
            entry.get("bitrate").is_none(),
            "Ice-Bitrate {bitrate}: {entry}"
        );
        drop(source);
        wait_for_entry(addr, "morning", |entry| entry.is_none());
    }
}
