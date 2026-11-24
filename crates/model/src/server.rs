//! Pairing with a station server: the document the server hands a station's app at
//! `GET /api/station`, and the settings part pairing writes. The document never holds the
//! station key.

/// The settings part of the pairing: the server's address, and what its document offered.
pub const SERVER_PART: &str = "server";
/// The key of the server's address in [`SERVER_PART`]. A station is paired while it is set.
pub const SERVER_ADDRESS_KEY: &str = "address";
/// The key of the paired push in [`SERVER_PART`]: the document's Icecast host, port, mount
/// and user, which name the keychain entry of the station key. Hand edits of `icecast.*`
/// do not move it.
pub const SERVER_PUSH_KEY: &str = "push";
/// The version of the pairing document's shape that this app reads.
pub const PAIRING_VERSION: u32 = 1;

/// The server address in its one accepted shape, or why it is not one, in words.
///
/// The rule: a scheme is required; `https://` to any host, `http://` only to this computer
/// (`localhost`, 127.0.0.0/8, `::1`), so the key never crosses a network in clear; the server
/// only, with no user, path or query. A trailing slash is dropped.
pub fn check_server_address(address: &str) -> Result<String, String> {
    let typed = address.trim().trim_end_matches('/');
    if typed.is_empty() {
        return Err("Enter the server's address, such as https://radio.example.org".into());
    }
    let lower = typed.to_ascii_lowercase();
    let (scheme, rest) = if let Some(rest) = lower.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = lower.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err("The address starts with https://, such as https://radio.example.org".into());
    };
    if rest.contains('@') {
        return Err("The address holds no user name or key; the key has a field of its own".into());
    }
    if rest.contains(['/', '?', '#']) {
        return Err(
            "The address is the server only, with no path, such as https://radio.example.org"
                .into(),
        );
    }
    if rest.chars().any(char::is_whitespace) {
        return Err("The address has a space in it".into());
    }
    let host = host_of(rest).ok_or_else(|| {
        "The address names no host, or its port is not a number from 1 to 65535".to_owned()
    })?;
    if scheme == "http" && !is_loopback_host(host) {
        return Err(
            "http:// is only for a server on this computer; use https:// so the key is \
             encrypted"
                .into(),
        );
    }
    Ok(format!("{scheme}://{rest}"))
}

/// The host of `host`, `host:port`, `[v6]` or `[v6]:port`, when the port is 1–65535.
fn host_of(authority: &str) -> Option<&str> {
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        (host, port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    if let Some(port) = port {
        port.parse::<u16>().ok().filter(|p| *p > 0)?;
    }
    Some(host)
}

/// True for `localhost` or a loopback address (127.0.0.0/8, `::1`), without brackets.
pub fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The pairing document, field for field as the server writes it. Unknown fields are read
/// past, so a server that offers more in a later phase still pairs.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct PairingDocument {
    /// The document's shape; only [`PAIRING_VERSION`] is read (see [`Self::check_version`]).
    pub version: u32,
    pub slug: String,
    pub name: String,
    /// The server's public URL, without a trailing slash.
    pub public_url: String,
    /// Where the app pushes its stream.
    pub icecast: PairingIcecast,
    /// The pushed stream as listeners reach it: `<public_url>/<slug>/stream`.
    pub stream_url: String,
    /// The re-encoded streams the server offers.
    pub outputs: Vec<PairingOutput>,
}

/// The Icecast push the app makes to the server. Its password is the station key, which the
/// document does not carry.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct PairingIcecast {
    /// A host name or IP address, an IPv6 address without brackets.
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub mount: String,
    pub user: String,
}

/// One re-encoded stream of the station on the server.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct PairingOutput {
    pub mount: String,
    /// `opus` or `mp3`.
    pub format: String,
    pub kbps: u32,
    pub url: String,
}

impl PairingDocument {
    /// Refuses, in words, a document whose shape this app does not read.
    pub fn check_version(&self) -> Result<(), String> {
        match self.version {
            PAIRING_VERSION => Ok(()),
            v if v > PAIRING_VERSION => Err(format!(
                "the server's pairing document is version {v}, newer than version \
                 {PAIRING_VERSION}, the one this app reads: update the app"
            )),
            v => Err(format!(
                "the server's pairing document is version {v}; this app reads version \
                 {PAIRING_VERSION} only"
            )),
        }
    }
}
