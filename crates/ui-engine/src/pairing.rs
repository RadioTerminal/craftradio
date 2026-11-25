//! Pairing with a station server: the address rule, the fetch of the pairing document, and
//! the state the Server page and the headless app show.
//!
//! The station key goes from the person (or the keychain) to the background lane, to the
//! `Authorization` header of one request, and to the keychain entry of the paired mount, which
//! the Icecast output reads. It never reaches the settings, the snapshot, a notice or a URL.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use radio_engine::{Command, EngineError, Outcome, State};
use radio_model::{
    OutputSpec, PAIRING_VERSION, PairingDocument, PairingOutput, SERVER_ADDRESS_KEY,
    SERVER_OUTPUTS_EDITABLE_KEY, SERVER_OUTPUTS_KEY, SERVER_PART, SERVER_PUSH_KEY,
};
use radio_ui_egui::view::{
    OUTPUTS_READ_ONLY, OutputRow, OutputsState, PairState, PairingView, Secret, StreamFormat,
    is_loopback,
};

use crate::{EngineSource, SecretStore};

/// What the strip, the status bar and the alarm say while the paired server refuses the key.
pub(crate) const KEY_REFUSED: &str =
    "the server refused the station key; pair again with the new key";
/// The whole fetch of the pairing document, connection included, takes at most this long.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// A connection to the server takes at most this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A pairing document is small; a longer answer is not one.
const MAX_DOCUMENT: u64 = 64 * 1024;
/// The server's reason for a refused list of outputs is a line of words.
const MAX_REASON: u64 = 4 * 1024;

/// Why a pairing try did not give a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    /// The server answered 401: it does not know the key.
    KeyRefused,
    /// No answer, or not now (503, 429, a timeout); the reason in words.
    Unreachable(String),
    /// An answer the app cannot pair with, or the keychain failed; the reason in words.
    Refused(String),
}

