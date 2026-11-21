//! The Icecast settings of the station. The password lives in the OS keychain.
//!
//! The settings keep only a reference to the keychain entry. A snapshot never carries the
//! password; it says only whether one is set.

use std::collections::HashMap;
use std::sync::Mutex;

use radio_engine::{Command, State};
use radio_stream::{IcecastConfig, StreamFormat as SFormat};
use radio_ui_egui::view::{IcecastView, StreamFormat};

use crate::EngineSource;

/// The part of `settings_by_part` for the Icecast mount.
pub const ICECAST_PART: &str = "icecast";
/// The keychain service of the app.
pub const KEYCHAIN_SERVICE: &str = "radio-station-manager";
const DEFAULT_PORT: u16 = 8000;
const DEFAULT_KBPS: u32 = 128;

/// A store for secrets, such as the OS keychain.
pub trait SecretStore: Send {
    /// The secret under `key`, or `None` when the store has none.
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    /// Stores `secret` under `key`.
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
}

/// The OS keychain, through the `keyring` crate.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(&self.service, key).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(&self.service, key).map_err(|e| e.to_string())?;
        entry.set_password(secret).map_err(|e| e.to_string())
    }
}

/// Secrets in memory only, for tests and the preview.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }
}

/// The keychain entry of a mount.
fn secret_key(ice: &IcecastView) -> String {
    format!(
        "icecast:{}@{}:{}{}",
        ice.user, ice.host, ice.port, ice.mount
    )
}

/// The Icecast mount from the settings of the station, without the password.
pub fn icecast_view(state: &State) -> IcecastView {
    let s = state.settings(ICECAST_PART);
    let text = |k: &str| {
        s.and_then(|m| m.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let num = |k: &str| s.and_then(|m| m.get(k)).and_then(serde_json::Value::as_u64);
    IcecastView {
        host: text("host"),
        port: num("port")
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(DEFAULT_PORT),
        mount: text("mount"),
        user: match text("user") {
            u if u.is_empty() => "source".to_owned(),
            u => u,
        },
        password: String::new(),
        password_set: false,
        format: if text("format") == "mp3" {
            StreamFormat::Mp3
        } else {
            StreamFormat::Opus
        },
        kbps: num("kbps")
            .and_then(|k| u32::try_from(k).ok())
            .unwrap_or(DEFAULT_KBPS),
    }
}

/// The Icecast output for playout, when the settings name a mount and the keychain has
/// its password. `Ok(None)` means that no mount is set up.
pub fn icecast_config(
    state: &State,
    secrets: &dyn SecretStore,
    station_name: &str,
) -> Result<Option<IcecastConfig>, String> {
    let ice = icecast_view(state);
    if ice.host.is_empty() || ice.mount.is_empty() {
        return Ok(None);
    }
    let key = state
        .settings(ICECAST_PART)
        .and_then(|m| m.get("password_ref"))
        .and_then(|v| v.as_str())
        .map_or_else(|| secret_key(&ice), str::to_owned);
    let password = secrets
        .get(&key)?
        .ok_or_else(|| format!("the keychain has no password for {}{}", ice.host, ice.mount))?;
    Ok(Some(IcecastConfig {
        host: ice.host,
        port: ice.port,
        mount: ice.mount,
        user: ice.user,
        password,
        name: station_name.to_owned(),
        description: String::new(),
        genre: String::new(),
        format: match ice.format {
            StreamFormat::Opus => SFormat::Opus { kbps: ice.kbps },
            StreamFormat::Mp3 => SFormat::Mp3 { kbps: ice.kbps },
        },
        tls: false,
    }))
}

impl EngineSource {
    /// The Icecast mount for the snapshot. The keychain answers once per mount.
    pub(crate) fn icecast(&mut self) -> IcecastView {
        let mut ice = icecast_view(&self.state);
        let key = secret_key(&ice);
        if self.secret_known.as_ref().is_none_or(|(k, _)| *k != key) {
            let set = !ice.host.is_empty()
                && self
                    .secrets
                    .get(&key)
                    .is_ok_and(|p| p.is_some_and(|p| !p.is_empty()));
            self.secret_known = Some((key, set));
        }
        ice.password_set = self.secret_known.as_ref().is_some_and(|(_, s)| *s);
        ice
    }

    pub(crate) fn set_icecast(&mut self, ice: IcecastView) -> Result<usize, String> {
        let host = ice.host.trim();
        if host.contains(['/', '@', ' ']) {
            return Err("The host is a name or an address only, such as stream.example.org".into());
        }
        let mount = ice.mount.trim();
        if !host.is_empty() && !mount.starts_with('/') {
            return Err("The mount starts with /, such as /live".into());
        }
        if ice.format == StreamFormat::Mp3 && !radio_stream::MP3 {
            return Err("This build cannot encode MP3; pick Opus".into());
        }
        if ice.port == 0 {
            return Err("The port must be over 0".into());
        }
        let clean = IcecastView {
            host: host.to_owned(),
            mount: mount.to_owned(),
            user: ice.user.trim().to_owned(),
            ..ice
        };
        let key = secret_key(&clean);
        if !clean.password.is_empty() {
            self.secrets
                .set(&key, &clean.password)
                .map_err(|e| format!("The keychain did not take the password: {e}"))?;
        }
        self.secret_known = None;
        let set = |k: &str, v: serde_json::Value| Command::SettingsSet {
            part: ICECAST_PART.into(),
            key: k.into(),
            value: v,
        };
        let format = match clean.format {
            StreamFormat::Opus => "opus",
            StreamFormat::Mp3 => "mp3",
        };
        self.run_batch(vec![
            set("host", clean.host.clone().into()),
            set("port", clean.port.into()),
            set("mount", clean.mount.clone().into()),
            set("user", clean.user.clone().into()),
            set("format", format.into()),
            set("kbps", clean.kbps.into()),
            set("password_ref", key.into()),
        ])?;
        Ok(1)
    }
}
