//! Pairing with a station server: the address rule, the fetch of the pairing document, and
//! the state the Server page and the headless app show.
//!
//! The station key goes from the person (or the keychain) to the background lane, to the
//! `Authorization` header of one request, and to the keychain entry of the paired mount, which
//! the Icecast output reads. It never reaches the settings, the snapshot, a notice or a URL.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use radio_engine::{Command, State};
use radio_model::{PAIRING_VERSION, PairingDocument, SERVER_ADDRESS_KEY, SERVER_PART};
use radio_ui_egui::view::{PairState, PairingView, Secret, is_loopback};

use crate::{EngineSource, SecretStore};

/// The whole fetch of the pairing document, connection included, takes at most this long.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// A connection to the server takes at most this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A pairing document is small; a longer answer is not one.
const MAX_DOCUMENT: u64 = 64 * 1024;

/// Why a pairing try did not give a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairError {
    /// The server answered 401: it does not know the key.
    KeyRefused,
    /// No answer, or not now (503, 429, a timeout); the reason in words.
    Unreachable(String),
    /// An answer the app cannot pair with, or the keychain failed; the reason in words.
    Refused(String),
}

/// The server address in its one accepted shape, or why it is not one, in words.
///
/// The rule: a scheme is required; `https://` to any host, `http://` only to this computer
/// (`localhost`, 127.0.0.0/8, `::1`), so the key never crosses a network in clear; the server
/// only, with no user, path or query. A trailing slash is dropped.
pub fn check_address(address: &str) -> Result<String, String> {
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
    if scheme == "http" && !is_loopback(host) {
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

/// The pairing document from its JSON. The version is read first, so a newer server is
/// refused in words and not with a message about a missing field.
pub(crate) fn read_document(body: &str) -> Result<PairingDocument, PairError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| PairError::Refused("the server's answer is not a pairing document".into()))?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(v) if v == u64::from(PAIRING_VERSION) => {}
        Some(v) if v > u64::from(PAIRING_VERSION) => {
            return Err(PairError::Refused(
                "This server needs a newer CraftRadio: update the app, then pair again".into(),
            ));
        }
        _ => {
            return Err(PairError::Refused(
                "the server's pairing document has a version this app does not read".into(),
            ));
        }
    }
    let document: PairingDocument = serde_json::from_value(value).map_err(|e| {
        PairError::Refused(format!("the server's pairing document is incomplete: {e}"))
    })?;
    let ice = &document.icecast;
    if ice.host.is_empty() || ice.host.contains(['/', '@', ' ']) || !ice.mount.starts_with('/') {
        return Err(PairError::Refused(
            "the server's pairing document names no usable push address".into(),
        ));
    }
    // The key is the push password: it never crosses a network in clear.
    if !ice.tls && !is_loopback(&ice.host) {
        return Err(PairError::Refused(format!(
            "the server asks for a push to {} without TLS, which would send the key in clear",
            ice.host
        )));
    }
    Ok(document)
}

/// Fetches `<address>/api/station` with `key` as the Basic password. `address` passed
/// [`check_address`].
pub(crate) fn fetch(address: &str, key: &str) -> Result<PairingDocument, PairError> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .http_status_as_error(false)
        // A redirect could lead the key to another host.
        .max_redirects(0)
        .max_redirects_will_error(false);
    let config = if address.starts_with("https://") {
        config.tls_config(radio_stream::https_tls().map_err(PairError::Unreachable)?)
    } else {
        config
    };
    let agent = ureq::Agent::new_with_config(config.build());
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("source:{key}"));
    let mut response = agent
        .get(format!("{address}/api/station"))
        .header("Authorization", format!("Basic {basic}"))
        .call()
        .map_err(|e| PairError::Unreachable(describe(&e)))?;
    match response.status().as_u16() {
        200 => {
            let body = response
                .body_mut()
                .with_config()
                .limit(MAX_DOCUMENT)
                .read_to_string()
                .map_err(|e| PairError::Unreachable(describe(&e)))?;
            read_document(&body)
        }
        401 => Err(PairError::KeyRefused),
        429 => Err(PairError::Unreachable(
            "the server refuses keys from this computer for a while after wrong ones; try \
             again in a few minutes"
                .into(),
        )),
        503 => Err(PairError::Unreachable(
            "the server is busy (503); try again later".into(),
        )),
        404 => Err(PairError::Refused(
            "that address has no pairing; is it a CraftRadio station server?".into(),
        )),
        code @ 300..=399 => Err(PairError::Refused(format!(
            "the server sends the app elsewhere ({code}); pair with its own address"
        ))),
        code => Err(PairError::Unreachable(format!(
            "the server answered {code}"
        ))),
    }
}

