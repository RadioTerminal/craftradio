//! The station engine core (L4): state, the command API, persistence and the as-played log.
//!
//! No audio and no planning are wired in yet. Commands that need audio update state and emit
//! [`EngineEvent::AudioIntent`]; their [`Outcome`] says `applied_to_audio: false`.

mod command;
mod error;
mod log;
mod store;

pub use command::{Command, CommandSpec, Mode, Tier, registry};
pub use error::EngineError;
pub use log::{AsPlayed, LogEntry, LogStatus, Report, RowId};

use jiff::{Timestamp, civil};
use radio_model::{Item, ItemId, PickOrder, Playlist, PlaylistId, Station};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
};

const DECKS: u8 = 4;

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
    pub folders: BTreeSet<String>,
}

impl State {
    fn fresh() -> Self {
        let main = Playlist {
            id: PlaylistId(1),
            name: "Main".into(),
            color: [0x3b, 0x82, 0xf6],
            enabled: true,
            is_main: true,
            is_fill_pool: false,
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
            folders: BTreeSet::new(),
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
        for d in &mut self.decks {
            if d.is_some_and(|i| !live.contains(&i)) {
                *d = None;
            }
        }
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
}

/// What the audio glue should do. Nothing consumes these yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioIntent {
    Load {
        deck: u8,
        item: ItemId,
    },
    Play {
        deck: u8,
    },
    Pause {
        deck: u8,
    },
    Fade {
        deck: u8,
    },
    Skip {
        deck: u8,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    AudioIntent(AudioIntent),
    /// The station state changed after a command.
    StateChanged,
}

pub struct Engine {
    state: State,
    store: store::Store,
    subscribers: Vec<Sender<EngineEvent>>,
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
        let state = store::load_state(&conn)?.unwrap_or_else(State::fresh);
        Ok(Self {
            state,
            store: store::Store::start(conn),
            subscribers: vec![],
        })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// A new event stream. Each call gets its own receiver.
    pub fn events(&mut self) -> Receiver<EngineEvent> {
        let (tx, rx) = mpsc::channel();
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
        let outcome = self.apply(cmd)?;
        if !query {
            self.store.snapshot(self.state.clone());
            self.emit(EngineEvent::StateChanged);
        }
        Ok(outcome)
    }

    fn deck(&self, deck: u8) -> Result<usize, EngineError> {
        if deck < DECKS {
            Ok(usize::from(deck))
        } else {
            Err(EngineError::UnknownDeck(deck))
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
                        id,
                        name,
                        color: [0x8b, 0x5c, 0xf6],
                        enabled: true,
                        is_main: false,
                        is_fill_pool: false,
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
                self.edit_station(|s| {
                    playlist_mut(s, playlist)?.name = name;
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistDelete { playlist } => {
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
            } => {
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
                        i.locked = true;
                        i
                    });
                    list.splice(at..at, locked);
                    Ok(())
                })?;
                Ok(Outcome::default())
            }
            C::PlaylistRemoveItems { playlist, items } => {
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
            C::PlaylistMoveItems {
                playlist,
                items,
                to,
            } => {
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
                self.edit_station(|s| {
                    let p = playlist_mut(s, playlist)?;
                    p.pick = set.pick;
                    p.rules = set.rules;
                    p.constraints = set.constraints;
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
                self.state.item_meta.insert(item, meta);
                Ok(Outcome::default())
            }
            C::DeckLoad { deck, item } => {
                let d = self.deck(deck)?;
                if !self.state.has_item(item) {
                    return Err(EngineError::UnknownItem(item));
                }
                self.state.decks[d] = Some(item);
                Ok(self.audio(AudioIntent::Load { deck, item }))
            }
            C::DeckPlay { deck } => {
                self.loaded(deck)?;
                Ok(self.audio(AudioIntent::Play { deck }))
            }
            C::DeckPause { deck } => {
                self.loaded(deck)?;
                Ok(self.audio(AudioIntent::Pause { deck }))
            }
            C::DeckFade { deck } => {
                self.loaded(deck)?;
                Ok(self.audio(AudioIntent::Fade { deck }))
            }
            C::DeckSkip { deck } => {
                self.loaded(deck)?;
                Ok(self.audio(AudioIntent::Skip { deck }))
            }
            C::ModeSet { mode } => {
                self.state.mode = mode;
                self.emit(EngineEvent::AudioIntent(AudioIntent::SetMode(mode)));
                Ok(Outcome::default())
            }
            C::PlayoutNext => Ok(self.audio(AudioIntent::Next)),
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
        }
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
        let mut out = String::from("start,end,playlist,title,artist,origin,rule,status\n");
        for r in self.log_handle()?.rows(since, until, None)? {
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
        let tz = jiff::tz::TimeZone::get(&self.state.station.tz)
            .map_err(|e| EngineError::BadArgument(e.to_string()))?;
        let bad = |e: jiff::Error| EngineError::BadArgument(e.to_string());
        let from = tz.to_zoned(day.at(0, 0, 0, 0)).map_err(bad)?.timestamp();
        let next = day.tomorrow().map_err(bad)?;
        let to = tz.to_zoned(next.at(0, 0, 0, 0)).map_err(bad)?.timestamp();
        let name = p.name.clone();
        let scheduled = u32::try_from(p.items.len()).unwrap_or(u32::MAX);
        let (aired, interrupted) = self.store.call(move |c| {
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
