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