/// A failed request in words.
fn describe(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => format!("no answer within {} seconds", FETCH_TIMEOUT.as_secs()),
        ureq::Error::HostNotFound => "the host name is not known".into(),
        ureq::Error::ConnectionFailed => "the connection failed".into(),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            "nothing answers at that address".into()
        }
        ureq::Error::BodyExceedsLimit(_) => {
            "the answer is too long to be a pairing document".into()
        }
        other => other.to_string(),
    }
}

/// What the background lane hands back for a pairing try.
pub(crate) struct PairAnswer {
    pub(crate) seq: u64,
    pub(crate) address: String,
    pub(crate) result: Result<PairingDocument, PairError>,
}

/// One pairing try, off the UI thread: the key (given, or read from `paired_entry`, the
/// keychain entry of the mount the station is paired with), the fetch, and on success the
/// key under the entry of the document's mount, where the Icecast output reads it. A key the
/// server refused is never stored.
pub(crate) fn run(
    seq: u64,
    address: String,
    key: Option<Secret>,
    paired_entry: Option<String>,
    store: &Arc<dyn SecretStore + Sync>,
) -> PairAnswer {
    let result = pair_with(&address, key, paired_entry, store.as_ref());
    PairAnswer {
        seq,
        address,
        result,
    }
}

fn pair_with(
    address: &str,
    key: Option<Secret>,
    paired_entry: Option<String>,
    store: &dyn SecretStore,
) -> Result<PairingDocument, PairError> {
    let (key, from_store) = match key {
        Some(key) => (key, false),
        None => {
            let entry = paired_entry.as_deref().ok_or_else(|| {
                PairError::Refused("enter the station key; the server's admin has it".into())
            })?;
            match store.get(entry) {
                Ok(Some(key)) if !key.is_empty() => (Secret::new(key), true),
                Ok(_) => {
                    return Err(PairError::Refused(
                        "the keychain has no key for this server; enter the key again".into(),
                    ));
                }
                Err(e) => {
                    return Err(PairError::Refused(format!(
                        "the keychain did not answer: {e}"
                    )));
                }
            }
        }
    };
    let document = fetch(address, key.expose())?;
    let ice = &document.icecast;
    let entry = crate::icecast::mount_entry(&ice.user, &ice.host, ice.port, &ice.mount);
    let moved = paired_entry.as_deref() != Some(entry.as_str());
    if !from_store || moved {
        store
            .set(&entry, key.expose())
            .map_err(|e| PairError::Refused(format!("the keychain did not take the key: {e}")))?;
    }
    // The server moved the push: the key of the old mount goes, so it has one copy.
    if moved && let Some(old) = paired_entry {
        let _ = store.delete(&old);
    }
    Ok(document)
}

/// The pairing as the source keeps it.
#[derive(Default)]
pub(crate) struct Pairing {
    /// The server a try is for, or the paired one once a try answered.
    address: String,
    state: PairState,
    /// The last try sent to the background lane. Older answers are dropped.
    asked: u64,
    /// True once the paired server was asked: by the start, or by a person.
    started: bool,
}

/// The address of the paired server, from the settings.
pub(crate) fn paired_address(state: &State) -> Option<String> {
    state
        .settings(SERVER_PART)
        .and_then(|s| s.get(SERVER_ADDRESS_KEY))
        .and_then(|v| v.as_str())
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
}

impl EngineSource {
    /// The keychain entry of the paired mount, when the station is paired.
    fn paired_entry(&self) -> Option<String> {
        paired_address(&self.state)
            .map(|_| crate::icecast::secret_key(&crate::icecast_view(&self.state)))
    }

    /// Asks the paired server again, once, when a snapshot first sees the station paired (at
    /// start, as soon as playout shows the saved state): the settings it hands out overwrite
    /// hand edits. A down server leaves the last paired settings as they are. It never asks
    /// twice by itself, so a refused key is not tried again.
    pub(crate) fn refetch_at_start(&mut self) {
        if self.pairing.started {
            return;
        }
        if let Some(address) = paired_address(&self.state) {
            self.pairing.started = true;
            self.ask_server(address, None);
        }
    }

    /// Sends a pairing try to the background lane.
    fn ask_server(&mut self, address: String, key: Option<Secret>) {
        self.pairing.asked += 1;
        self.pairing.address.clone_from(&address);
        let job = crate::bg::Job::Pair {
            seq: self.pairing.asked,
            address,
            key,
            paired_entry: self.paired_entry(),
            store: Arc::clone(&self.secrets),
        };
        let sent = self
            .worker
            .as_ref()
            .is_some_and(|w| w.jobs.send(job).is_ok());
        self.pairing.state = if sent {
            PairState::Pairing
        } else {
            PairState::Refused("the background lane has stopped; restart the app".into())
        };
    }

