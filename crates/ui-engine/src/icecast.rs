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
pub trait SecretStore: Send + Sync {
    /// The secret under `key`, or `None` when the store has none.
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    /// Stores `secret` under `key`.
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
    /// Removes the secret under `key`. A store that has none there is fine.
    fn delete(&self, key: &str) -> Result<(), String>;
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

    fn delete(&self, key: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(&self.service, key).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
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

    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(key);
        Ok(())
    }
}

/// The keychain entry of a mount.
pub(crate) fn secret_key(ice: &IcecastView) -> String {
    mount_entry(&ice.user, &ice.host, ice.port, &ice.mount)
}

/// The keychain entry of the mount `user@host:port/mount`.
pub fn mount_entry(user: &str, host: &str, port: u16, mount: &str) -> String {
    format!("icecast:{user}@{host}:{port}{mount}")
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
        keychain_error: None,
        // TLS is on for any host but this computer, unless a person turned it off.
        tls: s
            .and_then(|m| m.get("tls"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_else(|| radio_stream::default_tls(&text("host"))),
        format: if text("format") == "mp3" {
            StreamFormat::Mp3
        } else {
            StreamFormat::Opus
        },
        kbps: num("kbps")
            .and_then(|k| u32::try_from(k).ok())
            .unwrap_or(DEFAULT_KBPS),
        public_url: text("public_url"),
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
    // The keychain account always comes from the mount itself. A stored reference could
    // point a changed host at the password of another mount.
    let key = secret_key(&ice);
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
        tls: ice.tls,
    }))
}

/// The stream name Icecast gets: the station name setting, else `fallback`, as at start.
fn station_name(state: &State, fallback: &str) -> String {
    state.station_name().unwrap_or_else(|| fallback.to_owned())
}

/// What the Icecast settings of playout are built from, without the password.
pub(crate) fn icecast_seen(state: &State, fallback: &str) -> (IcecastView, String) {
    (icecast_view(state), station_name(state, fallback))
}

impl EngineSource {
    /// The Icecast mount for the snapshot. The background lane asks the keychain once per
    /// mount; until it answers, the password shows as not set.
    pub(crate) fn icecast(&mut self) -> IcecastView {
        let mut ice = icecast_view(&self.state);
        let key = secret_key(&ice);
        if self.secret_known.as_ref().is_none_or(|(k, _)| *k != key) {
            self.secret_known = Some((key.clone(), None));
            if !ice.host.is_empty()
                && let Some(w) = &self.worker
            {
                let _ = w.jobs.send(crate::bg::Job::Secret {
                    key,
                    set: None,
                    store: std::sync::Arc::clone(&self.secrets),
                });
            }
        }
        match self.secret_known.as_ref().and_then(|(_, r)| r.as_ref()) {
            Some(Ok(set)) => ice.password_set = *set,
            Some(Err(e)) => ice.keychain_error = Some(e.clone()),
            None => {}
        }
        ice
    }

    /// An answer of the keychain from the background lane.
    pub(crate) fn secret_answer(&mut self, key: String, wrote: bool, result: Result<bool, String>) {
        if wrote && let Err(e) = &result {
            self.notices
                .push(format!("The keychain did not take the password: {e}"));
        }
        // A new password applies at once, as new settings do.
        let stored = wrote && result.is_ok();
        if self.secret_known.as_ref().is_some_and(|(k, _)| *k == key) {
            self.secret_known = Some((key, Some(result)));
        }
        if stored {
            self.sync_icecast(true);
        }
    }

    /// Asks the background lane for the Icecast settings of playout when the settings or the
    /// station name changed since playout last got them, or always with `force` (after a
    /// keychain write). The keychain read stays off the UI thread.
    pub(crate) fn sync_icecast(&mut self, force: bool) {
        let seen = icecast_seen(&self.state, &self.station);
        if !force && seen == self.icecast_seen {
            return;
        }
        self.icecast_seen = seen;
        let Some(w) = &self.worker else {
            self.notices.push(
                "The background lane has stopped; Icecast settings apply at the next start".into(),
            );
            return;
        };
        self.icecast_asked += 1;
        let _ = w.jobs.send(crate::bg::Job::Icecast {
            seq: self.icecast_asked,
            state: std::sync::Arc::clone(&self.state),
            store: std::sync::Arc::clone(&self.secrets),
            station_name: station_name(&self.state, &self.station),
        });
    }

    /// The Icecast settings from the background lane go to playout. Without a password the
    /// source closes, so a changed host never keeps the connection of the old mount.
    pub(crate) fn icecast_answer(
        &mut self,
        seq: u64,
        result: Result<Option<IcecastConfig>, String>,
    ) {
        if seq != self.icecast_asked {
            return;
        }
        let cfg = match result {
            // The paired server refused the key: the push waits for a new one.
            Ok(Some(_)) if self.key_refused() => {
                self.notices.push(
                    "Icecast stays off: the server refused the station key; pair again with \
                     the new key"
                        .into(),
                );
                None
            }
            // The start asks the paired server first; the push follows its answer.
            Ok(Some(_)) if self.push_held() => None,
            Ok(cfg) => cfg,
            Err(e) => {
                self.notices.push(format!("Icecast stays off: {e}"));
                None
            }
        };
        if self.playout.set_icecast(cfg).is_err() {
            self.notices
                .push("The playout has stopped; the Icecast settings were not applied".into());
        }
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
        let public_url = ice.public_url.trim();
        if !public_url.is_empty() {
            radio_ui_egui::view::check_stream_url(public_url)
                .map_err(|e| format!("The public address: {e}"))?;
        }
        let clean = IcecastView {
            host: host.to_owned(),
            mount: mount.to_owned(),
            user: ice.user.trim().to_owned(),
            public_url: public_url.to_owned(),
            ..ice
        };
        let key = secret_key(&clean);
        // The background lane writes the keychain; a failure comes back as a notice.
        if !clean.password.is_empty() {
            let w = self
                .worker
                .as_ref()
                .ok_or("The background lane could not start")?;
            w.jobs
                .send(crate::bg::Job::Secret {
                    key: key.clone(),
                    set: Some(clean.password.clone()),
                    store: std::sync::Arc::clone(&self.secrets),
                })
                .map_err(|_| "The background lane has stopped".to_owned())?;
            self.secret_known = Some((key, None));
        }
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
            set("tls", clean.tls.into()),
            set("public_url", clean.public_url.clone().into()),
        ])?;
        Ok(1)
    }
}
