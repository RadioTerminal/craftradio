//! `server.pair` and `server.unpair`: the settings a station server's pairing document sets,
//! and taking them away again. The station key is not among them; the app keeps it in the
//! keychain.

use crate::{EngineError, Outcome};
use radio_model::{
    PairingDocument, SERVER_ADDRESS_KEY, SERVER_OUTPUTS_EDITABLE_KEY, SERVER_OUTPUTS_KEY,
    SERVER_PART, SERVER_PLAYER_URL_KEY, SERVER_PUSH_KEY,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

type Settings = BTreeMap<String, BTreeMap<String, Value>>;

/// The settings part of the Icecast push.
const ICECAST_PART: &str = "icecast";
/// The settings part of the WebRTC output.
const WEBRTC_PART: &str = "webrtc";

/// Every setting pairing writes, by part and key. Unpairing removes exactly these.
const PAIRED: [(&str, &str); 15] = [
    (ICECAST_PART, "host"),
    (ICECAST_PART, "port"),
    (ICECAST_PART, "mount"),
    (ICECAST_PART, "user"),
    (ICECAST_PART, "tls"),
    (ICECAST_PART, "public_url"),
    (WEBRTC_PART, "public_stream"),
    (SERVER_PART, SERVER_ADDRESS_KEY),
    (SERVER_PART, "slug"),
    (SERVER_PART, "name"),
    (SERVER_PART, "stream_url"),
    (SERVER_PART, SERVER_OUTPUTS_KEY),
    (SERVER_PART, SERVER_PUSH_KEY),
    (SERVER_PART, SERVER_OUTPUTS_EDITABLE_KEY),
    (SERVER_PART, SERVER_PLAYER_URL_KEY),
];

/// Writes the settings `document` offers. A document of another version changes nothing.
pub(crate) fn pair(
    settings: &mut Settings,
    address: String,
    document: PairingDocument,
) -> Result<Outcome, EngineError> {
    document.check_version().map_err(EngineError::BadArgument)?;
    // The same rule as the Server page: the stored key goes only to this address, so a
    // command from any client cannot point it at a host reached in clear.
    let address = radio_model::check_server_address(&address).map_err(EngineError::BadArgument)?;
    let outputs = serde_json::to_value(&document.outputs)
        .map_err(|e| EngineError::BadArgument(format!("the outputs cannot be stored: {e}")))?;
    let ice = document.icecast;
    // The key is the push password: it never crosses a network in clear.
    if !ice.tls && !radio_model::is_loopback_host(&ice.host) {
        return Err(EngineError::BadArgument(format!(
            "the push to {} has no TLS, which would send the station key in clear; only a \
             push to this computer may go without it",
            ice.host
        )));
    }
    // The push the key is stored for.
    let push = json!({"host": ice.host, "port": ice.port, "mount": ice.mount, "user": ice.user});
    let values = [
        json!(ice.host),
        json!(ice.port),
        json!(ice.mount),
        json!(ice.user),
        json!(ice.tls),
        // The Icecast public address is the one listeners open: the stream, not the server.
        json!(document.stream_url),
        json!(document.stream_url),
        json!(address),
        json!(document.slug),
        json!(document.name),
        json!(document.stream_url),
        outputs,
        push,
        json!(document.outputs_editable),
        json!(document.player_url),
    ];
    for ((part, key), value) in PAIRED.into_iter().zip(values) {
        settings
            .entry(part.to_owned())
            .or_default()
            .insert(key.to_owned(), value);
    }
    Ok(Outcome::default())
}

/// Removes the settings pairing wrote. A station that is not paired keeps all its settings,
/// so an Icecast push typed by hand is never wiped, and the outcome says so.
pub(crate) fn unpair(settings: &mut Settings) -> Outcome {
    let paired = settings
        .get(SERVER_PART)
        .is_some_and(|s| s.contains_key(SERVER_ADDRESS_KEY));
    if !paired {
        return Outcome {
            note: Some("the station is not paired with a server: nothing changed".into()),
            ..Outcome::default()
        };
    }
    for (part, key) in PAIRED {
        if let Some(keys) = settings.get_mut(part) {
            keys.remove(key);
            if keys.is_empty() {
                settings.remove(part);
            }
        }
    }
    Outcome::default()
}
