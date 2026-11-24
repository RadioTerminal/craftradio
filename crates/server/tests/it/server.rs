use crate::framing::{OggSource, granule, pages};
use base64::Engine;
use radio_server::{Config, Server, ServerHandle, Timeouts, hash_key};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

const MORNING_KEY: &str = "invented-morning-key-4Hq8";
const EVENING_KEY: &str = "invented-evening-key-9Zt3";
/// How long a test waits for something that should happen at once.
const PATIENCE: Duration = Duration::from_secs(5);

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

/// A reply's status and headers.
struct Head {
    status: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn client(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    // A server that never answers fails the test instead of hanging it.
    stream.set_read_timeout(Some(PATIENCE)).unwrap();
    stream.set_write_timeout(Some(PATIENCE)).unwrap();
    stream
}

/// Reads a reply head one byte at a time, so no byte of the body is taken.
fn read_head(stream: &mut TcpStream) -> Head {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("a reply head");
        head.push(byte[0]);
    }
    let text = String::from_utf8(head).unwrap();
    let mut lines = text.trim_end().split("\r\n");
    let status_line = lines.next().unwrap();
    let status = status_line.split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.to_owned(), value.trim().to_owned())
        })
        .collect();
    Head { status, headers }
}

/// Starts a source push the way the app's `connect()` does and reads the reply head.
fn push_with(
    addr: SocketAddr,
    slug: &str,
    key: &str,
    content_type: &str,
    extra: &str,
) -> (TcpStream, Head) {
    let mut stream = client(addr);
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("source:{key}"));
    write!(
        stream,
        "PUT /{slug}/stream HTTP/1.0\r\nHost: radio.example.test\r\nAuthorization: Basic {auth}\r\n\
         Content-Type: {content_type}\r\nContent-Length: 9223372036854775807\r\n\
         Ice-Name: Cloud Signal FM\r\nIce-Description: Invented test stream\r\nIce-Genre: Ambient\r\n\
         Ice-Bitrate: 96\r\n{extra}\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    (stream, head)
}

fn push(addr: SocketAddr, slug: &str, key: &str, content_type: &str) -> (TcpStream, Head) {
    push_with(addr, slug, key, content_type, "")
}

/// Pushes until the mount takes the source: the server notices a closed source a moment
/// after the close.
fn push_when_free(addr: SocketAddr, slug: &str, key: &str, content_type: &str) -> TcpStream {
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

fn request(addr: SocketAddr, method: &str, path: &str) -> (TcpStream, Head) {
    let mut stream = client(addr);
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: radio.example.test\r\n\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    (stream, head)
}

fn get(addr: SocketAddr, slug: &str) -> (TcpStream, Head) {
    request(addr, "GET", &format!("/{slug}/stream"))
}

/// Reads until the server closes the connection, failing the test after `limit`.
fn read_to_end_within(stream: &mut TcpStream, limit: Duration) -> Vec<u8> {
    let deadline = Instant::now() + limit;
    // Fails on a connection the server already reset; the read below then returns at once.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut out = Vec::new();
    let mut buf = [0; 4096];
    loop {
        assert!(
            Instant::now() < deadline,
            "the server did not close within {limit:?}"
        );
        match stream.read(&mut buf) {
            Ok(0) => return out,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == ErrorKind::ConnectionReset => return out,
            Err(e) => panic!("reading: {e}"),
        }
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
    let challenge = head.header("WWW-Authenticate").unwrap_or_default();
    assert!(challenge.starts_with("Basic"), "{challenge}");

    let (_right, head) = push(addr, "morning", MORNING_KEY, "audio/ogg");
    assert_eq!(head.status, 200);
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

#[test]
fn connections_over_the_thread_cap_are_closed_at_once() {
    // One station that takes no listeners: 0 + 2 × 1 + 32 connection threads.
    let config = Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
         [[station]]\nslug = \"morning\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\nmax_stream_listeners = 0\n",
        hash_key(MORNING_KEY),
    ))
    .unwrap();
    let server = Server::start(config).unwrap();
    let addr = server.local_addr();
    // Idle clients that never finish a head each hold a thread.
    let mut idle: Vec<TcpStream> = (0..34).map(|_| client(addr)).collect();
    for stream in &mut idle {
        stream.write_all(b"GET /mor").unwrap();
    }
    // Connections are accepted, and given a thread, in the order they were made.

    let mut over = client(addr);
    let _ = over.write_all(b"OPTIONS /morning/stream HTTP/1.0\r\n\r\n");
    let reply = read_to_end_within(&mut over, Duration::from_secs(2));
    assert!(reply.is_empty(), "a connection over the cap got a reply");

    drop(idle.pop());
    let deadline = Instant::now() + PATIENCE;
    loop {
        let mut stream = client(addr);
        let _ = stream.write_all(b"OPTIONS /morning/stream HTTP/1.0\r\n\r\n");
        if read_to_end_within(&mut stream, PATIENCE).starts_with(b"HTTP/1.0 204") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the freed thread was never reused"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
