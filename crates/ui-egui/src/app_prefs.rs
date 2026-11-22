//! The preferences of this computer, kept in `config.toml` with craft-settings, as every Craft
//! app keeps them: how the clock, the History, the Alarms and the Library show, and the size of
//! each tool window. Station settings (decks, mixer, Icecast, cartwall) stay in the station.
//!
//! The view source still holds every setting. A display preference is laid over the view's
//! settings at each snapshot, and saving one goes here instead of to the source, so the windows
//! read and write them as before.

use std::collections::BTreeMap;
use std::time::Instant;

use craft_settings::{Location, SaveStatus, Settings, SettingsStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::parts;
use crate::view::StationView;

/// The parts whose every setting is a display preference of this computer.
const DISPLAY_PARTS: [&str; 4] = [parts::CLOCK, parts::HISTORY, parts::ALARMS, parts::LIBRARY];

/// True when `part`/`key` is a preference of this computer, not a station setting.
pub(crate) fn is_display(part: &str, key: &str) -> bool {
    DISPLAY_PARTS.contains(&part) || key == crate::windows::SIZE_KEY
}

/// What `config.toml` holds: one `[part]` section per part, then the import marker.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Prefs {
    /// Once the display settings that the station kept are copied here, this is true.
    pub imported_from_station: bool,
    /// The display settings, by part and key.
    #[serde(flatten)]
    pub parts: BTreeMap<String, BTreeMap<String, Value>>,
}

impl Settings for Prefs {
    const APP_ID: &'static str = "craftradio";
    const SCHEMA_VERSION: u32 = 1;
}

/// The store of this computer's preferences.
pub struct AppPrefs {
    store: SettingsStore<Prefs>,
    /// The last save error, said once.
    failed: Option<String>,
}

impl AppPrefs {
    /// Preferences in memory only: nothing is read or written (tests and the preview).
    pub fn memory() -> Self {
        Self {
            store: SettingsStore::open(Location::Memory).0,
            failed: None,
        }
    }

    /// The preferences in `dir`, as a `--config-dir` flag would choose.
    pub fn open_in(dir: std::path::PathBuf) -> Self {
        Self {
            store: SettingsStore::open(Location::Dir(dir)).0,
            failed: None,
        }
    }

    /// The preferences in the platform's config folder, or where `CRAFTRADIO_CONFIG_DIR`
    /// points. It never fails; the text says what went wrong while reading, if anything.
    pub fn open() -> (Self, Option<String>) {
        let (store, report) = SettingsStore::open(Location::Auto);
        let problem = report.has_problems().then(|| {
            let where_ = report
                .dir
                .as_ref()
                .map_or_else(String::new, |d| format!(" in {}", d.display()));
            format!("Some preferences{where_} could not be read, and use their defaults")
        });
        (
            Self {
                store,
                failed: None,
            },
            problem,
        )
    }

    /// Lays the preferences over the settings of `view`.
    pub(crate) fn overlay(&self, view: &mut StationView) {
        for (part, keys) in &self.store.get().parts {
            let to = view.settings.entry(part.clone()).or_default();
            for (key, value) in keys {
                to.insert(key.clone(), value.clone());
            }
        }
    }

    /// Copies the display settings that the station kept, once. The station keeps them too.
    pub(crate) fn import(&mut self, view: &StationView) {
        if self.store.get().imported_from_station {
            return;
        }
        self.store.update(|p| {
            for (part, keys) in &view.settings {
                for (key, value) in keys {
                    if is_display(part, key) && !value.is_null() {
                        p.parts
                            .entry(part.clone())
                            .or_default()
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
                    }
                }
            }
            p.imported_from_station = true;
        });
    }

    /// Keeps `value` for `part`/`key`.
    pub(crate) fn set(&mut self, part: &str, key: &str, value: Value) {
        self.store.update(|p| {
            p.parts
                .entry(part.to_owned())
                .or_default()
                .insert(key.to_owned(), value);
        });
    }

    /// Saves after the debounce. It returns a failure to tell, once, and asks for a repaint at
    /// the next deadline so an idle window still saves.
    pub(crate) fn tick(&mut self, ctx: &egui::Context) -> Option<String> {
        let now = Instant::now();
        let tick = self.store.tick(now);
        if let Some(deadline) = tick.next_deadline {
            ctx.request_repaint_after(deadline.saturating_duration_since(now));
        }
        match tick.status {
            SaveStatus::Failed(e) => {
                let text = format!("Preferences are not saved: {e}");
                let new = self.failed.as_deref() != Some(text.as_str());
                self.failed = Some(text.clone());
                new.then_some(text)
            }
            SaveStatus::Saved => {
                self.failed = None;
                None
            }
            SaveStatus::Idle | SaveStatus::Pending => None,
        }
    }

    /// Saves what is pending, as the app closes.
    pub(crate) fn flush(&mut self) {
        let _ = self.store.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::ViewSource;
    use serde_json::json;

    fn view() -> StationView {
        let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
        let now = "2026-11-21T13:58:00Z".parse().expect("timestamp");
        crate::MockSource::new(tz).snapshot(now)
    }

    fn setting(view: &StationView, part: &str, key: &str) -> Option<Value> {
        view.settings.get(part)?.get(key).cloned()
    }

    #[test]
    fn how_a_window_shows_is_this_computers_and_the_station_keeps_its_own() {
        assert!(is_display(parts::CLOCK, "h24"));
        assert!(is_display(parts::HISTORY, "hours_back"));
        assert!(is_display(parts::CARTWALL, crate::windows::SIZE_KEY));
        assert!(
            !is_display(parts::CARTWALL, "cols"),
            "the cartwall grid is the station's"
        );
        assert!(
            !is_display(parts::DECKS, "A_fade_s"),
            "deck fades are the station's"
        );
        assert!(!is_display("icecast", "host"));
    }

    #[test]
    fn a_preference_comes_back_after_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut prefs = AppPrefs::open_in(dir.path().to_path_buf());
        prefs.set(parts::CLOCK, "h24", json!(false));
        prefs.flush();
        drop(prefs);
        let again = AppPrefs::open_in(dir.path().to_path_buf());
        let mut v = view();
        again.overlay(&mut v);
        assert_eq!(setting(&v, parts::CLOCK, "h24"), Some(json!(false)));
    }

    #[test]
    fn the_stations_display_settings_are_copied_once_and_its_own_stay() {
        let mut prefs = AppPrefs::memory();
        let mut station = view();
        station
            .settings
            .entry(parts::CLOCK.to_owned())
            .or_default()
            .insert("h24".to_owned(), json!(false));
        station
            .settings
            .entry(parts::CARTWALL.to_owned())
            .or_default()
            .insert("cols".to_owned(), json!(6));
        prefs.import(&station);
        let parts_kept = &prefs.store.get().parts;
        assert_eq!(parts_kept[parts::CLOCK]["h24"], json!(false));
        assert!(
            !parts_kept.contains_key(parts::CARTWALL),
            "a station setting is not copied"
        );
        // A later change in the station is not copied over the preference.
        station
            .settings
            .get_mut(parts::CLOCK)
            .expect("clock")
            .insert("h24".to_owned(), json!(true));
        prefs.import(&station);
        assert_eq!(prefs.store.get().parts[parts::CLOCK]["h24"], json!(false));
    }
}
