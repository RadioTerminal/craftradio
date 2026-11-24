//! Helpers the integration tests share: plain HTTP requests and replies, the public stats,
//! decoding a served stream, and TLS certificate files.

use base64::Engine;
use radio_server::{Decoded, Format, ListenerCap, Mount, SourceDecoder};
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long a test waits for something that should happen at once.
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);

/// A reply's status and headers.
pub(crate) struct Head {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
}

impl Head {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub(crate) fn client(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    // A server that never answers fails the test instead of hanging it.
    stream.set_read_timeout(Some(PATIENCE)).unwrap();
    stream.set_write_timeout(Some(PATIENCE)).unwrap();
    stream
}

/// Reads a reply head one byte at a time, so no byte of the body is taken.
pub(crate) fn read_head_bytes(stream: &mut impl Read) -> Vec<u8> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("a reply head");
        head.push(byte[0]);
    }
    head
}

pub(crate) fn read_head(stream: &mut impl Read) -> Head {
    let text = String::from_utf8(read_head_bytes(stream)).unwrap();
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

pub(crate) fn request(addr: SocketAddr, method: &str, path: &str) -> (TcpStream, Head) {
    let mut stream = client(addr);
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: radio.example.test\r\n\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    (stream, head)
}

/// Reads until the server closes the connection, failing the test after `limit`.
pub(crate) fn read_to_end_within(stream: &mut TcpStream, limit: Duration) -> Vec<u8> {
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

/// The public stats document, checked to be served as JSON to any origin.
pub(crate) fn stats(addr: SocketAddr) -> Value {
    let (mut stream, head) = request(addr, "GET", "/status-json.xsl");
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Type"), Some("application/json"));
    assert_eq!(head.header("Access-Control-Allow-Origin"), Some("*"));
    let body = read_to_end_within(&mut stream, PATIENCE);
    serde_json::from_slice(&body).expect("the stats are JSON")
}

/// The stats' list of live mounts, which is always an array.
pub(crate) fn sources(stats: &Value) -> &Vec<Value> {
    stats["icestats"]["source"]
        .as_array()
        .expect("icestats.source is an array")
}

/// The stats entry of `/<slug>/stream`, found as the listener page finds it.
pub(crate) fn source_entry(stats: &Value, slug: &str) -> Option<Value> {
    entry(stats, slug, "stream")
}

/// The stats entry of `/<slug>/<mount>`, found by the end of its `listenurl`.
pub(crate) fn entry(stats: &Value, slug: &str, mount: &str) -> Option<Value> {
    let path = format!("/{slug}/{mount}");
    sources(stats)
        .iter()
        .find(|source| {
            source["listenurl"]
                .as_str()
                .unwrap_or_default()
                .ends_with(&path)
        })
        .cloned()
}

/// Decodes a stream a server sends on `stream` (its reply head already read) with the
/// server's own decoder, until the tone is heard: true if it is within `limit`, false if
/// the stream ends or the time runs out first.
pub(crate) fn hears(stream: TcpStream, format: Format, limit: Duration) -> bool {
    // A mount of the test's own relays the bytes, as the server's transcoder reads them.
    let mount = Mount::new(ListenerCap::new(1));
    let mut guard = mount
        .begin(format, "Morning Station".into(), None)
        .expect("a fresh mount takes a source");
    let listener = mount.listen().expect("the relay is live");
    let stop = Arc::new(AtomicBool::new(false));
    let relay = {
        let stop = Arc::clone(&stop);
        let mut stream = stream;
        std::thread::spawn(move || {
            let deadline = Instant::now() + limit;
            let mut buf = [0; 4096];
            // Dropping the guard at the end ends the decoder below.
            while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => guard.write(&buf[..n]),
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                    Err(_) => break,
                }
            }
        })
    };
    let heard = match SourceDecoder::open(listener, Arc::clone(&stop)) {
        Ok(mut decoder) => {
            let mut samples = Vec::new();
            loop {
                samples.clear();
                match decoder.next(&mut samples) {
                    Ok(Decoded::Audio) if samples.iter().any(|s| s.abs() > 0.01) => break true,
                    Ok(Decoded::Audio) => {}
                    Ok(_) | Err(_) => break false,
                }
            }
        }
        Err(_) => false,
    };
    stop.store(true, Ordering::Release);
    relay.join().unwrap();
    heard
}

/// A certificate for 127.0.0.1 and its key, written as PEM files in a directory of their
/// own; made fresh by each test.
pub(crate) struct TlsFiles {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) cert: std::path::PathBuf,
    pub(crate) key: std::path::PathBuf,
    pub(crate) root: rustls::pki_types::CertificateDer<'static>,
}

/// DER as a PEM block, written by hand: the workspace's `rcgen` has no PEM feature.
pub(crate) fn pem_text(label: &str, der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = encoded
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        lines.join("\n")
    )
}

pub(crate) fn tls_files() -> TlsFiles {
    let made = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&cert, pem_text("CERTIFICATE", made.cert.der())).unwrap();
    std::fs::write(
        &key,
        pem_text("PRIVATE KEY", &made.signing_key.serialize_der()),
    )
    .unwrap();
    TlsFiles {
        dir,
        cert,
        key,
        root: made.cert.der().clone(),
    }
}
