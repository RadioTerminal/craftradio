//! `pair` and `unpair`: the station in a folder pairs with a station server, as the Server
//! page of Preferences does, through the same address rule, fetch and keychain entry.
//!
//! The station key comes from standard input or `CRAFTRADIO_SERVER_KEY`, goes to one request
//! and to the keychain, and is never an argument, printed or written to the settings.

use std::io::{self, BufRead, IsTerminal};
use std::path::Path;

use radio_engine::{Command, Engine};
use radio_ui_engine::{
    KEYCHAIN_SERVICE, KeyringStore, PairError, SecretStore, check_address, fetch_pairing,
    mount_entry, paired_entry, server_entry,
};

/// The environment variable that may hold the station key.
const KEY_ENV: &str = "CRAFTRADIO_SERVER_KEY";
/// Set in tests: the keychain is one in memory, so a test never touches the OS keychain.
/// A release build has no memory keychain and refuses to run with it set.
const TEST_KEYCHAIN_ENV: &str = "CRAFTRADIO_TEST_MEMORY_KEYCHAIN";

/// The keychain of the app, or one in memory in a debug build under test.
fn keychain() -> Result<Box<dyn SecretStore>, String> {
    if std::env::var_os(TEST_KEYCHAIN_ENV).is_some() {
        #[cfg(debug_assertions)]
        return Ok(Box::new(radio_ui_engine::MemoryStore::default()));
        #[cfg(not(debug_assertions))]
        return Err(format!(
            "{TEST_KEYCHAIN_ENV} is for tests of a debug build; unset it to use the keychain"
        ));
    }
    Ok(Box::new(KeyringStore::new(KEYCHAIN_SERVICE)))
}

/// The station key: `CRAFTRADIO_SERVER_KEY`, else one line of standard input. A terminal is
/// refused, because it would show the key as it is typed.
fn station_key() -> Result<String, String> {
    let key = match std::env::var_os(KEY_ENV) {
        Some(key) => key
            .into_string()
            .map_err(|_| format!("{KEY_ENV} holds something that is not text"))?,
        None => {
            let stdin = io::stdin();
            if stdin.is_terminal() {
                return Err(format!(
                    "the station key comes from a file or a pipe, so it is not shown on the \
                     screen: craftradio-cli pair <address> --data-dir <dir> < key.txt, or set \
                     {KEY_ENV}"
                ));
            }
            let mut line = String::new();
            stdin
                .lock()
                .read_line(&mut line)
                .map_err(|e| format!("cannot read the station key from standard input: {e}"))?;
            line
        }
    };
    let key = key.trim();
    if key.is_empty() {
        return Err(format!(
            "no station key: give it on standard input (one line) or in {KEY_ENV}; the \
             server's admin has it"
        ));
    }
    Ok(key.to_owned())
}

/// Why the fetch gave no document, in words. The key is never in them.
fn pair_error(e: PairError) -> String {
    match e {
        PairError::KeyRefused => {
            "key refused: the server does not know this station key; ask its admin for the key"
                .into()
        }
        PairError::Unreachable(reason) => format!("unreachable: {reason}"),
        PairError::Refused(reason) => reason,
    }
}

/// Pairs the station in `db` with the server at `address`.
pub(crate) fn pair(db: &Path, address: &str) -> Result<u8, String> {
    let address = check_address(address)?;
    let store = keychain()?;
    let key = station_key()?;
    let mut engine = Engine::open(db).map_err(|e| e.to_string())?;
    let old = paired_entry(engine.state());
    let old_address = engine
        .state()
        .settings(radio_model::SERVER_PART)
        .and_then(|s| s.get(radio_model::SERVER_ADDRESS_KEY))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let document = fetch_pairing(&address, &key).map_err(pair_error)?;
    let ice = &document.icecast;
    let entry = mount_entry(&ice.user, &ice.host, ice.port, &ice.mount);
    // The key first: a keychain that fails leaves the station as it was. It goes under the
    // push's entry, which the Icecast output reads, and under this address's own entry, the
    // only one a later fetch reads.
    let fetch_entry = server_entry(&address);
    store
        .set(&entry, &key)
        .and_then(|()| store.set(&fetch_entry, &key))
        .map_err(|e| format!("the keychain did not take the key, so nothing changed: {e}"))?;
    let name = document.name.clone();
    let stream = document.stream_url.clone();
    let written = engine
        .dispatch(Command::ServerPair {
            address: address.clone(),
            document,
        })
        .map_err(|e| e.to_string())
        .and_then(|outcome| {
            engine
                .flush()
                .map_err(|e| format!("the pairing is not saved: {e}"))
                .map(|()| outcome)
        });
    let outcome = match written {
        Ok(outcome) => outcome,
        Err(e) => {
            // A key stored for a push that was not paired would be one copy too many.
            if old.as_deref() != Some(entry.as_str()) {
                let _ = store.delete(&entry);
            }
            if old_address.as_deref() != Some(address.as_str()) {
                let _ = store.delete(&fetch_entry);
            }
            return Err(e);
        }
    };
    // The key of a push the server moved away from goes, so there is one copy.
    if let Some(old) = old.filter(|old| *old != entry)
        && let Err(e) = store.delete(&old)
    {
        eprintln!("note: the key of the old push stays in the keychain under {old}: {e}");
    }
    if let Some(old) = old_address
        .filter(|old| *old != address)
        .map(|a| server_entry(&a))
        && let Err(e) = store.delete(&old)
    {
        eprintln!("note: the key of the old server stays in the keychain under {old}: {e}");
    }
    println!("Paired with {address}: {name}, listeners hear it at {stream}");
    println!("The key is in the keychain under this entry: {entry}");
    println!("The app pushes at its next start.");
    if let Some(note) = &outcome.note {
        eprintln!("note: {note}");
    }
    Ok(0)
}

/// Unpairs the station in `db`: the settings pairing wrote go back to their
/// defaults and the key leaves the keychain.
pub(crate) fn unpair(db: &Path) -> Result<u8, String> {
    let store = keychain()?;
    let mut engine = Engine::open(db).map_err(|e| e.to_string())?;
    // Read before the unpair, which removes the push it is derived from.
    let Some(entry) = paired_entry(engine.state()) else {
        return Err("the station is not paired with a server".into());
    };
    let fetch_entry = engine
        .state()
        .settings(radio_model::SERVER_PART)
        .and_then(|s| s.get(radio_model::SERVER_ADDRESS_KEY))
        .and_then(|v| v.as_str())
        .map(server_entry);
    engine
        .dispatch(Command::ServerUnpair)
        .map_err(|e| e.to_string())?;
    engine
        .flush()
        .map_err(|e| format!("the unpairing is not saved: {e}"))?;
    for entry in std::iter::once(entry).chain(fetch_entry) {
        store.delete(&entry).map_err(|e| {
            format!("unpaired, but the key stays in the keychain under {entry}: {e}")
        })?;
    }
    println!("Not paired");
    Ok(0)
}
