//! Checks the OS keychain backend: `keychain set KEY VALUE`, `keychain get KEY`, `keychain delete KEY`.
//! It uses a test service name, so it never touches the entries of the app.

use radio_ui_engine::{KeyringStore, SecretStore};

const SERVICE: &str = "radio-station-manager-keychain-check";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let store = KeyringStore::new(SERVICE);
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["set", key, value] => store.set(key, value).map(|()| "stored".to_owned()),
        ["get", key] => store
            .get(key)
            .map(|v| v.map_or_else(|| "none".to_owned(), |v| format!("found {} chars", v.len()))),
        ["delete", key] => keyring::Entry::new(SERVICE, key)
            .and_then(|e| e.delete_credential())
            .map(|()| "deleted".to_owned())
            .map_err(|e| e.to_string()),
        _ => Err("usage: keychain set KEY VALUE | get KEY | delete KEY".to_owned()),
    };
    match result {
        Ok(text) => {
            println!("{text}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("keychain: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
