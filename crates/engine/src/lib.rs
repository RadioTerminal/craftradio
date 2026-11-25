//! The station engine core (L4): state, the command API, persistence and the as-played log.
//!
//! No audio and no planning are wired in yet. Commands that need audio update state and emit
//! [`EngineEvent::AudioIntent`]; their [`Outcome`] says `applied_to_audio: false`.

mod command;
mod error;
mod log;
mod one_off;
mod pairing;
mod store;

pub use command::{
    CartMode, Command, CommandSpec, CompressorPatch, DEFAULT_TOLERANCE, LimiterPatch, Mode, Tier,
    registry,
};
pub use error::EngineError;
pub use log::{AsPlayed, LogEntry, LogStatus, Report, RowId};
pub use one_off::{ONE_OFF_PLAYLIST, OneOff, one_off_playlist};

use jiff::{Timestamp, civil};
use radio_model::{
    DynamicsSettings, Item, ItemId, Loudness, LoudnessSettings, PickOrder, Playlist, PlaylistId,
    Station,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

const DECKS: u8 = 4;
/// The undo history keeps at least this many steps.
const UNDO_STEPS: usize = 100;
/// The settings key of the Program dynamics, in the part `dynamics`.
pub(crate) const DYNAMICS_KEY: &str = "program";
/// The settings key of loudness normalisation, in the part `dynamics`.
pub(crate) const LOUDNESS_KEY: &str = "loudness";

/// The names of the mixer strips.
pub const STRIPS: [&str; 7] = ["A", "B", "C", "D", "Cart", "Live", "Out"];
/// UI parts in `settings_by_part` start with this prefix, so they never meet the parts of the engine.
pub(crate) const UI_PREFIX: &str = "ui:";

/// The settings part of the audio devices.
pub const AUDIO_PART: &str = "audio";
/// The key of the Program device in [`AUDIO_PART`]. Absent or empty is the system default.
pub const OUT_KEY: &str = "out";
/// The key of the monitor device, which plays the SND bus.
pub const MONITOR_KEY: &str = "monitor";
/// The value of [`OUT_KEY`] for no Program device: a system-clock timer drives the mixer.
pub const NO_DEVICE: &str = "none";
/// The old key of the monitor device, from when it played PFL.
pub(crate) const OLD_CUE_KEY: &str = "pfl";

/// The settings of one mixer strip.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(from = "StripRow")]
pub struct StripState {
    pub gain_db: f32,
    pub mute: bool,
    /// The strip plays on the monitor device.
    pub snd: bool,
}

/// A stored strip, in the current or an older form.
#[derive(Deserialize)]
struct StripRow {
    #[serde(default)]
    gain_db: f32,
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    snd: bool,
    #[serde(default)]
    pfl: bool,
    #[serde(default)]
    route: Option<LegacyRoute>,
}

/// The routes that mute and SND replaced.
#[derive(Deserialize)]
enum LegacyRoute {
    Out,
    OutAndCue,
    CueOnly,
}

impl From<StripRow> for StripState {
    fn from(r: StripRow) -> Self {
        let (mute, snd) = match r.route {
            Some(LegacyRoute::CueOnly) => (true, true),
            Some(LegacyRoute::OutAndCue) => (r.mute, true),
            Some(LegacyRoute::Out) | None => (r.mute, r.snd || r.pfl),
        };
        Self {
            gain_db: r.gain_db,
            mute,
            snd: snd || r.pfl,
        }
    }
}

/// The settings of one cart.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CartSettings {
    pub page: u32,
    pub slot: u32,
    pub duck_db: f32,
    pub mode: CartMode,
}

/// One cartwall assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartAssign {
    pub page: u32,
    pub slot: u32,
    pub item: ItemId,
}

/// A playlist as stored: the model playlist plus engine-side data the model has no field for yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlaylistRecord {
    pub(crate) playlist: Playlist,
    #[serde(default)]
    pub(crate) script: Option<String>,
    /// Typed once `FolderSource` lands in the model.
    #[serde(default)]
    pub(crate) source: Option<Value>,
}

/// Everything the engine owns. Serialised by `station.get`.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub station: Station,
    pub(crate) scripts: BTreeMap<PlaylistId, String>,
    pub(crate) sources: BTreeMap<PlaylistId, Value>,
    /// Typed once `ItemMeta` lands in the model.
    pub(crate) item_meta: BTreeMap<ItemId, Value>,
    pub mode: Mode,
    pub stop_after_current: bool,
    pub carts: Vec<CartAssign>,
    pub decks: [Option<ItemId>; DECKS as usize],
    /// True for each deck whose item is on air (started), as playout reports it.
    pub live: [bool; DECKS as usize],
    pub folders: BTreeSet<String>,
    /// Settings of each part of the UI, by part and key (`settings_by_part`).
    pub(crate) settings: BTreeMap<String, BTreeMap<String, Value>>,
    /// The mixer strips, by name.
    pub strips: BTreeMap<String, StripState>,
    /// The compressor and the limiter on Program, or `None` when no one set them. Playout
    /// then keeps its own default.
    pub dynamics: Option<DynamicsSettings>,
    /// Loudness normalisation on the decks.
    pub loudness: LoudnessSettings,
    /// The measured loudness of each file, by content hash (hex). This is the analysis
    /// cache: a file gets one measurement, and a restart keeps it.
    pub measured: BTreeMap<String, Loudness>,
    /// The manual gain trim of items, in dB. Items without a trim have none here.
    pub trims: BTreeMap<ItemId, f32>,
    /// Items that play right after the item on air, in order. Playout takes each one off
    /// when it starts.
    pub queue: Vec<ItemId>,
    /// File items whose file is missing. Playout skips them until they are found again.
    pub missing: BTreeSet<ItemId>,
    /// The content hash (hex) and size of the file of each item, for a relink.
    pub hashes: BTreeMap<ItemId, (String, u64)>,
    /// Cart settings that differ from the defaults.
    pub cart_settings: Vec<CartSettings>,
    /// The ids of the alarms and warnings that a person acknowledged, oldest first. It
    /// keeps at most [`ACKED_MAX`] ids.
    pub acked_alarms: Vec<u64>,
}

/// The acknowledged alarm ids that the engine keeps. Older ids go first.
pub const ACKED_MAX: usize = 2000;

/// What an undo step restores: playlists, scripts, sources, tags and carts.
#[derive(Clone)]
struct Saved {
    station: Station,
    scripts: BTreeMap<PlaylistId, String>,
    sources: BTreeMap<PlaylistId, Value>,
    item_meta: BTreeMap<ItemId, Value>,
    carts: Vec<CartAssign>,
    cart_settings: Vec<CartSettings>,
    trims: BTreeMap<ItemId, f32>,
}

impl State {
    fn fresh() -> Self {
        let mut state = Self::fresh_station();
        // A new station plays nothing locally until someone picks a Program device.
        state
            .settings
            .entry(AUDIO_PART.to_owned())
            .or_default()
            .insert(OUT_KEY.to_owned(), Value::from(NO_DEVICE));
        state
    }

    /// Moves settings of older versions to their current place. It returns whether it
    /// changed anything.
    fn migrate(&mut self) -> bool {
        let Some(audio) = self.settings.get_mut(AUDIO_PART) else {
            return false;
        };
        let Some(old) = audio.remove(OLD_CUE_KEY) else {
            return false;
        };
        audio.entry(MONITOR_KEY.to_owned()).or_insert(old);
        true
    }

    fn fresh_station() -> Self {
        let main = Playlist {
            plays: radio_model::PlayMode::InSequence,
            id: PlaylistId(1),
            name: "Main".into(),
            color: [0x3b, 0x82, 0xf6],
            enabled: true,
            is_main: true,
            is_fill_pool: false,
            role: radio_model::PlaylistRole::User,
            on_interrupt: radio_model::OnInterrupt::Resume,
            priority: 0,
            pick: PickOrder::InOrder,
            items: vec![],
            rules: vec![],
            constraints: vec![],
            source: None,
        };
        Self::with(
            Station {
                tz: "UTC".into(),
                playlists: vec![main],
            },
            BTreeMap::new(),
            BTreeMap::new(),
        )
    }