    /// `Edit::Pair`: checks the address and asks the server. An empty key uses the stored
    /// one of the paired server.
    pub(crate) fn pair(&mut self, address: &str, key: &Secret) -> Result<usize, String> {
        let address = check_address(address)?;
        let key = key.expose().trim();
        let key = if key.is_empty() {
            if paired_address(&self.state).as_deref() != Some(address.as_str()) {
                return Err("Enter the station key; the server's admin has it".into());
            }
            None
        } else {
            Some(Secret::new(key))
        };
        if self.worker.is_none() {
            return Err("The background lane could not start".into());
        }
        // A person asked: the start needs not ask again.
        self.pairing.started = true;
        self.ask_server(address, key);
        Ok(1)
    }

    /// `Edit::Unpair`: the settings pairing wrote go, the key leaves the keychain, and the
    /// Icecast push closes with the settings.
    pub(crate) fn unpair(&mut self) -> Result<usize, String> {
        let Some(entry) = self.paired_entry() else {
            return Err("The station is not paired with a server".into());
        };
        self.run_edit(Command::ServerUnpair)?;
        if let Some(w) = &self.worker {
            let _ = w.jobs.send(crate::bg::Job::Forget {
                key: entry,
                store: Arc::clone(&self.secrets),
            });
        }
        // A try still on its way is for a pairing that is gone.
        self.pairing.asked += 1;
        self.pairing.address.clear();
        self.pairing.state = PairState::NotPaired;
        self.secret_known = None;
        Ok(1)
    }

    /// The answer of a pairing try from the background lane.
    pub(crate) fn pair_answer(&mut self, answer: PairAnswer) {
        if answer.seq != self.pairing.asked {
            return;
        }
        let was_refused = self.pairing.state == PairState::KeyRefused;
        self.pairing.address.clone_from(&answer.address);
        match answer.result {
            Ok(document) => {
                let sent = self.send(
                    "",
                    Command::ServerPair {
                        address: answer.address,
                        document,
                    },
                );
                self.pairing.state = match sent {
                    Ok(()) => PairState::Paired,
                    Err(e) => PairState::Refused(format!("the settings were not written: {e}")),
                };
                // The key is stored: the snapshot asks the keychain again, and a rotated key
                // for an unchanged mount applies at once.
                self.secret_known = None;
                self.sync_icecast(true);
            }
            Err(PairError::KeyRefused) => {
                self.pairing.state = PairState::KeyRefused;
                // The push would walk the server's lockout with the same key: it waits.
                if paired_address(&self.state).is_some() {
                    self.sync_icecast(true);
                }
            }
            Err(PairError::Unreachable(reason)) => {
                self.pairing.state = PairState::Unreachable(reason);
            }
            Err(PairError::Refused(reason)) => self.pairing.state = PairState::Refused(reason),
        }
        if was_refused && self.pairing.state != PairState::KeyRefused {
            self.sync_icecast(true);
        }
    }

    /// True while the paired server refused the key: the Icecast push then stays closed, so
    /// it does not walk the server's lockout.
    pub(crate) fn push_held(&self) -> bool {
        self.pairing.state == PairState::KeyRefused && paired_address(&self.state).is_some()
    }

    /// The pairing for the snapshot. The key is not in it.
    pub(crate) fn pairing_view(&self) -> PairingView {
        let paired = paired_address(&self.state);
        let address = if self.pairing.address.is_empty() {
            paired.clone().unwrap_or_default()
        } else {
            self.pairing.address.clone()
        };
        PairingView {
            address,
            paired: paired.is_some(),
            state: self.pairing.state.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_slash_and_case_are_dropped() {
        assert_eq!(
            check_address(" HTTPS://Radio.Example.org/ ").as_deref(),
            Ok("https://radio.example.org")
        );
        assert_eq!(
            check_address("http://[::1]:8000").as_deref(),
            Ok("http://[::1]:8000")
        );
        assert_eq!(
            check_address("http://127.0.0.2:8000/").as_deref(),
            Ok("http://127.0.0.2:8000")
        );
    }

    #[test]
    fn a_document_of_a_newer_version_is_refused_in_words() {
        let newer = r#"{"version": 2, "something": "else"}"#;
        let Err(PairError::Refused(reason)) = read_document(newer) else {
            panic!("a newer document paired");
        };
        assert!(reason.contains("newer CraftRadio"), "{reason}");
    }

    #[test]
    fn a_push_in_clear_to_another_computer_is_refused() {
        let doc = serde_json::json!({
            "version": 1, "slug": "dawn", "name": "Dawn", "public_url": "http://radio.example.org",
            "icecast": {"host": "radio.example.org", "port": 80, "tls": false,
                        "mount": "/dawn/stream", "user": "source"},
            "stream_url": "http://radio.example.org/dawn/stream", "outputs": []
        });
        assert!(matches!(
            read_document(&doc.to_string()),
            Err(PairError::Refused(_))
        ));
    }
}
