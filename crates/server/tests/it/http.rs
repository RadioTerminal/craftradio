use base64::Engine;
use radio_server::{HttpError, Request, client_addr, read_request, respond};
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::time::{Duration, Instant};

const KEY: &str = "invented-station-key-7Qp2";

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn basic(user: &str, password: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
}

fn parse(head: &str) -> Request {
    read_request(&mut Cursor::new(head.as_bytes().to_vec()), soon()).unwrap()
}

fn addr(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// The head `connect()` in `crates/stream/src/output.rs` writes, followed by the start of
/// the endless body.
fn source_head() -> String {
    format!(
        "PUT /morning/stream HTTP/1.0\r\nHost: radio.example.test\r\nAuthorization: Basic {}\r\nContent-Type: audio/ogg\r\nContent-Length: 9223372036854775807\r\nIce-Name: Morning Station\r\nIce-Description: Cloud Signal all morning\r\nIce-Genre: Ambient\r\nIce-Bitrate: 128\r\n\r\n",
        basic("source", KEY)
    )
}

#[test]
fn parses_the_apps_source_request() {
    let mut bytes = source_head().into_bytes();
    bytes.extend_from_slice(b"OggS\0body-follows");
    let mut reader = Cursor::new(bytes);

    let request = read_request(&mut reader, soon()).unwrap();

    assert_eq!(request.method, "PUT");
    assert_eq!(request.path, "/morning/stream");
    assert_eq!(request.basic_password().as_deref(), Some(KEY));
    assert_eq!(request.header("content-type"), Some("audio/ogg"));
    assert_eq!(request.header("ICE-NAME"), Some("Morning Station"));
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"OggS\0body-follows");
}

#[test]
fn decodes_the_metadata_query() {
    // The head `update_mp3_title` in `crates/stream/src/output.rs` writes.
    let head = format!(
        "GET /admin/metadata?mount=%2Fmorning%2Fstream&mode=updinfo&song=Cloud%20Signal%20%E2%80%93%20Echo HTTP/1.0\r\nHost: radio.example.test\r\nAuthorization: Basic {}\r\nConnection: close\r\n\r\n",
        basic("source", KEY)
    );
    let request = parse(&head);

    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/admin/metadata");
    assert_eq!(request.param("song"), Some("Cloud Signal – Echo"));
    assert_eq!(request.param("mount"), Some("/morning/stream"));
    assert_eq!(request.param("mode"), Some("updinfo"));
    assert_eq!(request.param("missing"), None);
}

#[test]
fn a_query_value_that_is_not_utf8_is_decoded_lossily() {
    let request = parse("GET /admin/metadata?song=Cloud%FFSignal&mode=updinfo HTTP/1.0\r\n\r\n");
    let song = request.param("song").unwrap();
    assert!(song.starts_with("Cloud") && song.ends_with("Signal"));
    assert_ne!(song, "Cloud%FFSignal");
    assert_eq!(request.param("mode"), Some("updinfo"));
}

#[test]
fn head_over_the_limit_is_too_large() {
    let mut bytes = b"GET / HTTP/1.0\r\nX-Padding: ".to_vec();
    bytes.extend(std::iter::repeat_n(b'a', 9000));
    bytes.extend_from_slice(b"\r\n\r\n");
    let result = read_request(&mut Cursor::new(bytes), soon());
    assert!(matches!(result, Err(HttpError::TooLarge)), "{result:?}");
}

#[test]
fn the_limit_is_8192_bytes_of_head() {
    let head = |len: usize| {
        let start = b"GET / HTTP/1.0\r\nX-Padding: ";
        let end = b"\r\n\r\n";
        let mut bytes = start.to_vec();
        bytes.extend(std::iter::repeat_n(b'a', len - start.len() - end.len()));
        bytes.extend_from_slice(end);
        bytes
    };
    assert!(read_request(&mut Cursor::new(head(8192)), soon()).is_ok());
    let over = read_request(&mut Cursor::new(head(8193)), soon());
    assert!(matches!(over, Err(HttpError::TooLarge)), "{over:?}");
}