    fn with(
        station: Station,
        scripts: BTreeMap<PlaylistId, String>,
        sources: BTreeMap<PlaylistId, Value>,
    ) -> Self {
        Self {
            station,
            scripts,
            sources,
            item_meta: BTreeMap::new(),
            mode: Mode::Auto,
            stop_after_current: false,
            carts: vec![],
            decks: [None; DECKS as usize],
            live: [false; DECKS as usize],
            folders: BTreeSet::new(),
            settings: BTreeMap::new(),
            strips: BTreeMap::new(),
            dynamics: None,
            loudness: LoudnessSettings::default(),
            measured: BTreeMap::new(),
            trims: BTreeMap::new(),
            cart_settings: Vec::new(),
            queue: Vec::new(),
            missing: BTreeSet::new(),
            hashes: BTreeMap::new(),
            acked_alarms: Vec::new(),
        }
    }

    pub(crate) fn from_parts(
        tz: String,
        records: Vec<PlaylistRecord>,
        mode: Mode,
        carts: Vec<CartAssign>,
        folders: BTreeSet<String>,
        item_meta: BTreeMap<ItemId, Value>,
    ) -> Result<Self, EngineError> {
        let mut scripts = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut playlists = Vec::new();
        for r in records {
            if let Some(s) = r.script {
                scripts.insert(r.playlist.id, s);
            }
            if let Some(s) = r.source {
                sources.insert(r.playlist.id, s);
            }
            playlists.push(r.playlist);
        }
        let station = Station { tz, playlists };
        station.validate()?;
        let mut state = Self::with(station, scripts, sources);
        state.mode = mode;
        state.carts = carts;
        state.folders = folders;
        state.item_meta = item_meta;
        Ok(state)
    }

    pub(crate) fn records(&self) -> Vec<PlaylistRecord> {
        self.station
            .playlists
            .iter()
            .map(|p| PlaylistRecord {
                playlist: p.clone(),
                script: self.scripts.get(&p.id).cloned(),
                source: self.sources.get(&p.id).cloned(),
            })
            .collect()
    }

    /// Drop meta, cart slots and deck loads that point at items no playlist holds any more.
    fn prune_orphans(&mut self) {
        let live: BTreeSet<ItemId> = self
            .station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| i.id))
            .collect();
        self.item_meta.retain(|i, _| live.contains(i));
        self.carts.retain(|c| live.contains(&c.item));
        self.queue.retain(|i| live.contains(i));
        self.missing.retain(|i| live.contains(i));
        self.hashes.retain(|i, _| live.contains(i));
        self.trims.retain(|i, _| live.contains(i));
        let files: BTreeSet<&String> = self.hashes.values().map(|(h, _)| h).collect();
        self.measured.retain(|h, _| files.contains(h));
        for d in &mut self.decks {
            if d.is_some_and(|i| !live.contains(&i)) {
                *d = None;
            }
        }
    }

    /// The settings of one part of the UI.
    pub fn settings(&self, part: &str) -> Option<&BTreeMap<String, Value>> {
        self.settings.get(part)
    }

    /// The number of decks the station uses. The other decks take no new item.
    pub fn deck_count(&self) -> usize {
        let stored = self
            .settings(radio_model::DECKS_PART)
            .and_then(|s| s.get(radio_model::DECK_COUNT_KEY))
            .and_then(Value::as_u64);
        radio_model::deck_count(stored)
    }

    /// A text setting, trimmed. `None` when it is unset, not text, or blank.
    fn text_setting(&self, part: &str, key: &str) -> Option<String> {
        let text = self.settings(part)?.get(key)?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }

    /// The station name from Preferences, or `None` when it is unset or blank.
    pub fn station_name(&self) -> Option<String> {
        self.text_setting(radio_model::BRANDING_PART, radio_model::BRANDING_NAME_KEY)
    }

    /// The station logo file from Preferences, or `None` when it is unset or blank.
    pub fn station_logo(&self) -> Option<PathBuf> {
        self.text_setting(radio_model::BRANDING_PART, radio_model::BRANDING_LOGO_KEY)
            .map(PathBuf::from)
    }

    /// The settings of all parts of the UI.
    pub fn ui_settings(&self) -> &BTreeMap<String, BTreeMap<String, Value>> {
        &self.settings
    }

    /// The rules script of a playlist, as a person wrote it.
    pub fn script(&self, playlist: PlaylistId) -> Option<&str> {
        self.scripts.get(&playlist).map(String::as_str)
    }

    /// The folder source of a playlist, as `playlist.set_source` stored it.
    pub fn source(&self, playlist: PlaylistId) -> Option<&Value> {
        self.sources.get(&playlist)
    }

    /// The metadata that `item.set_meta` stored for an item.
    pub fn item_meta(&self, item: ItemId) -> Option<&Value> {
        self.item_meta.get(&item)
    }

    /// True when a person acknowledged the alarm or warning `id`.
    pub fn is_acked(&self, id: u64) -> bool {
        self.acked_alarms.contains(&id)
    }

    /// Marks alarm ids as acknowledged. It returns how many were new.
    fn ack(&mut self, ids: impl IntoIterator<Item = u64>) -> usize {
        let mut added = 0;
        for id in ids {
            if !self.acked_alarms.contains(&id) {
                self.acked_alarms.push(id);
                added += 1;
            }
        }
        let over = self.acked_alarms.len().saturating_sub(ACKED_MAX);
        self.acked_alarms.drain(..over);
        added
    }

    /// The measured loudness of the file of an item, from the cache by content hash.
    pub fn loudness_of(&self, item: ItemId) -> Option<Loudness> {
        let (hash, _) = self.hashes.get(&item)?;
        self.measured.get(hash).copied()
    }

    /// The manual gain trim of an item in dB, or 0.
    pub fn trim_of(&self, item: ItemId) -> f32 {
        self.trims.get(&item).copied().unwrap_or(0.0)
    }

    fn has_item(&self, id: ItemId) -> bool {
        self.station
            .playlists
            .iter()
            .any(|p| p.items.iter().any(|i| i.id == id))
    }
}

/// The result of a command.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Outcome {
    /// The command needs the audio engine to take effect.
    pub audio_required: bool,
    /// Always false until the audio glue is wired in: an intent was queued, nothing was played.
    pub applied_to_audio: bool,
    /// Result of a query, or the id a command created.
    pub data: Option<Value>,
    /// Why a part of the command does not reach the audio, when it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What the audio glue should do. Nothing consumes these yet.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioIntent {
    Load {
        deck: u8,
        item: ItemId,
    },
    /// `expect_item` is the item the command meant, or else the item the engine believes
    /// is on the deck. Playout refuses the intent if its deck holds another item.
    Play {
        deck: u8,
        expect_item: Option<ItemId>,
        /// A deck that has not started fades in over this time.
        fade_in: std::time::Duration,
    },
    /// `expect_item` is the item the command meant, or else the item the engine believes
    /// is on the deck. Playout refuses the intent if its deck holds another item.
    Pause {
        deck: u8,
        expect_item: Option<ItemId>,
    },
    /// `expect_item` is the item the command meant, or else the item the engine believes
    /// is on the deck. Playout refuses the intent if its deck holds another item.
    Fade {
        deck: u8,
        expect_item: Option<ItemId>,
        over: Option<std::time::Duration>,
    },
    /// `expect_item` is the item the command meant, or else the item the engine believes
    /// is on the deck. Playout refuses the intent if its deck holds another item.
    Skip {
        deck: u8,
        expect_item: Option<ItemId>,
    },
    /// Move the file on the deck to `to`, counted from its cue-in. `expect_item` is as
    /// for `Pause`.
    Seek {
        deck: u8,
        expect_item: Option<ItemId>,
        to: std::time::Duration,
    },
    SetMode(Mode),
    StopAfterCurrent,
    /// Start the next item now (the operator pressed Next).
    Next,
    FireCart {
        page: u32,
        slot: u32,
    },
    StopAllCarts,
    /// The station plays on after the item on air.
    Continue,
    /// The fader of a strip, with mute applied.
    SetStrip {
        strip: String,
        gain_db: f32,
        mute: bool,
        snd: bool,
    },
    /// The compressor and the limiter on Program.
    SetDynamics(DynamicsSettings),
    /// Loudness normalisation on the decks. Playout sets the item gains again.
    SetLoudness(LoudnessSettings),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// Checkpoint failed, or recovered when reason is None.
    PersistenceFailed {
        reason: Option<String>,
    },
    AudioIntent(AudioIntent),
    /// The station state changed after a command.
    StateChanged,
    /// Only mixer or cart settings changed: no new plan is needed.
    SettingsChanged,
}

pub struct Engine {
    state: State,
    store: store::Store,
    subscribers: Vec<Sender<EngineEvent>>,
    undo: Vec<Saved>,
    redo: Vec<Saved>,
    /// The control of the last undo step and when it came, to merge quick repeats.
    last_step: Option<(String, std::time::Instant)>,
}

