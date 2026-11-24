//! Helpers the integration tests share: plain HTTP requests and replies, the public stats,
//! and TLS certificate files.

use base64::Engine;
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
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
    let path = format!("/{slug}/stream");
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