/// Never has a byte to give.
struct Silent;

impl Read for Silent {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::WouldBlock.into())
    }
}

#[test]
fn a_silent_client_times_out_once_the_deadline_passes() {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(20);
    let result = read_request(&mut Silent, deadline);
    assert!(matches!(result, Err(HttpError::Timeout)), "{result:?}");
    assert!(Instant::now() >= deadline);
}

/// Yields one byte, then times out on every read after.
struct Stalls {
    sent: bool,
}

impl Read for Stalls {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.sent || buf.is_empty() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.sent = true;
        buf[0] = b'G';
        Ok(1)
    }
}

#[test]
fn a_head_that_never_ends_times_out() {
    let past = Instant::now() - Duration::from_millis(1);
    let result = read_request(&mut Stalls { sent: false }, past);
    assert!(matches!(result, Err(HttpError::Timeout)), "{result:?}");
}

/// Times out on the first read, then hands out the head it holds.
struct SlowStart {
    stalled: bool,
    head: Cursor<Vec<u8>>,
}

impl Read for SlowStart {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.stalled {
            self.stalled = true;
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        self.head.read(buf)
    }
}

#[test]
fn a_read_timeout_before_the_deadline_is_not_the_end() {
    let mut reader = SlowStart {
        stalled: false,
        head: Cursor::new(b"GET /status HTTP/1.0\r\n\r\n".to_vec()),
    };
    let request = read_request(&mut reader, soon()).unwrap();
    assert_eq!(request.path, "/status");
}

#[test]
fn a_connection_closed_mid_head_is_an_error() {
    let result = read_request(
        &mut Cursor::new(b"GET / HTTP/1.0\r\nHost: a".to_vec()),
        soon(),
    );
    assert!(matches!(result, Err(HttpError::Io(_))), "{result:?}");
}

#[test]
fn lines_that_do_not_parse_are_malformed() {
    for head in [
        "GET /\r\n\r\n",
        "GET / HTTP/1.0 extra\r\n\r\n",
        "GET / FTP/1.0\r\n\r\n",
        "GET / HTTP/1.0\r\nno colon here\r\n\r\n",
        "GET / HTTP/1.0\r\n: no name\r\n\r\n",
        "GET / HTTP/1.0\r\nBad Name: x\r\n\r\n",
    ] {
        let result = read_request(&mut Cursor::new(head.as_bytes().to_vec()), soon());
        assert!(
            matches!(result, Err(HttpError::Malformed)),
            "{head:?}: {result:?}"
        );
    }
}

#[test]
fn a_repeated_header_answers_with_its_first_value() {
    let request =
        parse("GET / HTTP/1.0\r\nIce-Name: Morning Station\r\nice-name: Evening Station\r\n\r\n");
    assert_eq!(request.header("Ice-Name"), Some("Morning Station"));
}

#[test]
fn forwarded_address_counts_only_from_the_trusted_proxy() {
    let request = parse(
        "GET /morning/stream HTTP/1.0\r\nX-Forwarded-For: 203.0.113.50, 198.51.100.7\r\n\r\n",
    );
    let proxy = addr("192.0.2.1");
    let other = addr("192.0.2.99");

    assert_eq!(
        client_addr(proxy, &request, Some(proxy)),
        addr("198.51.100.7")
    );
    assert_eq!(client_addr(other, &request, Some(proxy)), other);
    assert_eq!(client_addr(proxy, &request, None), proxy);
}

#[test]
fn a_forged_forwarded_line_before_the_proxys_own_does_not_count() {
    let request = parse(
        "GET / HTTP/1.0\r\nX-Forwarded-For: 203.0.113.66\r\nX-Forwarded-For: 198.51.100.7\r\n\r\n",
    );
    let proxy = addr("192.0.2.1");
    assert_eq!(
        client_addr(proxy, &request, Some(proxy)),
        addr("198.51.100.7")
    );
}