/// The server address in its one accepted shape, or why it is not one, in words: the rule
/// of [`radio_model::check_server_address`], which the engine's `server.pair` applies too.
pub fn check_address(address: &str) -> Result<String, String> {
    radio_model::check_server_address(address)
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

/// The agent for a request that carries the station key to the server at `address` (in
/// [`check_address`]'s shape): no proxy, no redirects, short timeouts.
fn agent(address: &str) -> Result<ureq::Agent, String> {
    let config = ureq::Agent::config_builder()
        // A proxy from the environment would see the key, in clear over http.
        .proxy(None)
        .timeout_global(Some(FETCH_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .http_status_as_error(false)
        // A redirect could lead the key to another host.
        .max_redirects(0)
        .max_redirects_will_error(false);
    let config = if address.starts_with("https://") {
        config.tls_config(radio_stream::https_tls()?)
    } else {
        config
    };
    Ok(ureq::Agent::new_with_config(config.build()))
}

/// The `Authorization` header with `key` as the Basic password.
fn basic(key: &str) -> String {
    let pair = base64::engine::general_purpose::STANDARD.encode(format!("source:{key}"));
    format!("Basic {pair}")
}

/// Fetches `<address>/api/station` with `key` as the Basic password. An address outside
/// [`check_address`]'s rule is refused before anything is sent, wherever it came from.
pub fn fetch(address: &str, key: &str) -> Result<PairingDocument, PairError> {
    let address = &check_address(address).map_err(PairError::Refused)?;
    let agent = agent(address).map_err(PairError::Unreachable)?;
    let mut response = agent
        .get(format!("{address}/api/station"))
        .header("Authorization", basic(key))
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

/// Why a list of outputs sent to the server did not give a new document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutError {
    /// The server answered 401: it does not know the key.
    KeyRefused,
    /// No answer, or not now (503, 429, a timeout); the reason in words.
    Unreachable(String),
    /// The server refused the list (its reason, in its words), or keeps no outputs the app
    /// may change, or answered with something that is not a pairing document.
    Refused(String),
}

/// Sends `outputs`, the whole new list, to `<address>/api/station/outputs` with `key` as the
/// Basic password, and gives the pairing document the server answers with. An address
/// outside [`check_address`]'s rule is refused before anything is sent.
pub fn put_outputs(
    address: &str,
    key: &str,
    outputs: &[OutputSpec],
) -> Result<PairingDocument, PutError> {
    let address = &check_address(address).map_err(PutError::Refused)?;
    let body = serde_json::to_vec(outputs)
        .map_err(|e| PutError::Refused(format!("the outputs cannot be sent: {e}")))?;
    let agent = agent(address).map_err(PutError::Unreachable)?;
    // A body of bytes goes with its Content-Length, which the server needs.
    let mut response = agent
        .put(format!("{address}/api/station/outputs"))
        .header("Authorization", basic(key))
        .header("Content-Type", "application/json")
        .send(&body[..])
        .map_err(|e| PutError::Unreachable(describe(&e)))?;
    let status = response.status().as_u16();
    let limit = if status == 200 {
        MAX_DOCUMENT
    } else {
        MAX_REASON
    };
    let text = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string();
    match status {
        200 => {
            let body = text.map_err(|e| PutError::Unreachable(describe(&e)))?;
            read_document(&body).map_err(|e| match e {
                PairError::KeyRefused => PutError::KeyRefused,
                PairError::Unreachable(r) => PutError::Unreachable(r),
                PairError::Refused(r) => PutError::Refused(r),
            })
        }
        // The server's own reason: a bad bitrate, a taken mount, or no state_dir.
        400 | 403 | 413 | 500 => {
            let reason = text.unwrap_or_default();
            let reason = reason.trim();
            Err(PutError::Refused(if reason.is_empty() {
                format!("the server refused the outputs ({status})")
            } else {
                reason.to_owned()
            }))
        }
        401 => Err(PutError::KeyRefused),
        429 => Err(PutError::Unreachable(
            "the server refuses keys from this computer for a while after wrong ones; try \
             again in a few minutes"
                .into(),
        )),
        503 => Err(PutError::Unreachable(
            "the server is busy (503); try again later".into(),
        )),
        404 | 405 => Err(PutError::Refused(
            "this server cannot change its outputs from the app; update craftradio-server".into(),
        )),
        code @ 300..=399 => Err(PutError::Refused(format!(
            "the server sends the app elsewhere ({code}); pair with its own address"
        ))),
        code => Err(PutError::Unreachable(format!("the server answered {code}"))),
    }
}

/// The rows of the Server page as the server reads them.
pub(crate) fn output_specs(rows: &[OutputRow]) -> Vec<OutputSpec> {
    rows.iter()
        .map(|r| OutputSpec {
            mount: r.mount.clone(),
            format: match r.format {
                StreamFormat::Opus => "opus",
                StreamFormat::Mp3 => "mp3",
            }
            .to_owned(),
            kbps: r.kbps,
        })
        .collect()
}

/// The outputs the paired settings list, as rows. An output of a format this app does not
/// know is left out.
pub(crate) fn output_rows(state: &State) -> Vec<OutputRow> {
    let listed = state
        .settings(SERVER_PART)
        .and_then(|s| s.get(SERVER_OUTPUTS_KEY))
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<PairingOutput>>(v).ok())
        .unwrap_or_default();
    listed
        .into_iter()
        .filter_map(|o| {
            let format = match o.format.as_str() {
                "opus" => StreamFormat::Opus,
                "mp3" => StreamFormat::Mp3,
                _ => return None,
            };
            Some(OutputRow {
                mount: o.mount,
                format,
                kbps: o.kbps,
            })
        })
        .collect()
}

/// True when the paired settings say the server keeps outputs the app may change.
pub(crate) fn outputs_editable(state: &State) -> bool {
    state
        .settings(SERVER_PART)
        .and_then(|s| s.get(SERVER_OUTPUTS_EDITABLE_KEY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// What the background lane hands back for a save of the outputs.
pub(crate) struct PutAnswer {
    pub(crate) seq: u64,
    pub(crate) address: String,
    pub(crate) result: Result<PairingDocument, PutError>,
}

/// One save of the outputs, off the UI thread: the key stored under `entry`, the entry of
/// the paired address itself, then the request. It never writes the keychain.
pub(crate) fn run_put(
    seq: u64,
    address: String,
    entry: &str,
    outputs: &[OutputSpec],
    store: &Arc<dyn SecretStore + Sync>,
) -> PutAnswer {
    let result = match stored_or(None, Some(entry), store.as_ref()) {
        Ok(key) => put_outputs(&address, key.expose(), outputs),
        Err(PairError::Refused(reason) | PairError::Unreachable(reason)) => {
            Err(PutError::Refused(reason))
        }
        Err(PairError::KeyRefused) => Err(PutError::KeyRefused),
    };
    PutAnswer {
        seq,
        address,
        result,
    }
}

/// What the background lane hands back for a pairing try. The keychain is written only
/// once the source accepted the answer, so an answer that came too late never stores a key.
pub(crate) struct PairAnswer {
    pub(crate) seq: u64,
    pub(crate) address: String,
    pub(crate) result: Result<PairingDocument, PairError>,
    /// The key this try sent, to store once the server took it, when it is new or the push
    /// moved.
    pub(crate) key: Option<Secret>,
    /// True when a person typed the key; else it came from the keychain.
    pub(crate) typed: bool,
}

/// One pairing try, off the UI thread: the key (typed, or read from `paired_entry`, the
/// keychain entry of the paired push), then the fetch. It never writes the keychain.
pub(crate) fn run(
    seq: u64,
    address: String,
    typed: Option<Secret>,
    paired_entry: Option<String>,
    store: &Arc<dyn SecretStore + Sync>,
) -> PairAnswer {
    let was_typed = typed.is_some();
    let (result, key) = match stored_or(typed, paired_entry.as_deref(), store.as_ref()) {
        Ok(key) => (fetch(&address, key.expose()), Some(key)),
        Err(e) => (Err(e), None),
    };
    PairAnswer {
        seq,
        address,
        result,
        key,
        typed: was_typed,
    }
}

/// The typed key, else the one stored under `paired_entry`.
fn stored_or(
    typed: Option<Secret>,
    paired_entry: Option<&str>,
    store: &dyn SecretStore,
) -> Result<Secret, PairError> {
    if let Some(key) = typed {
        return Ok(key);
    }
    let entry = paired_entry.ok_or_else(|| {
        PairError::Refused("enter the station key; the server's admin has it".into())
    })?;
    match store.get(entry) {
        Ok(Some(key)) if !key.is_empty() => Ok(Secret::new(key)),
        Ok(_) => Err(PairError::Refused(
            "the keychain has no key for this server; enter the key again".into(),
        )),
        Err(e) => Err(PairError::Refused(format!(
            "the keychain did not answer: {e}"
        ))),
    }
}

/// Whether the Icecast push may dial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Hold {
    #[default]
    Open,
    /// The start asks the paired server first: a rotated key is found by one request, not
    /// by pushes that count toward the server's lockout.
    Asking,
    /// The paired server refused the key: the push waits for a new one.
    Refused,
}

/// The pairing as the source keeps it.
#[derive(Default)]
pub(crate) struct Pairing {
    /// The server a try is for, or the paired one once a try answered.
    address: String,
    state: PairState,
    /// The last try sent to the background lane. Older answers are dropped.
    asked: u64,
    /// The paired address that the last snapshot saw in the settings.
    seen: Option<String>,
    /// The address this source last paired itself, so it is not asked again when the
    /// settings show it.
    asked_for: Option<String>,
    hold: Hold,
    /// True when the last snapshot saw the push refused, so only a new refusal counts.
    push_refused: bool,
    /// The keychain entries of the key while the station is paired, as last seen.
    seen_entries: Vec<String>,
    /// The entries the key of an accepted pairing is being written to.
    storing: Vec<String>,
    /// Why a write of `storing` failed, once one did.
    store_failed: Option<String>,
    /// The last save of the outputs sent to the background lane. Older answers are dropped.
    saves: u64,
    /// What the last save of the outputs said.
    outputs_state: OutputsState,
    /// The rows of the last save, shown while it is on its way and after it failed.
    outputs_sent: Vec<OutputRow>,
    /// The engine's answer to the `server.pair` of a saved list, and the outputs it writes.
    outputs_reply: Option<OutputsReply>,
}

/// The `server.pair` of a list the server took, on its way to the engine.
struct OutputsReply {
    rx: std::sync::mpsc::Receiver<Result<Outcome, EngineError>>,
    /// The `server.outputs` setting the document writes, to say Saved only once the
    /// settings show it.
    outputs: serde_json::Value,
    /// True once the engine took the command.
    applied: bool,
}

/// The keychain entry of the station key for fetching from the server at `address` (in
/// [`check_address`]'s shape). It is bound to the address in the keychain itself: only a
/// person who types the key writes it, so a server address changed in the settings, by any
/// client, finds no key there and nothing is sent.
pub fn server_entry(address: &str) -> String {
    format!("server:{address}")
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

/// True when the station is paired with a server. Its Icecast push then waits until the
/// app asked the server at start.
pub fn is_paired(state: &State) -> bool {
    paired_address(state).is_some()
}

/// The keychain entry of the paired push, from what the server's document named (the
/// `server.push` setting), so hand edits of `icecast.*` do not move it. A pairing written
/// before that setting existed falls back to the Icecast settings.
pub fn paired_entry(state: &State) -> Option<String> {
    paired_address(state)?;
    let push = state
        .settings(SERVER_PART)
        .and_then(|s| s.get(SERVER_PUSH_KEY));
    let text = |k: &str| push.and_then(|p| p.get(k)).and_then(|v| v.as_str());
    let port = push
        .and_then(|p| p.get("port"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u16::try_from(p).ok());
    match (text("user"), text("host"), port, text("mount")) {
        (Some(user), Some(host), Some(port), Some(mount)) => {
            Some(crate::icecast::mount_entry(user, host, port, mount))
        }
        _ => Some(crate::icecast::secret_key(&crate::icecast_view(state))),
    }
}

impl EngineSource {
    /// Follows the paired address in the settings, whoever changed it (this window, the CLI,
    /// MCP or a script). A paired address that this source did not just pair itself, as at
    /// start, is asked once with the stored key, and the push waits for the answer: the
    /// settings the server hands out overwrite hand edits, and a down server leaves the last
    /// paired settings as they are. The same address is never asked twice by itself, so a
    /// refused key is not tried again. Unpaired by anyone, the state says not paired.
    pub(crate) fn watch_pairing(&mut self) {
        let now = paired_address(&self.state);
        // The entries of the key, while paired, so an unpair by any client removes them.
        if let (Some(push), Some(address)) = (paired_entry(&self.state), now.as_deref()) {
            self.pairing.seen_entries = vec![push, server_entry(address)];
        }
        if now == self.pairing.seen {
            return;
        }
        let before = std::mem::replace(&mut self.pairing.seen, now.clone());
        // A save on its way is for a pairing that changed: its answer writes nothing.
        self.forget_save();
        match now {
            None => {
                if before.is_some() {
                    // Unpaired by another client (the CLI, MCP, a script): the key goes too.
                    for entry in std::mem::take(&mut self.pairing.seen_entries) {
                        self.forget(entry);
                    }
                    self.pairing.storing.clear();
                    self.pairing.asked += 1;
                    self.pairing.address.clear();
                    self.pairing.state = PairState::NotPaired;
                    self.pairing.asked_for = None;
                    self.release_push();
                }
            }
            Some(address) => {
                if self.pairing.asked_for.as_deref() != Some(address.as_str()) {
                    self.pairing.hold = Hold::Asking;
                    self.ask_server(address, None);
                    self.sync_icecast(true);
                }
            }
        }
    }

    /// A push refused by the paired server (HTTP 401), as playout reports it: the key was
    /// rotated while the app ran. The state says key refused and the push stays closed.
    pub(crate) fn watch_push(&mut self, icecast: Option<&radio_stream::StreamStatus>) {
        let refused = matches!(icecast, Some(radio_stream::StreamStatus::Refused { .. }));
        let new = refused && !self.pairing.push_refused;
        self.pairing.push_refused = refused;
        if new && is_paired(&self.state) && self.pairing.hold == Hold::Open {
            // A try on its way is about a key the push just found refused.
            self.pairing.asked += 1;
            if let Some(address) = paired_address(&self.state) {
                self.pairing.address = address;
            }
            self.pairing.state = PairState::KeyRefused;
            self.pairing.hold = Hold::Refused;
        }
    }

    /// Lets the push dial again, with the settings and key there are now.
    fn release_push(&mut self) {
        if self.pairing.hold != Hold::Open {
            self.pairing.hold = Hold::Open;
            self.sync_icecast(true);
        }
    }

    /// Removes the keychain entry `entry` on the background lane; a failure is a notice.
    fn forget(&mut self, entry: String) {
        let sent = self.worker.as_ref().is_some_and(|w| {
            w.jobs
                .send(crate::bg::Job::Forget {
                    key: entry,
                    store: Arc::clone(&self.secrets),
                })
                .is_ok()
        });
        if !sent {
            self.notices.push(
                "The background lane has stopped; the station key stays in the keychain".into(),
            );
        }
    }

    /// A try that cannot be sent: the state says why, and a push waiting for it opens.
    fn not_asked(&mut self, reason: String) {
        self.pairing.state = PairState::Refused(reason);
        self.pairing.asked_for = None;
        if self.pairing.hold == Hold::Asking {
            self.release_push();
        }
    }

    /// Sends a pairing try to the background lane. The stored key goes only to the address
    /// it was paired through, and no key goes to an address outside the rule.
    fn ask_server(&mut self, address: String, typed: Option<Secret>) {
        self.pairing.asked += 1;
        self.pairing.address.clone_from(&address);
        self.pairing.asked_for = Some(address.clone());
        if let Err(e) = check_address(&address) {
            return self.not_asked(format!("the server address is not usable: {e}"));
        }
        // The stored key is read only from the entry of this very address.
        let entry = server_entry(&address);
        let job = crate::bg::Job::Pair {
            seq: self.pairing.asked,
            address,
            key: typed,
            paired_entry: Some(entry),
            store: Arc::clone(&self.secrets),
        };
        let sent = self
            .worker
            .as_ref()
            .is_some_and(|w| w.jobs.send(job).is_ok());
        if sent {
            self.pairing.state = PairState::Pairing;
        } else {
            self.not_asked("the background lane has stopped; restart the app".into());
        }
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
        self.ask_server(address, key);
        Ok(1)
    }

    /// `Edit::Unpair`: the settings pairing wrote go, the key leaves the keychain, and the
    /// Icecast push closes with the settings.
    pub(crate) fn unpair(&mut self) -> Result<usize, String> {
        let (Some(entry), Some(address)) = (paired_entry(&self.state), paired_address(&self.state))
        else {
            return Err("The station is not paired with a server".into());
        };
        if self.worker.is_none() {
            return Err("The background lane could not start; nothing changed".into());
        }
        self.run_edit(Command::ServerUnpair)?;
        self.pairing.seen_entries.clear();
        self.pairing.storing.clear();
        self.forget(entry);
        self.forget(server_entry(&address));
        // A try still on its way is for a pairing that is gone: its answer stores nothing.
        self.pairing.asked += 1;
        self.pairing.address.clear();
        self.pairing.state = PairState::NotPaired;
        self.pairing.asked_for = None;
        self.pairing.hold = Hold::Open;
        self.secret_known = None;
        self.forget_save();
        Ok(1)
    }

    /// The answer of a pairing try from the background lane. Only the answer to the last
    /// try counts; it writes the settings and, then, the keychain.
    pub(crate) fn pair_answer(&mut self, answer: PairAnswer) {
        if answer.seq != self.pairing.asked {
            return;
        }
        self.pairing.address.clone_from(&answer.address);
        // Only a pairing this source wrote is not asked again when the settings show it; after
        // a failed try, another client's pairing of the same server is asked once.
        if answer.result.is_err() {
            self.pairing.asked_for = None;
        }
        match answer.result {
            Ok(document) => self.paired(answer.address, document, answer.key, answer.typed),
            Err(PairError::KeyRefused) => {
                self.pairing.state = PairState::KeyRefused;
                // The paired server refused the stored key: the push would walk its lockout
                // with it, so it waits for a new one.
                if paired_address(&self.state).as_deref() == Some(answer.address.as_str()) {
                    self.pairing.hold = Hold::Refused;
                    self.sync_icecast(true);
                } else if self.pairing.hold == Hold::Asking {
                    self.release_push();
                }
            }
            Err(PairError::Unreachable(reason)) => {
                self.pairing.state = PairState::Unreachable(reason);
                if self.pairing.hold == Hold::Asking {
                    self.release_push();
                }
            }
            Err(PairError::Refused(reason)) => {
                self.pairing.state = PairState::Refused(reason);
                if self.pairing.hold == Hold::Asking {
                    self.release_push();
                }
            }
        }
    }

    /// The server took the key: the settings, then the key under the entry of the push the
    /// document names, where the Icecast output reads it. The key of a push the server moved
    /// away from goes, so there is one copy.
    fn paired(
        &mut self,
        address: String,
        document: PairingDocument,
        key: Option<Secret>,
        typed: bool,
    ) {
        let ice = &document.icecast;
        let entry = crate::icecast::mount_entry(&ice.user, &ice.host, ice.port, &ice.mount);
        let old = paired_entry(&self.state);
        let old_address = paired_address(&self.state);
        let moved = old.as_deref() != Some(entry.as_str());
        let store = Arc::clone(&self.secrets);
        // A typed key is stored for the push and for this address; a stored one moves with the
        // push (it came from this address's own entry).
        let mut writes = Vec::new();
        if let Some(key) = &key {
            if typed {
                writes.push((server_entry(&address), key.clone()));
            }
            if typed || moved {
                writes.push((entry.clone(), key.clone()));
            }
        }
        let mut forgets: Vec<String> = Vec::new();
        if moved && let Some(old) = old {
            forgets.push(old);
        }
        if let Some(old) = old_address.filter(|a| *a != address) {
            forgets.push(server_entry(&old));
        }
        if let Err(e) = self.send("", Command::ServerPair { address, document }) {
            self.pairing.state = PairState::Refused(format!("the settings were not written: {e}"));
            self.release_push();
            return;
        }
        let Some(w) = &self.worker else {
            self.pairing.state =
                PairState::Refused("the background lane has stopped; restart the app".into());
            return;
        };
        let mut sent = true;
        self.pairing.storing.clear();
        self.pairing.store_failed = None;
        for (at, key) in writes {
            self.pairing.storing.push(at.clone());
            sent &= w
                .jobs
                .send(crate::bg::Job::Secret {
                    key: at,
                    set: Some(key.expose().to_owned()),
                    store: Arc::clone(&store),
                })
                .is_ok();
        }
        for old in forgets {
            sent &= w
                .jobs
                .send(crate::bg::Job::Forget {
                    key: old,
                    store: Arc::clone(&store),
                })
                .is_ok();
        }
        // Paired once the keychain took the key (see `stored`), at once when none was to store.
        self.pairing.state = if !sent {
            PairState::Refused("the keychain was not reached; the key is not stored".into())
        } else if self.pairing.storing.is_empty() {
            PairState::Paired
        } else {
            PairState::Pairing
        };
        // The keychain jobs run first on the lane, so the push reads the new key; a rotated
        // key for an unchanged mount applies at once (the same settings keep a working
        // connection).
        self.pairing.hold = Hold::Open;
        self.secret_known = None;
        self.sync_icecast(true);
    }

    /// The keychain's answer to a write: the key of an accepted pairing is stored, or the state
    /// says it is not.
    pub(crate) fn stored(&mut self, entry: &str, result: &Result<bool, String>) {
        let Some(at) = self.pairing.storing.iter().position(|e| e == entry) else {
            return;
        };
        self.pairing.storing.remove(at);
        if let Err(e) = result {
            self.pairing.store_failed = Some(e.clone());
        }
        if self.pairing.storing.is_empty() {
            self.pairing.state = match self.pairing.store_failed.take() {
                None => PairState::Paired,
                Some(e) => PairState::Refused(format!("the keychain did not take the key: {e}")),
            };
        }
    }

    /// Drops a save of the outputs on its way: the outputs show the settings again.
    fn forget_save(&mut self) {
        self.pairing.saves += 1;
        self.pairing.outputs_state = OutputsState::Idle;
        self.pairing.outputs_sent.clear();
        self.pairing.outputs_reply = None;
    }

    /// `Edit::SaveServerOutputs`: sends the whole list to the paired server. It is not
    /// offered while unpaired, while a pairing is asked, while a save is on its way, or when
    /// the server keeps its outputs in its config.
    pub(crate) fn save_outputs(&mut self, rows: Vec<OutputRow>) -> Result<usize, String> {
        let Some(address) = paired_address(&self.state) else {
            return Err("The station is not paired with a server".into());
        };
        if self.pairing.state == PairState::Pairing {
            return Err("The server is being asked; save once it answered".into());
        }
        if self.pairing.outputs_state == OutputsState::Saving {
            return Err("The outputs are on their way to the server; wait for its answer".into());
        }
        if !outputs_editable(&self.state) {
            return Err(OUTPUTS_READ_ONLY.into());
        }
        check_address(&address).map_err(|e| format!("the server address is not usable: {e}"))?;
        self.pairing.saves += 1;
        // The stored key is read only from the entry of this very address.
        let job = crate::bg::Job::PutOutputs {
            seq: self.pairing.saves,
            entry: server_entry(&address),
            address,
            outputs: output_specs(&rows),
            store: Arc::clone(&self.secrets),
        };
        let sent = self
            .worker
            .as_ref()
            .is_some_and(|w| w.jobs.send(job).is_ok());
        if !sent {
            return Err("The background lane has stopped; restart the app".into());
        }
        self.pairing.outputs_reply = None;
        self.pairing.outputs_sent = rows;
        self.pairing.outputs_state = OutputsState::Saving;
        Ok(1)
    }

    /// The server's answer to a save of the outputs. Only the answer to the last save, for
    /// the address still paired, counts; the settings follow the document it hands back.
    pub(crate) fn put_answer(&mut self, answer: PutAnswer) {
        if answer.seq != self.pairing.saves {
            return;
        }
        if paired_address(&self.state).as_deref() != Some(answer.address.as_str()) {
            return self.forget_save();
        }
        let document = match answer.result {
            Ok(document) => document,
            Err(e) => {
                self.pairing.outputs_state = OutputsState::Failed(match e {
                    PutError::KeyRefused => KEY_REFUSED.to_owned(),
                    PutError::Unreachable(reason) => format!("server unreachable: {reason}"),
                    PutError::Refused(reason) => reason,
                });
                return;
            }
        };
        let outputs = serde_json::to_value(&document.outputs).unwrap_or_default();
        let cmd = Command::ServerPair {
            address: answer.address,
            document,
        };
        match self.playout.send(cmd) {
            Ok(rx) => {
                self.pairing.outputs_reply = Some(OutputsReply {
                    rx,
                    outputs,
                    applied: false,
                })
            }
            Err(e) => {
                self.pairing.outputs_state = OutputsState::Failed(format!(
                    "the server took the outputs, but the settings were not written: {e}"
                ));
            }
        }
    }

    /// The engine's answer to the `server.pair` of a saved list, then Saved once the
    /// settings hold what the server answered; or why not.
    pub(crate) fn watch_outputs(&mut self) {
        let Some(reply) = &mut self.pairing.outputs_reply else {
            return;
        };
        if !reply.applied {
            let failed = match reply.rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some("the playout stopped".to_owned())
                }
            };
            if let Some(e) = failed {
                self.pairing.outputs_reply = None;
                self.pairing.outputs_state = OutputsState::Failed(format!(
                    "the server took the outputs, but the settings were not written: {e}"
                ));
                return;
            }
            reply.applied = true;
        }
        let listed = self
            .state
            .settings(SERVER_PART)
            .and_then(|s| s.get(SERVER_OUTPUTS_KEY));
        if listed == Some(&reply.outputs) {
            self.pairing.outputs_reply = None;
            self.pairing.outputs_state = OutputsState::Saved;
        }
    }

    /// True while the push waits: for the start's answer, or for a new key.
    pub(crate) fn push_held(&self) -> bool {
        self.pairing.hold != Hold::Open
    }

    /// True while the paired server refused the key.
    pub(crate) fn key_refused(&self) -> bool {
        self.pairing.hold == Hold::Refused
    }

    /// The pairing for the snapshot. The key is not in it.
    pub(crate) fn pairing_view(&self) -> PairingView {
        let paired = paired_address(&self.state);
        let address = if self.pairing.address.is_empty() {
            paired.clone().unwrap_or_default()
        } else {
            self.pairing.address.clone()
        };
        // Paired is said only while the settings hold that pairing: until the engine took
        // it, the try is still on its way.
        let state = match &self.pairing.state {
            PairState::Paired if paired.as_deref() != Some(address.as_str()) => PairState::Pairing,
            state => state.clone(),
        };
        let outputs_state = self.pairing.outputs_state.clone();
        let outputs = match outputs_state {
            OutputsState::Saving | OutputsState::Failed(_) => self.pairing.outputs_sent.clone(),
            OutputsState::Idle | OutputsState::Saved => output_rows(&self.state),
        };
        PairingView {
            address,
            paired: paired.is_some(),
            state,
            outputs,
            outputs_editable: paired.is_some() && outputs_editable(&self.state),
            outputs_state,
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