/// Repeated edits of one control within this time make one undo step.
const MERGE_STEPS: std::time::Duration = std::time::Duration::from_secs(1);

/// The control that an edit changes, for edits that repeat while a person drags.
fn control_of(cmd: &Command) -> Option<String> {
    match cmd {
        Command::CartSet { page, slot, .. } => Some(format!("cart:{page}:{slot}")),
        _ => None,
    }
}

/// True for a command that Undo can take back.
fn undoable(cmd: &Command) -> bool {
    use Command as C;
    match cmd {
        C::StationBatch { commands } => commands.iter().any(undoable),
        C::PlaylistCreate { .. }
        | C::PlaylistRename { .. }
        | C::PlaylistDelete { .. }
        | C::PlaylistSetMain { .. }
        | C::PlaylistEnable { .. }
        | C::PlaylistSetFillPool { .. }
        | C::PlaylistSetOnInterrupt { .. }
        | C::PlaylistAddItems { .. }
        | C::PlaylistRemoveItems { .. }
        | C::PlaylistClear { .. }
        | C::PlaylistMoveItems { .. }
        | C::PlaylistSetRules { .. }
        | C::PlaylistSetRulesScript { .. }
        | C::PlaylistSetSource { .. }
        | C::PlaylistSetColor { .. }
        | C::PlaylistSetPick { .. }
        | C::PlaylistDuplicate { .. }
        | C::ItemLock { .. }
        | C::ItemUnlock { .. }
        | C::ItemSetMeta { .. }
        | C::ItemSetKeepAlive { .. }
        | C::ItemSetGainTrim { .. }
        | C::CartAssign { .. }
        | C::CartClear { .. }
        | C::CartSet { .. }
        | C::ScheduleAdd { .. }
        | C::ScheduleUpdate { .. }
        | C::ScheduleRemove { .. } => true,
        _ => false,
    }
}

/// True for a command that a batch may hold: a station edit without audio.
fn batchable(cmd: &Command) -> bool {
    !matches!(cmd, Command::StationBatch { .. })
        && (undoable(cmd)
            || matches!(
                cmd,
                Command::SettingsSet { .. }
                    | Command::PlayoutPlayNext { .. }
                    | Command::ItemMarkMissing { .. }
                    | Command::ItemRelink { .. }
                    | Command::ItemSetHash { .. }
            ))
}