#[test]
fn an_ipv4_mapped_address_counts_as_the_ipv4_address() {
    let request = parse("GET / HTTP/1.0\r\nX-Forwarded-For: ::ffff:198.51.100.7\r\n\r\n");
    // A server listening on [::] sees an IPv4 proxy as IPv4-mapped.
    assert_eq!(
        client_addr(addr("::ffff:127.0.0.1"), &request, Some(addr("127.0.0.1"))),
        addr("198.51.100.7")
    );
    // The proxy may be written either way in the config.
    assert_eq!(
        client_addr(addr("127.0.0.1"), &request, Some(addr("::ffff:127.0.0.1"))),
        addr("198.51.100.7")
    );
    // Any other peer is itself, in IPv4 form.
    assert_eq!(
        client_addr(
            addr("::ffff:203.0.113.5"),
            &request,
            Some(addr("127.0.0.1"))
        ),
        addr("203.0.113.5")
    );
    // The proxy is matched exactly, not by its network.
    let proxy = addr("2001:db8::1");
    assert_eq!(
        client_addr(addr("2001:db8::2"), &request, Some(proxy)),
        addr("2001:db8::2")
    );
}

#[test]
fn a_forwarded_address_that_does_not_parse_falls_back_to_the_peer() {
    let request = parse("GET / HTTP/1.0\r\nX-Forwarded-For: not-an-address\r\n\r\n");
    let proxy = addr("192.0.2.1");
    assert_eq!(client_addr(proxy, &request, Some(proxy)), proxy);
}

#[test]
fn bearer_and_basic_are_told_apart() {
    let with_bearer = parse(&format!(
        "GET /api/status HTTP/1.0\r\nAuthorization: Bearer {KEY}\r\n\r\n"
    ));
    assert_eq!(with_bearer.bearer(), Some(KEY));
    assert_eq!(with_bearer.basic_password(), None);

    let with_basic = parse(&format!(
        "GET /api/status HTTP/1.0\r\nAuthorization: Basic {}\r\n\r\n",
        basic("source", KEY)
    ));
    assert_eq!(with_basic.basic_password().as_deref(), Some(KEY));
    assert_eq!(with_basic.bearer(), None);

    let without = parse("GET /api/status HTTP/1.0\r\n\r\n");
    assert_eq!(without.bearer(), None);
    assert_eq!(without.basic_password(), None);
}

#[test]
fn a_basic_password_may_contain_a_colon() {
    let request = parse(&format!(
        "PUT /morning/stream HTTP/1.0\r\nAuthorization: Basic {}\r\n\r\n",
        basic("source", "invented:key")
    ));
    assert_eq!(request.basic_password().as_deref(), Some("invented:key"));
}

#[test]
fn respond_writes_a_closed_http_1_0_reply_with_its_length() {
    let mut out = Vec::new();
    respond(
        &mut out,
        429,
        "Too Many Requests",
        &[("Retry-After", "300")],
        b"locked out",
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    assert_eq!(lines.next(), Some("HTTP/1.0 429 Too Many Requests"));
    let headers: Vec<&str> = lines.collect();
    assert!(headers.contains(&"Retry-After: 300"));
    assert!(headers.contains(&"Connection: close"));
    assert!(headers.contains(&format!("Content-Length: {}", body.len()).as_str()));
    assert_eq!(body, "locked out");
}

#[test]
fn a_plus_in_the_query_is_a_space_and_an_encoded_plus_is_a_plus() {
    let request = parse("GET /admin/metadata?song=Cloud+Signal%2B HTTP/1.0\r\n\r\n");
    assert_eq!(request.param("song"), Some("Cloud Signal+"));
}

#[test]
fn a_request_printed_for_a_log_hides_its_key() {
    let request = parse(&source_head());
    let printed = format!("{request:?}");
    assert!(!printed.contains(&basic("source", KEY)), "{printed}");
    assert!(!printed.contains(KEY), "{printed}");
    assert!(printed.contains("<redacted>"), "{printed}");
    // The rest of the head still prints, so the log stays useful.
    assert!(printed.contains("/morning/stream"), "{printed}");
    assert!(printed.contains("Morning Station"), "{printed}");
}
