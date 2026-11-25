//! `pair`, `unpair` and `outputs`: the station in a folder pairs with a station server, and
//! sets the outputs it keeps there, as the Server page of Preferences does, through the same
//! address rule, requests and keychain entries.
//!
//! The station key comes from standard input or `CRAFTRADIO_SERVER_KEY`, goes to one request
//! and to the keychain, and is never an argument, printed or written to the settings.

use std::io::{self, BufRead, IsTerminal};
use std::path::Path;

use radio_engine::{Command, Engine};
use radio_model::{OutputSpec, PairingOutput, SERVER_ADDRESS_KEY, SERVER_OUTPUTS_KEY, SERVER_PART};
use radio_ui_engine::{
    KEYCHAIN_SERVICE, KeyringStore, PairError, PutError, SecretStore, check_address, fetch_pairing,
    mount_entry, paired_entry, put_outputs, server_entry,
};

/// The environment variable that may hold the station key.
const KEY_ENV: &str = "CRAFTRADIO_SERVER_KEY";
/// Set in tests: the keychain is one in memory, so a test never touches the OS keychain.
/// A release build has no memory keychain and refuses to run with it set.
const TEST_KEYCHAIN_ENV: &str = "CRAFTRADIO_TEST_MEMORY_KEYCHAIN";
/// Set in tests: the keychain is this JSON file, so one run of the CLI finds what an earlier
/// run stored. As [`TEST_KEYCHAIN_ENV`], a release build refuses to run with it set.
const TEST_KEYCHAIN_FILE_ENV: &str = "CRAFTRADIO_TEST_KEYCHAIN_FILE";

/// The keychain of the app, or a test one (a file, else in memory) in a debug build.
fn keychain() -> Result<Box<dyn SecretStore>, String> {
    if let Some(path) = std::env::var_os(TEST_KEYCHAIN_FILE_ENV) {
        #[cfg(debug_assertions)]
        return Ok(Box::new(FileStore(path.into())));
        #[cfg(not(debug_assertions))]
        {
            let _ = path;
            return Err(format!(
                "{TEST_KEYCHAIN_FILE_ENV} is for tests of a debug build; unset it to use the \
                 keychain"
            ));
        }
    }
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

/// A keychain for tests in a JSON file of entries, kept between runs of the CLI.
#[cfg(debug_assertions)]
struct FileStore(std::path::PathBuf);

#[cfg(debug_assertions)]
impl FileStore {
    fn read(&self) -> Result<std::collections::BTreeMap<String, String>, String> {
        match std::fs::read_to_string(&self.0) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(&self, entries: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
        let text = serde_json::to_string(entries).map_err(|e| e.to_string())?;
        std::fs::write(&self.0, text).map_err(|e| e.to_string())
    }
}

#[cfg(debug_assertions)]
impl SecretStore for FileStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.read()?.get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        let mut entries = self.read()?;
        entries.insert(key.to_owned(), secret.to_owned());
        self.write(&entries)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let mut entries = self.read()?;
        entries.remove(key);
        self.write(&entries)
    }
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

/// The paired server's address in the settings of `engine`, or why there is none.
fn paired_server(engine: &Engine) -> Result<String, String> {
    engine
        .state()
        .settings(SERVER_PART)
        .and_then(|s| s.get(SERVER_ADDRESS_KEY))
        .and_then(|v| v.as_str())
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            "the station is not paired with a server; pair it first: craftradio-cli pair \
             <address> --data-dir <dir>"
                .into()
        })
}

/// Prints the outputs of `outputs`, one per line: `<mount> <format> <kbps> <url>`.
fn print_outputs(outputs: &[PairingOutput]) -> Result<(), String> {
    use std::io::Write as _;
    let mut out = io::stdout().lock();
    for o in outputs {
        writeln!(out, "{} {} {} {}", o.mount, o.format, o.kbps, o.url)
            .map_err(|e| format!("cannot write the outputs: {e}"))?;
    }
    Ok(())
}

/// Prints the outputs of the station in `db` on its server, as the settings hold them.
pub(crate) fn list_outputs(db: &Path) -> Result<u8, String> {
    let engine = Engine::open(db).map_err(|e| e.to_string())?;
    paired_server(&engine)?;
    let outputs: Vec<PairingOutput> = engine
        .state()
        .settings(SERVER_PART)
        .and_then(|s| s.get(SERVER_OUTPUTS_KEY))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("the station's outputs setting cannot be read: {e}"))?
        .unwrap_or_default();
    if outputs.is_empty() {
        eprintln!("note: the server offers no re-encoded outputs of this station");
    }
    print_outputs(&outputs)?;
    Ok(0)
}

/// One output as typed: `<mount>=<format>:<kbps>`.
fn output_spec(typed: &str) -> Result<OutputSpec, String> {
    let shape = || format!("{typed:?} is not <mount>=<format>:<kbps>, such as low.opus=opus:32");
    let (mount, rest) = typed.split_once('=').ok_or_else(shape)?;
    let (format, kbps) = rest.split_once(':').ok_or_else(shape)?;
    if mount.is_empty() {
        return Err(shape());
    }
    let format = format.to_ascii_lowercase();
    if format != "opus" && format != "mp3" {
        return Err(format!(
            "{typed:?}: the format is opus or mp3, not {format:?}"
        ));
    }
    let kbps = kbps
        .parse()
        .map_err(|_| format!("{typed:?}: the bitrate is a whole number of kbps"))?;
    Ok(OutputSpec {
        mount: mount.to_owned(),
        format,
        kbps,
    })
}

/// Sends `typed`, the whole new list of outputs, to the server the station in `db` is
/// paired with, and writes the settings from the document it answers with.
pub(crate) fn set_outputs(db: &Path, typed: &[String]) -> Result<u8, String> {
    let outputs = typed
        .iter()
        .map(|t| output_spec(t))
        .collect::<Result<Vec<_>, _>>()?;
    let mut engine = Engine::open(db).map_err(|e| e.to_string())?;
    let address = paired_server(&engine)?;
    let store = keychain()?;
    // The key only from the entry of this very address, which only a typed key writes.
    let key = match store.get(&server_entry(&address)) {
        Ok(Some(key)) if !key.is_empty() => key,
        Ok(_) => {
            return Err(format!(
                "the keychain has no key for {address}; pair again: craftradio-cli pair \
                 {address} --data-dir <dir>"
            ));
        }
        Err(e) => return Err(format!("the keychain did not answer: {e}")),
    };
    let document = put_outputs(&address, &key, &outputs).map_err(|e| match e {
        PutError::KeyRefused => {
            "key refused: the server does not know this station key; pair again with the new \
             key"
            .to_owned()
        }
        PutError::Unreachable(reason) => format!("unreachable: {reason}"),
        PutError::Refused(reason) => reason,
    })?;
    let listed = document.outputs.clone();
    engine
        .dispatch(Command::ServerPair { address, document })
        .map_err(|e| {
            format!("the server took the outputs, but the settings were not written: {e}")
        })?;
    engine
        .flush()
        .map_err(|e| format!("the server took the outputs, but the settings are not saved: {e}"))?;
    print_outputs(&listed)?;
    Ok(0)
}