impl Engine {
    /// Open (or create) a station database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EngineError> {
        Self::start(store::open_connection(Some(path.as_ref()))?)
    }

    pub fn open_in_memory() -> Result<Self, EngineError> {
        Self::start(store::open_connection(None)?)
    }

    fn start(conn: rusqlite::Connection) -> Result<Self, EngineError> {
        let mut state = store::load_state(&conn)?.unwrap_or_else(State::fresh);
        let migrated = state.migrate();
        let engine = Self {
            state,
            store: store::Store::start(conn),
            subscribers: vec![],
            undo: Vec::new(),
            redo: Vec::new(),
            last_step: None,
        };
        if migrated {
            engine.store.snapshot(engine.state.clone());
        }
        Ok(engine)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// A new event stream. Each call gets its own receiver.
    pub fn events(&mut self) -> Receiver<EngineEvent> {
        let (tx, rx) = mpsc::channel();
        self.store.subscribe(tx.clone());
        self.subscribers.push(tx);
        rx
    }

    /// Write the latest state to disk now and wait for it.
    pub fn flush(&self) -> Result<(), EngineError> {
        self.store.flush()
    }

    /// The last checkpoint write error, if the most recent write failed.
    pub fn persistence_error(&self) -> Option<String> {
        self.store.last_error()
    }

    fn emit(&mut self, ev: EngineEvent) {
        self.subscribers.retain(|s| s.send(ev.clone()).is_ok());
    }

    fn audio(&mut self, intent: AudioIntent) -> Outcome {
        self.emit(EngineEvent::AudioIntent(intent));
        Outcome {
            audio_required: true,
            ..Outcome::default()
        }
    }

    /// Run a command. A failed command changes nothing.
    pub fn dispatch(&mut self, cmd: Command) -> Result<Outcome, EngineError> {
        let query = cmd.tier() == Tier::ReadOnly;
        let control = control_of(&cmd);
        let merged = control.as_ref().is_some_and(|c| {
            self.last_step
                .as_ref()
                .is_some_and(|(k, at)| k == c && at.elapsed() < MERGE_STEPS)
        });
        let is_undoable = undoable(&cmd);
        let before = (is_undoable && !merged).then(|| self.save());
        let system = matches!(cmd, Command::StationSystem { .. });
        // An acknowledgement changes no station data, so it needs no new plan.
        // An acknowledgement or a measurement changes no plan: it needs a save, not a
        // new plan.
        let ack = matches!(
            cmd,
            Command::AlarmAck { .. }
                | Command::AlarmAckAll { .. }
                | Command::ItemSetLoudness { .. }
        );
        // A fader, mute or cart duck changes no station data, so it needs no plan and no
        // full checkpoint: one settings row goes to disk.
        let strip_row = match &cmd {
            Command::MixerSetStrip { strip, .. } => Some(("mixer", strip.clone())),
            Command::MixerSetDynamics { .. } => Some(("dynamics", DYNAMICS_KEY.to_owned())),
            Command::MixerSetLoudness { .. } => Some(("dynamics", LOUDNESS_KEY.to_owned())),
            Command::CartSet { page, slot, .. } => Some(("carts", format!("{page}:{slot}"))),
            _ => None,
        };
        let outcome = self.apply(cmd)?;
        if let Some(before) = before {
            self.undo.push(before);
            if self.undo.len() > UNDO_STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        if is_undoable {
            self.last_step = control.map(|c| (c, std::time::Instant::now()));
        }
        if system {
            self.clear_history();
        }
        if let Some((part, key)) = strip_row {
            self.write_row(part, &key);
            self.emit(EngineEvent::SettingsChanged);
        } else if ack {
            self.store.snapshot(self.state.clone());
            self.emit(EngineEvent::SettingsChanged);
        } else if !query {
            self.store.snapshot(self.state.clone());
            self.emit(EngineEvent::StateChanged);
        }
        Ok(outcome)
    }

    /// Writes one row of the strip or cart settings, without a full checkpoint.
    fn write_row(&self, part: &'static str, key: &str) {
        let value = match part {
            "dynamics" if key == LOUDNESS_KEY => serde_json::to_string(&self.state.loudness).ok(),
            "dynamics" => self
                .state
                .dynamics
                .and_then(|d| serde_json::to_string(&d).ok()),
            "mixer" => self
                .state
                .strips
                .get(key)
                .and_then(|s| serde_json::to_string(s).ok()),
            _ => self
                .state
                .cart_settings
                .iter()
                .find(|c| format!("{}:{}", c.page, c.slot) == key)
                .and_then(|c| serde_json::to_string(c).ok()),
        };
        let (Some(value), Some(writer)) = (value, self.store.writer()) else {
            return;
        };
        let key = key.to_owned();
        let _ = writer.spawn(move |c| {
            let _ = c.execute(
                "INSERT INTO settings_by_part (part, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (part, key) DO UPDATE SET value = excluded.value",
                params![part, key, value],
            );
        });
    }

    /// Forgets the undo and redo steps, for example after the start-up edits of the app.
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.last_step = None;
    }

    /// Refuses a restore that takes away an item that is on air on a deck.
    fn check_decks(&self, saved: &Saved) -> Result<(), EngineError> {
        for (d, item) in self.state.decks.iter().enumerate() {
            let Some(item) = item.as_ref().filter(|_| self.state.live[d]) else {
                continue;
            };
            let kept = saved
                .station
                .playlists
                .iter()
                .any(|p| p.items.iter().any(|i| i.id == *item));
            if !kept {
                let deck = ['A', 'B', 'C', 'D'].get(d).copied().unwrap_or('?');
                return Err(EngineError::BadArgument(format!(
                    "this step would remove the item on air on deck {deck}; skip it first"
                )));
            }
        }
        Ok(())
    }

    fn save(&self) -> Saved {
        let s = &self.state;
        Saved {
            station: s.station.clone(),
            scripts: s.scripts.clone(),
            sources: s.sources.clone(),
            item_meta: s.item_meta.clone(),
            carts: s.carts.clone(),
            cart_settings: s.cart_settings.clone(),
            trims: s.trims.clone(),
        }
    }

    fn restore(&mut self, saved: Saved) {
        let s = &mut self.state;
        s.station = saved.station;
        s.scripts = saved.scripts;
        s.sources = saved.sources;
        s.item_meta = saved.item_meta;
        s.carts = saved.carts;
        s.cart_settings = saved.cart_settings;
        s.trims = saved.trims;
        s.prune_orphans();
    }

    /// Runs the commands of a batch. A failure restores the state before the batch.
    fn batch(&mut self, commands: Vec<Command>) -> Result<Outcome, EngineError> {
        if let Some(bad) = commands.iter().find(|c| !batchable(c)) {
            return Err(EngineError::BadArgument(format!(
                "{} cannot run in a batch",
                bad.id()
            )));
        }
        let before = self.state.clone();
        let mut data = Vec::new();
        for c in commands {
            match self.apply(c) {
                Ok(o) => data.push(o.data.unwrap_or(Value::Null)),
                Err(e) => {
                    self.state = before;
                    return Err(e);
                }
            }
        }
        Ok(Outcome {
            data: Some(Value::Array(data)),
            ..Outcome::default()
        })
    }

    fn deck(&self, deck: u8) -> Result<usize, EngineError> {
        if deck < DECKS {
            Ok(usize::from(deck))
        } else {
            Err(EngineError::UnknownDeck(deck))
        }
    }

    /// Like `loaded`, and if `expect` names an item, the deck must still hold it.
    fn loaded_with(&self, deck: u8, expect: Option<ItemId>) -> Result<(), EngineError> {
        self.loaded(deck)?;
        let found = self.state.decks[usize::from(deck)];
        match expect {
            Some(expected) if found != Some(expected) => Err(EngineError::StaleTarget {
                deck,
                expected,
                found,
            }),
            _ => Ok(()),
        }
    }

    fn loaded(&self, deck: u8) -> Result<(), EngineError> {
        let d = self.deck(deck)?;
        if self.state.decks[d].is_none() {
            return Err(EngineError::BadArgument(format!("deck {deck} is empty")));
        }
        Ok(())
    }

    /// Apply a playlist edit to a copy and keep it only if the station stays valid.
    fn edit_station(
        &mut self,
        f: impl FnOnce(&mut Station) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        let mut next = self.state.station.clone();
        f(&mut next)?;
        next.validate()?;
        self.state.station = next;
        Ok(())
    }

    fn apply(&mut self, cmd: Command) -> Result<Outcome, EngineError> {
        use Command as C;
        match cmd {
            C::PlaylistCreate { name } => {
                let id = PlaylistId(
                    self.state
                        .station
                        .playlists
                        .iter()
                        .map(|p| p.id.0)
                        .max()
                        .unwrap_or(0)
                        + 1,
                );
                self.edit_station(|s| {
                    s.playlists.push(Playlist {
                        plays: radio_model::PlayMode::InSequence,
                        id,
                        name,
                        color: [0x8b, 0x5c, 0xf6],
                        enabled: true,
                        is_main: false,
                        is_fill_pool: false,
                        role: radio_model::PlaylistRole::User,
                        on_interrupt: radio_model::OnInterrupt::Resume,
                        priority: 0,
                        pick: PickOrder::InOrder,
                        items: vec![],
                        rules: vec![],
                        constraints: vec![],
                        source: None,
                    });
                    Ok(())
                })?;
                Ok(Outcome {
                    data: Some(id.0.into()),
                    ..Outcome::default()
                })
            }
            C::PlaylistRename { playlist, name } => {
                self.refuse_system(playlist)?;
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?.name = name;
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistDelete { playlist } => {
                self.refuse_system(playlist)?;
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?;
                    s.playlists.retain(|p| p.id != playlist);
                    Ok(())
                })?;
                self.state.scripts.remove(&playlist);
                self.state.sources.remove(&playlist);
                self.state.prune_orphans();
                Ok(Outcome::default())
            }
            C::PlaylistSetMain { playlist } => {
                self.refuse_system(playlist)?;
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?;
                    for p in &mut s.playlists {
                        p.is_main = p.id == playlist;
                    }
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistEnable { playlist, enabled } => {
                if !enabled {
                    self.refuse_system(playlist)?;
                }
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?.enabled = enabled;
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistSetFillPool {
                playlist,
                fill_pool,
            } => {
                self.refuse_system(playlist)?;
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?.is_fill_pool = fill_pool;
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistSetOnInterrupt {
                playlist,
                on_interrupt,
            } => {
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?.on_interrupt = on_interrupt;
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistAddItems {
                playlist,
                items,
                at,
                lock,
            } => {
                self.refuse_system(playlist)?;
                self.edit_station(|s| {
                    let list = &mut playlist_mut(s, playlist)?.items;
                    let at = at.unwrap_or(list.len());
                    if at > list.len() {
                        return Err(EngineError::BadArgument(format!(
                            "position {at} is past the end ({})",
                            list.len()
                        )));
                    }
                    let locked = items.into_iter().map(|mut i: Item| {
                        i.locked = lock;
                        i
                    });
                    list.splice(at..at, locked);
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistRemoveItems { playlist, items } => {
                self.refuse_system(playlist)?;
                self.edit_station(|s| {
                    let list = &mut playlist_mut(s, playlist)?.items;
                    for id in &items {
                        if !list.iter().any(|i| i.id == *id) {
                            return Err(EngineError::UnknownItem(*id));
                        }
                    }
                    list.retain(|i| !items.contains(&i.id));
                    Ok(())
                })?;
                self.state.prune_orphans();
                Ok(Outcome::default())
            }
            C::PlaylistClear {
                playlist,
                expect_count,
                remove_locked,
            } => {
                self.refuse_system(playlist)?;
                let on_deck: Vec<ItemId> = self.state.decks.iter().flatten().copied().collect();
                let mut removed = 0usize;
                self.edit_station(|s| {
                    let p = playlist_mut(s, playlist)?;
                    if p.items.len() != expect_count {
                        return Err(EngineError::BadArgument(
                            "playlist changed, try again".into(),
                        ));
                    }
                    let before = p.items.len();
                    p.items
                        .retain(|i| on_deck.contains(&i.id) || (i.locked && !remove_locked));
                    removed = before - p.items.len();
                    // A folder playlist stops watching, or its watch would add it all back.
                    if let Some(src) = &mut p.source {
                        src.watch = false;
                    }
                    Ok(())
                })?;
                if let Some(src) = self.state.sources.get_mut(&playlist)
                    && let Some(w) = src.get_mut("watch")
                {
                    *w = Value::Bool(false);
                }
                self.state.prune_orphans();
                Ok(Outcome {
                    data: Some(removed.into()),
                    ..Outcome::default()
                })
            }
            C::PlaylistMoveItems {
                playlist,
                items,
                to,
            } => {
                self.refuse_system(playlist)?;
                self.edit_station(|s| {
                    let list = &mut playlist_mut(s, playlist)?.items;
                    let mut moved = Vec::new();
                    for id in &items {
                        let pos = list
                            .iter()
                            .position(|i| i.id == *id)
                            .ok_or(EngineError::UnknownItem(*id))?;
                        moved.push(list.remove(pos));
                    }
                    if to > list.len() {
                        return Err(EngineError::BadArgument(format!(
                            "position {to} is past the end ({})",
                            list.len()
                        )));
                    }
                    for m in &mut moved {
                        m.locked = true;
                    }
                    list.splice(to..to, moved);
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistSetRulesScript { playlist, script } => {
                let names: Vec<(String, PlaylistId)> = self
                    .state
                    .station
                    .playlists
                    .iter()
                    .map(|p| (p.name.clone(), p.id))
                    .collect();
                if !names.iter().any(|(_, id)| *id == playlist) {
                    return Err(EngineError::UnknownPlaylist(playlist));
                }
                let set =
                    radio_script::eval_rules(&script, &names, &radio_script::Limits::default())?;
                self.apply(C::PlaylistSetRules {
                    playlist,
                    pick: set.pick,
                    rules: set.rules,
                    constraints: set.constraints,
                    script,
                })
            }
            C::PlaylistSetRules {
                playlist,
                pick,
                rules,
                constraints,
                script,
            } => {
                for rule in &rules {
                    let invalid = rule.take == 0
                        || match &rule.trigger {
                            radio_model::Trigger::EveryNTracks(n)
                            | radio_model::Trigger::EveryNMinutes(n) => *n == 0,
                            radio_model::Trigger::AtMinutes(minutes) => {
                                minutes.is_empty() || minutes.iter().any(|m| *m > 59)
                            }
                            _ => false,
                        };
                    if invalid {
                        return Err(EngineError::BadArgument(
                            "Rules need a positive interval and take count, and valid minutes"
                                .into(),
                        ));
                    }
                }
                self.edit_station(|s| {
                    let p = playlist_mut(s, playlist)?;
                    p.pick = pick;
                    p.rules = rules;
                    p.constraints = constraints;
                    Ok(())
                })?;
                self.state.scripts.insert(playlist, script);
                Ok(Outcome::default())
            }
            C::PlaylistSetSource { playlist, source } => {
                playlist_mut(&mut self.state.station, playlist)?;
                self.state.sources.insert(playlist, source);
                Ok(Outcome::default())
            }
            C::ItemLock { item } => self.set_locked(item, true),
            C::ItemUnlock { item } => self.set_locked(item, false),
            C::ItemSetMeta { item, meta } => {
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                // Typed tags also go into the item, so playout titles and logs use them.
                if let Ok(typed) = serde_json::from_value::<radio_model::ItemMeta>(meta.clone()) {
                    self.edit_station(|s| {
                        if let Some(i) = item_mut(s, item) {
                            i.meta = typed;
                        }
                        Ok(())
                    })?;
                }
                self.state.item_meta.insert(item, meta);
                Ok(Outcome::default())
            }
            C::ItemMarkMissing { items, missing } => {
                for item in &items {
                    if !self.state.has_item(*item) {
                        return Err(EngineError::UnknownItem(*item));
                    }
                }
                for item in items {
                    if missing {
                        self.state.missing.insert(item);
                    } else {
                        self.state.missing.remove(&item);
                    }
                }
                Ok(Outcome::default())
            }
            C::ItemRelink { item, path } => {
                self.edit_station(|s| {
                    let i = item_mut(s, item).ok_or(EngineError::UnknownItem(item))?;
                    match &mut i.source {
                        radio_model::Source::File { path: p } => {
                            *p = path;
                            Ok(())
                        }
                        _ => Err(EngineError::BadArgument("only a file item relinks".into())),
                    }
                })?;
                self.state.missing.remove(&item);
                Ok(Outcome::default())
            }
            C::ItemSetHash { item, hash, size } => {
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                self.state.hashes.insert(item, (hash, size));
                Ok(Outcome::default())
            }
            C::ItemSetLoudness {
                item,
                hash,
                size,
                lufs,
                peak_db,
            } => {
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                let finite = lufs.is_finite() && peak_db.is_none_or(f32::is_finite);
                if !finite || hash.is_empty() {
                    return Err(EngineError::BadArgument(
                        "a loudness needs a hash and finite numbers".into(),
                    ));
                }
                self.state.hashes.insert(item, (hash.clone(), size));
                self.state.measured.insert(hash, Loudness { lufs, peak_db });
                Ok(Outcome::default())
            }
            C::ItemSetGainTrim { item, db } => {
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                radio_model::check_trim(db).map_err(EngineError::BadArgument)?;
                if db == 0.0 {
                    self.state.trims.remove(&item);
                } else {
                    self.state.trims.insert(item, db);
                }
                Ok(Outcome::default())
            }
            C::ItemSetKeepAlive { item, keep_alive } => {
                self.edit_station(|s| {
                    let i = item_mut(s, item).ok_or(EngineError::UnknownItem(item))?;
                    match &mut i.source {
                        radio_model::Source::Stream { keep_alive: k, .. } => {
                            *k = keep_alive;
                            Ok(())
                        }
                        _ => Err(EngineError::BadArgument(
                            "only a stream has keep-alive".into(),
                        )),
                    }
                })?;
                Ok(Outcome::default())
            }
            C::DeckLoad { deck, item } => {
                let d = self.deck(deck)?;
                // A deck past the number in use takes a new item only in place of the
                // one it holds.
                if d >= self.state.deck_count() && self.state.decks[d].is_none() {
                    return Err(EngineError::BadArgument(format!(
                        "deck {deck} is not in use (the station uses {} decks)",
                        self.state.deck_count()
                    )));
                }
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                self.state.decks[d] = Some(item);
                Ok(self.audio(AudioIntent::Load { deck, item }))
            }
            C::DeckPlay {
                deck,
                expect_item,
                fade_in,
            } => {
                self.loaded_with(deck, expect_item)?;
                let expect_item = expect_item.or(self.state.decks[usize::from(deck)]);
                Ok(self.audio(AudioIntent::Play {
                    deck,
                    expect_item,
                    fade_in,
                }))
            }
            C::DeckPause { deck, expect_item } => {
                self.loaded_with(deck, expect_item)?;
                let expect_item = expect_item.or(self.state.decks[usize::from(deck)]);
                Ok(self.audio(AudioIntent::Pause { deck, expect_item }))
            }
            C::DeckFade {
                deck,
                expect_item,
                over,
            } => {
                self.loaded_with(deck, expect_item)?;
                let expect_item = expect_item.or(self.state.decks[usize::from(deck)]);
                Ok(self.audio(AudioIntent::Fade {
                    deck,
                    expect_item,
                    over,
                }))
            }
            C::DeckSkip { deck, expect_item } => {
                self.loaded_with(deck, expect_item)?;
                let expect_item = expect_item.or(self.state.decks[usize::from(deck)]);
                Ok(self.audio(AudioIntent::Skip { deck, expect_item }))
            }
            C::DeckSeek {
                deck,
                position: to,
                expect_item,
            } => {
                self.loaded_with(deck, expect_item)?;
                let expect_item = expect_item.or(self.state.decks[usize::from(deck)]);
                Ok(self.audio(AudioIntent::Seek {
                    deck,
                    expect_item,
                    to,
                }))
            }
            C::ModeSet { mode } => {
                self.state.mode = mode;
                self.emit(EngineEvent::AudioIntent(AudioIntent::SetMode(mode)));
                Ok(Outcome::default())
            }
            C::PlayoutNext => Ok(self.audio(AudioIntent::Next)),
            C::PlayoutPlayNext { item } => {
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                self.state.queue.retain(|i| *i != item);
                self.state.queue.insert(0, item);
                Ok(Outcome::default())
            }
            C::StationStopAfterCurrent => {
                self.state.stop_after_current = true;
                self.emit(EngineEvent::AudioIntent(AudioIntent::StopAfterCurrent));
                Ok(Outcome::default())
            }
            C::CartAssign { page, slot, item } => {
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                self.state
                    .carts
                    .retain(|c| !(c.page == page && c.slot == slot));
                self.state.carts.push(CartAssign { page, slot, item });
                self.state.carts.sort_by_key(|c| (c.page, c.slot));
                Ok(Outcome::default())
            }
            C::CartFire { page, slot } => {
                if !self
                    .state
                    .carts
                    .iter()
                    .any(|c| c.page == page && c.slot == slot)
                {
                    return Err(EngineError::BadArgument(format!(
                        "cart page {page} slot {slot} is empty"
                    )));
                }
                Ok(self.audio(AudioIntent::FireCart { page, slot }))
            }
            C::CartStopAll => Ok(self.audio(AudioIntent::StopAllCarts)),
            C::LibraryAddFolder { path } => {
                self.state.folders.insert(path);
                Ok(Outcome::default())
            }
            C::LibraryRemoveFolder { path } => {
                if !self.state.folders.remove(&path) {
                    return Err(EngineError::BadArgument(format!(
                        "{path} is not in the library"
                    )));
                }
                Ok(Outcome::default())
            }
            C::HistoryUndo => {
                let saved = self
                    .undo
                    .last()
                    .ok_or_else(|| EngineError::BadArgument("nothing to undo".into()))?;
                self.check_decks(saved)?;
                let saved = self
                    .undo
                    .pop()
                    .ok_or_else(|| EngineError::BadArgument("nothing to undo".into()))?;
                self.redo.push(self.save());
                self.restore(saved);
                Ok(Outcome::default())
            }
            C::HistoryRedo => {
                let saved = self
                    .redo
                    .last()
                    .ok_or_else(|| EngineError::BadArgument("nothing to redo".into()))?;
                self.check_decks(saved)?;
                let saved = self
                    .redo
                    .pop()
                    .ok_or_else(|| EngineError::BadArgument("nothing to redo".into()))?;
                self.undo.push(self.save());
                self.restore(saved);
                Ok(Outcome::default())
            }
            C::StationBatch { commands } | C::StationSystem { commands } => self.batch(commands),
            C::StationImport { station } => {
                station.validate()?;
                self.state.station = station;
                self.state.scripts.clear();
                self.state.sources.clear();
                self.state.prune_orphans();
                self.undo.clear();
                self.redo.clear();
                Ok(Outcome::default())
            }
            C::StationContinue => {
                self.state.stop_after_current = false;
                self.emit(EngineEvent::AudioIntent(AudioIntent::Continue));
                Ok(Outcome::default())
            }
            C::SettingsSet { part, key, value } => {
                if part.is_empty() {
                    return Err(EngineError::BadArgument(
                        "the settings part is empty".into(),
                    ));
                }
                self.state
                    .settings
                    .entry(part)
                    .or_default()
                    .insert(key, value);
                Ok(Outcome::default())
            }
            C::ServerPair { address, document } => {
                pairing::pair(&mut self.state.settings, address, document)
            }
            C::ServerUnpair => Ok(pairing::unpair(&mut self.state.settings)),
            C::PlaylistSetColor { playlist, color } => {
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?.color = color;
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistSetPick { playlist, pick } => {
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?.pick = pick;
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistDuplicate { playlist, name } => {
                self.refuse_system(playlist)?;
                self.duplicate(playlist, name)
            }
            C::ScheduleAdd {
                one_off,
                start,
                fix,
                tolerance,
                repeat,
            } => self.schedule(
                None,
                one_off::Wish {
                    item: *one_off,
                    start,
                    fix,
                    tolerance,
                    repeat,
                },
            ),
            C::ScheduleUpdate {
                id,
                one_off,
                start,
                fix,
                tolerance,
                repeat,
            } => self.schedule(
                Some(id),
                one_off::Wish {
                    item: *one_off,
                    start,
                    fix,
                    tolerance,
                    repeat,
                },
            ),
            C::ScheduleRemove { id } => self.unschedule(id),
            C::CartClear { page, slot } => {
                let before = self.state.carts.len();
                self.state
                    .carts
                    .retain(|c| !(c.page == page && c.slot == slot));
                if self.state.carts.len() == before {
                    return Err(EngineError::BadArgument(format!(
                        "cart page {page} slot {slot} is empty"
                    )));
                }
                Ok(Outcome::default())
            }
            C::CartSet {
                page,
                slot,
                duck_db,
                mode,
            } => {
                if !duck_db.is_finite() || duck_db > 0.0 {
                    return Err(EngineError::BadArgument(
                        "the duck depth must be 0 dB or lower".into(),
                    ));
                }
                let list = &mut self.state.cart_settings;
                list.retain(|c| !(c.page == page && c.slot == slot));
                list.push(CartSettings {
                    page,
                    slot,
                    duck_db,
                    mode,
                });
                list.sort_by_key(|c| (c.page, c.slot));
                Ok(Outcome {
                    audio_required: true,
                    applied_to_audio: false,
                    data: None,
                    note: Some(
                        "stored; the cart player of this build plays every cart once, without ducking"
                            .into(),
                    ),
                })
            }
            C::MixerSetStrip {
                strip,
                gain_db,
                mute,
                snd,
                route,
            } => {
                if !STRIPS.contains(&strip.as_str()) {
                    return Err(EngineError::BadArgument(format!("no mixer strip {strip}")));
                }
                if route.is_some() {
                    return Err(EngineError::BadArgument(
                        "route is gone: use mute and snd".into(),
                    ));
                }
                if snd == Some(true)
                    && self
                        .state
                        .settings(AUDIO_PART)
                        .and_then(|s| s.get(MONITOR_KEY))
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                {
                    return Err(EngineError::BadArgument(
                        "Monitor output is not available; choose a monitor device first".into(),
                    ));
                }
                // A fader all the way down is a mute; the stored gain stays finite.
                let (gain_db, mute) = match gain_db {
                    Some(g) if g == f32::NEG_INFINITY => (None, Some(true)),
                    other => (other, mute),
                };
                if gain_db.is_some_and(|g| !g.is_finite() || g > 12.0) {
                    return Err(EngineError::BadArgument(
                        "the fader takes a finite value up to +12 dB".into(),
                    ));
                }
                let st = self.state.strips.entry(strip.clone()).or_default();
                if let Some(g) = gain_db {
                    st.gain_db = g;
                }
                if let Some(m) = mute {
                    st.mute = m;
                }
                if let Some(p) = snd {
                    st.snd = p;
                }
                let (gain_db, mute, snd) = (st.gain_db, st.mute, st.snd);
                Ok(self.audio(AudioIntent::SetStrip {
                    strip,
                    gain_db,
                    mute,
                    snd,
                }))
            }
            C::MixerSetDynamics {
                compressor,
                limiter,
            } => {
                // Merge onto the settings in force, check the whole result, then store it.
                let mut next = self.state.dynamics.unwrap_or_default();
                if let Some(c) = compressor {
                    c.apply(&mut next.compressor);
                }
                if let Some(l) = limiter {
                    l.apply(&mut next.limiter);
                }
                next.validate().map_err(EngineError::BadArgument)?;
                self.state.dynamics = Some(next);
                Ok(self.audio(AudioIntent::SetDynamics(next)))
            }
            C::MixerSetLoudness {
                enabled,
                target_lufs,
                max_boost_db,
            } => {
                let mut next = self.state.loudness;
                if let Some(v) = enabled {
                    next.enabled = v;
                }
                if let Some(v) = target_lufs {
                    next.target_lufs = v;
                }
                if let Some(v) = max_boost_db {
                    next.max_boost_db = v;
                }
                next.validate().map_err(EngineError::BadArgument)?;
                self.state.loudness = next;
                Ok(self.audio(AudioIntent::SetLoudness(next)))
            }
            C::AlarmAck { id } => Ok(Outcome {
                data: Some(self.state.ack([id]).into()),
                ..Outcome::default()
            }),
            C::AlarmAckAll { ids } => Ok(Outcome {
                data: Some(self.state.ack(ids).into()),
                ..Outcome::default()
            }),
            C::StationGet => Ok(Outcome {
                data: Some(to_value(&self.state)?),
                ..Outcome::default()
            }),
            C::HistoryList { since, limit } => Ok(Outcome {
                data: Some(to_value(&self.history(since, limit)?)?),
                ..Outcome::default()
            }),
            C::HistoryExportCsv { since, until } => Ok(Outcome {
                data: Some(Value::String(self.export_csv(since, until)?)),
                ..Outcome::default()
            }),
            C::HistoryPlaysCsv {
                since,
                until,
                playlist,
            } => Ok(Outcome {
                data: Some(Value::String(self.log_handle()?.plays_csv(
                    since,
                    until,
                    playlist.as_deref(),
                )?)),
                ..Outcome::default()
            }),
            C::HistoryAiringsCsv {
                since,
                until,
                playlist,
            } => Ok(Outcome {
                data: Some(Value::String(self.log_handle()?.airings_csv(
                    since,
                    until,
                    playlist.as_deref(),
                    &self.state.station.tz,
                )?)),
                ..Outcome::default()
            }),
        }
    }

    fn duplicate(&mut self, playlist: PlaylistId, name: String) -> Result<Outcome, EngineError> {
        let st = &self.state.station;
        let src = st
            .playlists
            .iter()
            .find(|p| p.id == playlist)
            .ok_or(EngineError::UnknownPlaylist(playlist))?
            .clone();
        let id = PlaylistId(st.playlists.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
        let next = st
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| i.id.0))
            .max()
            .unwrap_or(0)
            + 1;
        let mut copy = src;
        copy.id = id;
        copy.name = name;
        copy.is_main = false;
        let mut map = BTreeMap::new();
        for (k, item) in (next..).zip(copy.items.iter_mut()) {
            map.insert(item.id, ItemId(k));
            item.id = ItemId(k);
        }
        if let PickOrder::Weighted { weights } = &mut copy.pick {
            for (i, _) in weights.iter_mut() {
                if let Some(n) = map.get(i) {
                    *i = *n;
                }
            }
        }
        self.edit_station(|s| {
            s.playlists.push(copy);
            Ok(())
        })?;
        if let Some(script) = self.state.scripts.get(&playlist).cloned() {
            self.state.scripts.insert(id, script);
        }
        // The copies play the same files: they keep the trim and the file hash, so the
        // measured loudness applies to them too.
        for (old, new) in &map {
            if let Some(t) = self.state.trims.get(old).copied() {
                self.state.trims.insert(*new, t);
            }
            if let Some(h) = self.state.hashes.get(old).cloned() {
                self.state.hashes.insert(*new, h);
            }
        }
        Ok(Outcome {
            data: Some(id.0.into()),
            ..Outcome::default()
        })
    }

    fn set_locked(&mut self, item: ItemId, locked: bool) -> Result<Outcome, EngineError> {
        let found = self
            .state
            .station
            .playlists
            .iter_mut()
            .flat_map(|p| p.items.iter_mut())
            .find(|i| i.id == item)
            .ok_or(EngineError::UnknownItem(item))?;
        found.locked = locked;
        Ok(Outcome::default())
    }

    // ---- as-played log ----

    /// A handle to the as-played log and the runtime values for another thread. Its
    /// writes go to the writer thread in order, and the `*_later` calls do not wait.
    pub fn log_handle(&self) -> Result<LogHandle, EngineError> {
        Ok(LogHandle {
            writer: self
                .store
                .writer()
                .ok_or_else(|| EngineError::Store("writer thread is gone".into()))?,
        })
    }

    /// Record that an item went to air. Written at once (not on the 1 s checkpoint).
    pub fn log_start(&self, entry: &LogEntry) -> Result<RowId, EngineError> {
        self.log_handle()?.log_start(entry)
    }

    /// Complete a row when its item ends.
    pub fn log_end(
        &self,
        row: RowId,
        end: Timestamp,
        status: LogStatus,
    ) -> Result<(), EngineError> {
        self.log_handle()?.log_end(row, end, status)
    }

    /// Stores a runtime value under `key` (see [`LogHandle::save_runtime`]).
    pub fn save_runtime(&self, key: &str, value: &Value) -> Result<(), EngineError> {
        self.log_handle()?.save_runtime(key, value)
    }

    /// The runtime value under `key`, if a save stored one.
    pub fn runtime(&self, key: &str) -> Result<Option<Value>, EngineError> {
        self.log_handle()?.runtime(key)
    }

    /// Rows starting at or after `since`, oldest first, at most `limit`.
    pub fn history(&self, since: Timestamp, limit: u32) -> Result<Vec<AsPlayed>, EngineError> {
        self.log_handle()?.history(since, limit)
    }

    /// Records the item on a deck, or `None` for an empty deck. Playout calls this for
    /// the decks it loads itself, so `deck.*` commands accept those decks too.
    pub fn set_deck(&mut self, deck: u8, item: Option<ItemId>) {
        if let Ok(d) = self.deck(deck)
            && self.state.decks[d] != item
        {
            self.state.decks[d] = item;
            self.store.snapshot(self.state.clone());
            self.emit(EngineEvent::StateChanged);
        }
    }

    /// Records if the item of a deck is on air. Undo never takes an item on air away.
    pub fn set_live(&mut self, deck: u8, live: bool) {
        if let Ok(d) = self.deck(deck) {
            self.state.live[d] = live;
        }
    }

    /// Takes an item off the play-next queue. Playout calls this when the item starts.
    pub fn dequeue(&mut self, item: ItemId) {
        if self.state.queue.contains(&item) {
            self.state.queue.retain(|i| *i != item);
            self.store.snapshot(self.state.clone());
            self.emit(EngineEvent::StateChanged);
        }
    }

    /// Clears the stop-after-current flag. Playout calls this when the stop takes effect.
    pub fn clear_stop_after_current(&mut self) {
        if self.state.stop_after_current {
            self.state.stop_after_current = false;
            self.store.snapshot(self.state.clone());
            self.emit(EngineEvent::StateChanged);
        }
    }

    /// CSV of the rows that started in `[since, until)`:
    /// `start,end,playlist,title,artist,origin,rule,status`. Times are UTC, RFC 3339.
    pub fn export_csv(&self, since: Timestamp, until: Timestamp) -> Result<String, EngineError> {
        self.log_handle()?.export_csv(since, until)
    }

    /// Counts for one playlist on one civil day in the station's time zone.
    pub fn aired_vs_scheduled(
        &self,
        playlist: PlaylistId,
        day: civil::Date,
    ) -> Result<Report, EngineError> {
        let p = self
            .state
            .station
            .playlists
            .iter()
            .find(|p| p.id == playlist)
            .ok_or(EngineError::UnknownPlaylist(playlist))?;
        self.log_handle()?.aired_vs_scheduled(
            &p.name,
            &self.state.station.tz,
            u32::try_from(p.items.len()).unwrap_or(u32::MAX),
            day,
        )
    }
}

fn item_mut(s: &mut Station, id: ItemId) -> Option<&mut Item> {
    s.playlists
        .iter_mut()
        .flat_map(|p| p.items.iter_mut())
        .find(|i| i.id == id)
}

fn playlist_mut(s: &mut Station, id: PlaylistId) -> Result<&mut Playlist, EngineError> {
    s.playlists
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or(EngineError::UnknownPlaylist(id))
}

fn to_value<T: Serialize>(v: &T) -> Result<Value, EngineError> {
    serde_json::to_value(v).map_err(|e| EngineError::Store(e.to_string()))
}

fn csv_field(f: &str) -> String {
    let safe = if f.trim_start().starts_with(['=', '+', '-', '@']) {
        format!("'{f}")
    } else {
        f.to_owned()
    };
    let f = safe.as_str();
    if f.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_string()
    }
}

/// The as-played log and the runtime values, usable from any thread. Each call runs on
/// the writer thread of the engine, in the order of the calls.
#[derive(Clone)]
pub struct LogHandle {
    writer: store::Writer,
}

fn insert_start(c: &mut rusqlite::Connection, e: &LogEntry) -> rusqlite::Result<RowId> {
    c.execute(
        "INSERT INTO as_played (start_ms, playlist, title, artist, origin, rule)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            e.start.as_millisecond(),
            e.playlist,
            e.title,
            e.artist,
            e.origin,
            e.rule
        ],
    )?;
    Ok(RowId(c.last_insert_rowid()))
}

fn update_end(
    c: &mut rusqlite::Connection,
    row: RowId,
    end: Timestamp,
    status: LogStatus,
) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE as_played SET end_ms = ?1, status = ?2 WHERE id = ?3",
        params![end.as_millisecond(), status.as_str(), row.0],
    )?;
    Ok(())
}

fn upsert_runtime(c: &mut rusqlite::Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO settings_by_part (part, key, value) VALUES ('runtime', ?1, ?2)
         ON CONFLICT (part, key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

impl LogHandle {
    /// The current checkpoint failure, if writes have not recovered.
    pub fn persistence_error(&self) -> Option<String> {
        self.writer.persistence_error()
    }

    /// Record that an item went to air, and wait for the row id.
    pub fn log_start(&self, entry: &LogEntry) -> Result<RowId, EngineError> {
        let e = entry.clone();
        self.writer.call(move |c| insert_start(c, &e))
    }

    /// Complete a row when its item ends, and wait for the write.
    pub fn log_end(
        &self,
        row: RowId,
        end: Timestamp,
        status: LogStatus,
    ) -> Result<(), EngineError> {
        self.writer.call(move |c| update_end(c, row, end, status))
    }

    /// Stores a runtime value under `key`, for example the playout state for a
    /// resume. Station snapshots do not touch it.
    pub fn save_runtime(&self, key: &str, value: &Value) -> Result<(), EngineError> {
        let (key, value) = (key.to_string(), value.to_string());
        self.writer.call(move |c| upsert_runtime(c, &key, &value))
    }

    /// The runtime value under `key`, if a save stored one.
    pub fn runtime(&self, key: &str) -> Result<Option<Value>, EngineError> {
        let key = key.to_string();
        let text: Option<String> = self.writer.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT value FROM settings_by_part WHERE part = 'runtime' AND key = ?1",
            )?;
            let mut rows = stmt.query_map(params![key], |r| r.get(0))?;
            rows.next().transpose()
        })?;
        text.map(|t| serde_json::from_str(&t).map_err(|e| EngineError::Store(e.to_string())))
            .transpose()
    }

    /// Rows starting at or after `since`, oldest first, at most `limit`.
    pub fn history(&self, since: Timestamp, limit: u32) -> Result<Vec<AsPlayed>, EngineError> {
        self.rows(since, Timestamp::MAX, Some(limit))
    }

    /// Rows that started in `[since, until)`, oldest first, at most `limit`.
    pub fn history_between(
        &self,
        since: Timestamp,
        until: Timestamp,
        limit: u32,
    ) -> Result<Vec<AsPlayed>, EngineError> {
        self.rows(since, until, Some(limit))
    }

    /// The plays of the rows that started in `[since, until)`, one line per artist and title:
    /// `artist,title,plays,air_seconds`. A play counts once it started; the air time is what
    /// really aired, so a play that was cut off adds only its part. With `playlist`, only the
    /// plays of that playlist. For royalty reporting.
    pub fn plays_csv(
        &self,
        since: Timestamp,
        until: Timestamp,
        playlist: Option<&str>,
    ) -> Result<String, EngineError> {
        let mut plays: std::collections::BTreeMap<(String, String), (u32, i64)> =
            std::collections::BTreeMap::new();
        for r in self.rows(since, until, None)? {
            if playlist.is_some_and(|p| p != r.entry.playlist) {
                continue;
            }
            let aired = r
                .end
                .map_or(0, |end| end.duration_since(r.entry.start).as_secs().max(0));
            let key = (r.entry.artist.unwrap_or_default(), r.entry.title);
            let sum = plays.entry(key).or_default();
            sum.0 += 1;
            sum.1 += aired;
        }
        let mut out = String::from("artist,title,plays,air_seconds\n");
        for ((artist, title), (count, secs)) in plays {
            let fields = [artist, title, count.to_string(), secs.to_string()];
            let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
            out.push_str(&line.join(","));
            out.push('\n');
        }
        Ok(out)
    }

    /// Every airing that started in `[since, until)`, with its real start and end in the time
    /// zone `zone`: `date,start,end,seconds,playlist,title,artist,rule,status`. With `playlist`,
    /// only the airings of that playlist. For proof of airing.
    pub fn airings_csv(
        &self,
        since: Timestamp,
        until: Timestamp,
        playlist: Option<&str>,
        zone: &str,
    ) -> Result<String, EngineError> {
        let tz =
            jiff::tz::TimeZone::get(zone).map_err(|e| EngineError::BadArgument(e.to_string()))?;
        let clock = |t: Timestamp| t.to_zoned(tz.clone()).strftime("%H:%M:%S").to_string();
        let mut out = String::from("date,start,end,seconds,playlist,title,artist,rule,status\n");
        for r in self.rows(since, until, None)? {
            if playlist.is_some_and(|p| p != r.entry.playlist) {
                continue;
            }
            let start = r.entry.start;
            let secs = r
                .end
                .map(|end| end.duration_since(start).as_secs().max(0).to_string());
            let fields = [
                start.to_zoned(tz.clone()).date().to_string(),
                clock(start),
                r.end.map(clock).unwrap_or_default(),
                secs.unwrap_or_default(),
                r.entry.playlist,
                r.entry.title,
                r.entry.artist.unwrap_or_default(),
                r.entry.rule.unwrap_or_default(),
                r.status.map_or("onair", LogStatus::as_str).to_string(),
            ];
            let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
            out.push_str(&line.join(","));
            out.push('\n');
        }
        Ok(out)
    }

    /// Counts through the store handle. Call from a worker thread.
    pub fn aired_vs_scheduled(
        &self,
        name: &str,
        zone: &str,
        scheduled: u32,
        day: civil::Date,
    ) -> Result<Report, EngineError> {
        let tz =
            jiff::tz::TimeZone::get(zone).map_err(|e| EngineError::BadArgument(e.to_string()))?;
        let bad = |e: jiff::Error| EngineError::BadArgument(e.to_string());
        let from = tz.to_zoned(day.at(0, 0, 0, 0)).map_err(bad)?.timestamp();
        let next = day.tomorrow().map_err(bad)?;
        let to = tz.to_zoned(next.at(0, 0, 0, 0)).map_err(bad)?.timestamp();
        let name = name.to_owned();
        let (aired, interrupted) = self.writer.call(move |c| {
            let n = |status: &str| {
                c.query_row(
                    "SELECT COUNT(*) FROM as_played
                     WHERE playlist = ?1 AND start_ms >= ?2 AND start_ms < ?3 AND status = ?4",
                    params![name, from.as_millisecond(), to.as_millisecond(), status],
                    |r| r.get::<_, u32>(0),
                )
            };
            Ok((n("aired")?, n("interrupted")?))
        })?;
        Ok(Report {
            scheduled,
            aired,
            interrupted,
        })
    }

    /// CSV export through the store handle. Call from a worker thread.
    pub fn export_csv(&self, since: Timestamp, until: Timestamp) -> Result<String, EngineError> {
        let mut out = String::from("start,end,playlist,title,artist,origin,rule,status\n");
        for r in self.rows(since, until, None)? {
            let status = r.status.map_or("onair", LogStatus::as_str);
            let fields = [
                r.entry.start.to_string(),
                r.end.map(|t| t.to_string()).unwrap_or_default(),
                r.entry.playlist,
                r.entry.title,
                r.entry.artist.unwrap_or_default(),
                r.entry.origin,
                r.entry.rule.unwrap_or_default(),
                status.to_string(),
            ];
            let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
            out.push_str(&line.join(","));
            out.push('\n');
        }
        Ok(out)
    }

    /// Like [`LogHandle::log_start`], but returns at once. `done` gets the row id on the
    /// writer thread.
    pub fn log_start_later(
        &self,
        entry: &LogEntry,
        done: impl FnOnce(Result<RowId, EngineError>) + Send + 'static,
    ) {
        let e = entry.clone();
        let _ = self
            .writer
            .spawn(move |c| done(insert_start(c, &e).map_err(EngineError::from)));
    }

    /// Like [`LogHandle::log_end`], but returns at once.
    pub fn log_end_later(&self, row: RowId, end: Timestamp, status: LogStatus) {
        let _ = self.writer.spawn(move |c| {
            let _ = update_end(c, row, end, status);
        });
    }

    /// Like [`LogHandle::save_runtime`], but returns at once.
    pub fn save_runtime_later(&self, key: &str, value: Value) {
        let (key, value) = (key.to_string(), value.to_string());
        let _ = self.writer.spawn(move |c| {
            let _ = upsert_runtime(c, &key, &value);
        });
    }

    pub(crate) fn rows(
        &self,
        since: Timestamp,
        until: Timestamp,
        limit: Option<u32>,
    ) -> Result<Vec<AsPlayed>, EngineError> {
        let (s, u) = (since.as_millisecond(), until.as_millisecond());
        let limit = i64::from(limit.unwrap_or(u32::MAX));
        self.writer.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, start_ms, end_ms, playlist, title, artist, origin, rule, status
                 FROM as_played WHERE start_ms >= ?1 AND start_ms < ?2
                 ORDER BY start_ms, id LIMIT ?3",
            )?;
            let ts = |ms: i64| {
                Timestamp::from_millisecond(ms).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Integer,
                        Box::new(e),
                    )
                })
            };
            let rows = stmt.query_map(params![s, u, limit], |r| {
                let status: String = r.get(8)?;
                let end: Option<i64> = r.get(2)?;
                Ok(AsPlayed {
                    id: RowId(r.get(0)?),
                    entry: LogEntry {
                        start: ts(r.get(1)?)?,
                        playlist: r.get(3)?,
                        title: r.get(4)?,
                        artist: r.get(5)?,
                        origin: r.get(6)?,
                        rule: r.get(7)?,
                    },
                    end: end.map(ts).transpose()?,
                    status: match status.as_str() {
                        "aired" => Some(LogStatus::Aired),
                        "interrupted" => Some(LogStatus::Interrupted),
                        "unfinished" => Some(LogStatus::Unfinished),
                        _ => None,
                    },
                })
            })?;
            rows.collect()
        })
    }
}

/// Evaluate script commands on an I/O worker before dispatch to playout.
pub fn prepare_scripts(
    cmd: Command,
    names: &[(String, PlaylistId)],
) -> Result<Command, EngineError> {
    Ok(match cmd {
        Command::PlaylistSetRulesScript { playlist, script } => {
            let set = radio_script::eval_rules(&script, names, &radio_script::Limits::default())?;
            Command::PlaylistSetRules {
                playlist,
                script,
                pick: set.pick,
                rules: set.rules,
                constraints: set.constraints,
            }
        }
        Command::StationBatch { commands } => Command::StationBatch {
            commands: commands
                .into_iter()
                .map(|c| prepare_scripts(c, names))
                .collect::<Result<_, _>>()?,
        },
        Command::StationSystem { commands } => Command::StationSystem {
            commands: commands
                .into_iter()
                .map(|c| prepare_scripts(c, names))
                .collect::<Result<_, _>>()?,
        },
        other => other,
    })
}
