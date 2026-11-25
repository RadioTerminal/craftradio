//! The pairing document: what a station's app needs to push to this server and to link
//! listeners to it, served at `GET /api/station` to the station's key. It names the
//! station and its streams and never holds the key or its hash.

use crate::config::{Output, Station};
use serde::{Deserialize, Serialize};

/// The version of the document's shape. An app refuses a version it does not know.
pub const VERSION: u32 = 1;
/// The user name the station's Icecast push sends; the server reads only the password.
const SOURCE_USER: &str = "source";

/// What `GET /api/station` answers, as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    pub slug: String,
    pub name: String,
    /// The server's public URL, without a trailing slash.
    pub public_url: String,
    /// Where the app pushes its stream.
    pub icecast: IcecastTarget,
    /// The pushed stream as listeners reach it: `<public_url>/<slug>/stream`.
    pub stream_url: String,
    /// The re-encoded streams, in the order they were set.
    pub outputs: Vec<OutputEntry>,
    /// Whether the app may change the outputs (`PUT /api/station/outputs`): the server has
    /// a `state_dir` to keep them in. An older server sends no field; read it as false.
    #[serde(default)]
    pub outputs_editable: bool,
}

/// The Icecast push the app makes to this server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IcecastTarget {
    /// A host name or IP address, an IPv6 address without brackets.
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// `/<slug>/stream`.
    pub mount: String,
    pub user: String,
}

/// One re-encoded stream of the station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputEntry {
    pub mount: String,
    /// `opus` or `mp3`.
    pub format: String,
    pub kbps: u32,
    /// `<public_url>/<slug>/<mount>`.
    pub url: String,
}

/// A public URL taken apart: `http` or `https`, a host and maybe a port, and no path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl {
    /// The URL without a trailing slash.
    pub base: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

impl PublicUrl {
    /// Reads `http(s)://host[:port]`, with or without a trailing slash. Anything else is
    /// refused in words: the station's push goes to the host and port this names, and to
    /// `/<slug>/stream` on it, so a path, a user or a query could not be followed.
    pub fn parse(url: &str) -> Result<PublicUrl, String> {
        let base = url.trim_end_matches('/');
        let (tls, rest) = if let Some(rest) = base.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = base.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(format!(
                "public_url {url:?} must start with https:// or http://"
            ));
        };
        if rest.contains(['/', '?', '#', '@']) || rest.chars().any(char::is_whitespace) {
            return Err(format!(
                "public_url {url:?} must be the scheme, a host and maybe a port, with no \
                 path: stations push to /<slug>/stream on that host"
            ));
        }
        let (host, port) = split_host_port(rest).ok_or_else(|| {
            format!(
                "public_url {url:?} has no host, or a port that is not a number from 1 to 65535"
            )
        })?;
        let port = port.unwrap_or(if tls { 443 } else { 80 });
        Ok(PublicUrl {
            base: base.to_owned(),
            host: host.to_owned(),
            port,
            tls,
        })
    }
}

/// `host`, `host:port`, `[v6]` or `[v6]:port`: the host (brackets dropped) and the port if
/// one is given. None for an empty host or a port that is not 1–65535.
fn split_host_port(authority: &str) -> Option<(&str, Option<u16>)> {
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
    let port = match port {
        None => None,
        Some(port) => Some(port.parse::<u16>().ok().filter(|port| *port > 0)?),
    };
    Some((host, port))
}

impl Document {
    /// The document for `station` on a server reached at `public_url`, whose outputs are
    /// now `outputs`, which the app may change when `outputs_editable`.
    pub fn for_station(
        public_url: &PublicUrl,
        station: &Station,
        outputs: &[Output],
        outputs_editable: bool,
    ) -> Document {
        let base = &public_url.base;
        let slug = &station.slug;
        let outputs = outputs
            .iter()
            .map(|output| OutputEntry {
                mount: output.mount.clone(),
                format: output.format.name().to_owned(),
                kbps: output.kbps,
                url: format!("{base}/{slug}/{}", output.mount),
            })
            .collect();
        Document {
            version: VERSION,
            slug: slug.clone(),
            name: station.name.clone(),
            public_url: base.clone(),
            icecast: IcecastTarget {
                host: public_url.host.clone(),
                port: public_url.port,
                tls: public_url.tls,
                mount: format!("/{slug}/stream"),
                user: SOURCE_USER.to_owned(),
            },
            stream_url: format!("{base}/{slug}/stream"),
            outputs,
            outputs_editable,
        }
    }
}
