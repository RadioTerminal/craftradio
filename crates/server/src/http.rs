//! Reading HTTP requests and writing replies, by hand, one connection per thread.
//!
//! A source's endless body follows its request head on the same socket, so
//! [`read_request`] reads one byte at a time and never past the blank line.

use base64::Engine;
use percent_encoding::percent_decode_str;
use std::io::{ErrorKind, Read, Write};
use std::net::IpAddr;
use std::time::Instant;

/// The most bytes a request head may take, blank line included.
const MAX_HEAD: usize = 8192;

/// Why a request could not be read.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("request head is over {MAX_HEAD} bytes")]
    TooLarge,
    #[error("request head did not arrive in time")]
    Timeout,
    #[error("request head does not parse")]
    Malformed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A request head. The path and query are percent-decoded.
///
/// Its `Debug` output prints the `Authorization` header as `<redacted>`, so a logged
/// request never carries a key.
#[derive(Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, value)| {
                let shown = if name.eq_ignore_ascii_case("Authorization") {
                    "<redacted>"
                } else {
                    value.as_str()
                };
                (name.as_str(), shown)
            })
            .collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .field("headers", &headers)
            .finish()
    }
}

impl Request {
    /// The value of the first header with this name, ignoring case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The password of `Authorization: Basic`, whatever the user name.
    pub fn basic_password(&self) -> Option<String> {
        let encoded = self.credentials("Basic")?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?;
        let text = String::from_utf8(decoded).ok()?;
        text.split_once(':')
            .map(|(_, password)| password.to_owned())
    }

    /// The token of `Authorization: Bearer`.
    pub fn bearer(&self) -> Option<&str> {
        self.credentials("Bearer")
    }

    /// The first query value with this name.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn last_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn credentials(&self, scheme: &str) -> Option<&str> {
        let (given, rest) = self.header("Authorization")?.split_once(' ')?;
        let rest = rest.trim();
        (given.eq_ignore_ascii_case(scheme) && !rest.is_empty()).then_some(rest)
    }
}

/// Reads a request head, up to and including the blank line and not one byte more.
///
/// The caller sets the socket's read timeout; a read that times out before `deadline`
/// is tried again, and once `deadline` has passed the request is [`HttpError::Timeout`].
pub fn read_request(stream: &mut impl Read, deadline: Instant) -> Result<Request, HttpError> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err(HttpError::TooLarge);
        }
        if Instant::now() >= deadline {
            return Err(HttpError::Timeout);
        }
        let mut byte = [0];
        match stream.read(&mut byte) {
            Ok(0) => return Err(std::io::Error::from(ErrorKind::UnexpectedEof).into()),
            Ok(_) => head.extend_from_slice(&byte),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::Interrupted | ErrorKind::WouldBlock | ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    parse(&head)
}

fn parse(head: &[u8]) -> Result<Request, HttpError> {
    let text = std::str::from_utf8(head).map_err(|_| HttpError::Malformed)?;
    let mut lines = text
        .strip_suffix("\r\n\r\n")
        .ok_or(HttpError::Malformed)?
        .split("\r\n");
    let request_line = lines.next().ok_or(HttpError::Malformed)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HttpError::Malformed);
    };
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
        return Err(HttpError::Malformed);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines.map(header).collect::<Result<_, _>>()?;
    Ok(Request {
        method: method.to_owned(),
        path: decode(path),
        query: query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (decode_query(key), decode_query(value))
            })
            .collect(),
        headers,
    })
}

fn header(line: &str) -> Result<(String, String), HttpError> {
    let (name, value) = line.split_once(':').ok_or(HttpError::Malformed)?;
    if name.is_empty()
        || name
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(HttpError::Malformed);
    }
    Ok((name.to_owned(), value.trim().to_owned()))
}

fn decode(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().into_owned()
}

/// A query part: `+` is a space, as in a form; the app itself writes `%20` and `%2B`.
fn decode_query(text: &str) -> String {
    decode(&text.replace('+', " "))
}

/// The address a request came from: the last `X-Forwarded-For` entry when the peer is the
/// trusted proxy, else the peer.
pub fn client_addr(peer: IpAddr, request: &Request, trusted_proxy: Option<IpAddr>) -> IpAddr {
    if trusted_proxy != Some(peer) {
        return peer;
    }
    // A proxy may append its own line rather than extend the client's, so the last line
    // counts, not the first.
    request
        .last_header("X-Forwarded-For")
        .and_then(|list| list.rsplit(',').next())
        .and_then(|last| last.trim().parse().ok())
        .unwrap_or(peer)
}

/// Writes a whole HTTP/1.0 reply that closes the connection.
pub fn respond(
    stream: &mut impl Write,
    status: u16,
    reason: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut reply = format!("HTTP/1.0 {status} {reason}\r\n");
    for (name, value) in headers {
        reply.push_str(&format!("{name}: {value}\r\n"));
    }
    reply.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = reply.into_bytes();
    bytes.extend_from_slice(body);
    stream.write_all(&bytes)?;
    stream.flush()
}
