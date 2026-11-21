//! SQLite persistence behind a writer thread. The dispatch thread never touches the database:
//! it sends state snapshots (written at most once a second) and requests (answered in order).

use crate::{EngineError, EngineEvent, State};
use rusqlite::{Connection, params};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Sends jobs to the writer thread. Jobs run in the order of the calls.
#[derive(Clone)]
pub(crate) struct Writer {
    tx: Sender<Msg>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl Writer {
    pub(crate) fn persistence_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }

    /// Run `f` on the writer thread and wait for its result.
    pub(crate) fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<R, rusqlite::Error> + Send + 'static,
    ) -> Result<R, EngineError> {
        let (rtx, rrx) = mpsc::channel();
        let job: Job = Box::new(move |c| {
            let _ = rtx.send(f(c));
        });
        self.tx.send(Msg::Run(job)).map_err(|_| gone())?;
        rrx.recv().map_err(|_| gone())?.map_err(EngineError::from)
    }

    /// Run `f` on the writer thread, and do not wait for it.
    pub(crate) fn spawn(
        &self,
        f: impl FnOnce(&mut Connection) + Send + 'static,
    ) -> Result<(), EngineError> {
        self.tx.send(Msg::Run(Box::new(f))).map_err(|_| gone())
    }
}

const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

enum Msg {
    Snapshot(Box<State>),
    Run(Job),
    Flush(Sender<()>),
    /// The engine closes: write what is pending, then stop. Log handles may still exist.
    Stop,
}

pub(crate) struct Store {
    tx: Option<Sender<Msg>>,
    handle: Option<JoinHandle<()>>,
    last_error: Arc<Mutex<Option<String>>>,
    subscribers: Arc<Mutex<Vec<Sender<EngineEvent>>>>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS playlists (
    id INTEGER PRIMARY KEY, position INTEGER NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS carts (
    page INTEGER NOT NULL, slot INTEGER NOT NULL, item INTEGER NOT NULL,
    PRIMARY KEY (page, slot));
CREATE TABLE IF NOT EXISTS settings_by_part (
    part TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (part, key));
CREATE TABLE IF NOT EXISTS as_played (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    start_ms INTEGER NOT NULL, end_ms INTEGER,
    playlist TEXT NOT NULL, title TEXT NOT NULL, artist TEXT,
    origin TEXT NOT NULL, rule TEXT,
    status TEXT NOT NULL DEFAULT 'onair');
CREATE INDEX IF NOT EXISTS as_played_start ON as_played (start_ms);
";

pub(crate) fn open_connection(path: Option<&Path>) -> Result<Connection, EngineError> {
    let conn = match path {
        Some(p) => Connection::open(p)?,
        None => Connection::open_in_memory()?,
    };
    // Returns the mode as a row, so it cannot go through `execute`. In memory it stays "memory".
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch(SCHEMA)?;
    // A row still without an end after a restart was cut off by a crash.
    conn.execute(
        "UPDATE as_played SET status = 'unfinished' WHERE end_ms IS NULL",
        [],
    )?;
    Ok(conn)
}

impl Store {
    pub(crate) fn start(conn: Connection) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let last_error = Arc::new(Mutex::new(None));
        let errors = last_error.clone();
        let subscribers = Arc::new(Mutex::new(Vec::new()));
        let events = subscribers.clone();
        let handle = std::thread::spawn(move || writer(conn, rx, errors, events));
        Self {
            tx: Some(tx),
            handle: Some(handle),
            last_error,
            subscribers,
        }
    }

    /// Queue the state for writing; consecutive snapshots within a second collapse into one.
    pub(crate) fn snapshot(&self, state: State) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Snapshot(Box::new(state)));
        }
    }

    /// A handle that sends jobs to the writer thread from any thread.
    pub(crate) fn writer(&self) -> Option<Writer> {
        self.tx.clone().map(|tx| Writer {
            tx,
            last_error: self.last_error.clone(),
        })
    }

    /// Write any pending snapshot now and wait until it is on disk.
    pub(crate) fn flush(&self) -> Result<(), EngineError> {
        let (atx, arx) = mpsc::channel();
        let tx = self.tx.as_ref().ok_or_else(gone)?;
        tx.send(Msg::Flush(atx)).map_err(|_| gone())?;
        arx.recv().map_err(|_| gone())?;
        self.last_error()
            .map_or(Ok(()), |e| Err(EngineError::Store(e)))
    }

    pub(crate) fn subscribe(&self, tx: Sender<EngineEvent>) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(tx);
        }
    }

    pub(crate) fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }
}

fn gone() -> EngineError {
    EngineError::Store("writer thread is gone".into())
}

impl Drop for Store {
    fn drop(&mut self) {
        // The writer writes what is pending, then exits, even while log handles exist.
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Msg::Stop);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn writer(
    mut conn: Connection,
    rx: mpsc::Receiver<Msg>,
    errors: Arc<Mutex<Option<String>>>,
    subscribers: Arc<Mutex<Vec<Sender<EngineEvent>>>>,
) {
    let mut pending: Option<Box<State>> = None;
    let mut last_write = Instant::now()
        .checked_sub(CHECKPOINT_EVERY)
        .unwrap_or_else(Instant::now);
    let write = |conn: &mut Connection, pending: &mut Option<Box<State>>| {
        if let Some(s) = pending.take() {
            let result = write_state(conn, &s).err().map(|e| e.to_string());
            if let Ok(mut g) = errors.lock()
                && *g != result
            {
                *g = result.clone();
                if let Ok(mut subscribers) = subscribers.lock() {
                    subscribers.retain(|s| {
                        s.send(EngineEvent::PersistenceFailed {
                            reason: result.clone(),
                        })
                        .is_ok()
                    });
                }
            }
        }
    };
    loop {
        match rx.recv_timeout(CHECKPOINT_EVERY) {
            Ok(Msg::Snapshot(s)) => pending = Some(s),
            Ok(Msg::Run(job)) => job(&mut conn),
            Ok(Msg::Flush(ack)) => {
                write(&mut conn, &mut pending);
                last_write = Instant::now();
                let _ = ack.send(());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
        if pending.is_some() && last_write.elapsed() >= CHECKPOINT_EVERY {
            write(&mut conn, &mut pending);
            last_write = Instant::now();
        }
    }
    write(&mut conn, &mut pending);
}

fn set(tx: &rusqlite::Transaction, part: &str, key: &str, value: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO settings_by_part (part, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT (part, key) DO UPDATE SET value = excluded.value",
        params![part, key, value],
    )?;
    Ok(())
}

fn json<T: serde::Serialize>(v: &T) -> rusqlite::Result<String> {
    serde_json::to_string(v).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn write_state(conn: &mut Connection, s: &State) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM playlists", [])?;
    for (pos, rec) in s.records().iter().enumerate() {
        tx.execute(
            "INSERT INTO playlists (id, position, json) VALUES (?1, ?2, ?3)",
            params![rec.playlist.id.0 as i64, pos as i64, json(rec)?],
        )?;
    }
    tx.execute("DELETE FROM carts", [])?;
    for c in &s.carts {
        tx.execute(
            "INSERT INTO carts (page, slot, item) VALUES (?1, ?2, ?3)",
            params![c.page, c.slot, c.item.0 as i64],
        )?;
    }
    tx.execute("DELETE FROM settings_by_part WHERE part = 'item_meta'", [])?;
    for (id, meta) in &s.item_meta {
        set(&tx, "item_meta", &id.0.to_string(), &json(meta)?)?;
    }
    set(&tx, "engine", "tz", &json(&s.station.tz)?)?;
    set(&tx, "engine", "mode", &json(&s.mode)?)?;
    set(&tx, "library", "folders", &json(&s.folders)?)?;
    set(&tx, "library", "missing", &json(&s.missing)?)?;
    set(&tx, "library", "hashes", &json(&s.hashes)?)?;
    set(&tx, "alarms", "acked", &json(&s.acked_alarms)?)?;
    for (name, st) in &s.strips {
        set(&tx, "mixer", name, &json(st)?)?;
    }
    if let Some(d) = &s.dynamics {
        set(&tx, "dynamics", crate::DYNAMICS_KEY, &json(d)?)?;
    }
    set(&tx, "dynamics", crate::LOUDNESS_KEY, &json(&s.loudness)?)?;
    set(&tx, "library", "trims", &json(&s.trims)?)?;
    tx.execute("DELETE FROM settings_by_part WHERE part = 'loudness'", [])?;
    for (hash, l) in &s.measured {
        set(&tx, "loudness", hash, &json(l)?)?;
    }
    tx.execute("DELETE FROM settings_by_part WHERE part = 'carts'", [])?;
    for c in &s.cart_settings {
        set(&tx, "carts", &format!("{}:{}", c.page, c.slot), &json(c)?)?;
    }
    for (part, keys) in &s.settings {
        for (key, value) in keys {
            set(
                &tx,
                &format!("{}{part}", crate::UI_PREFIX),
                key,
                &json(value)?,
            )?;
        }
    }
    tx.commit()
}

/// Load the saved state, or `None` for an empty database.
pub(crate) fn load_state(conn: &Connection) -> Result<Option<State>, EngineError> {
    let mut stmt = conn.prepare("SELECT json FROM playlists ORDER BY position")?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if rows.is_empty() {
        return Ok(None);
    }
    let get = |part: &str, key: &str| -> Result<Option<String>, rusqlite::Error> {
        match conn.query_row(
            "SELECT value FROM settings_by_part WHERE part = ?1 AND key = ?2",
            params![part, key],
            |r| r.get::<_, String>(0),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    };
    let bad = |e: serde_json::Error| EngineError::Store(format!("corrupt saved state: {e}"));
    let mut records = Vec::new();
    for r in rows {
        records.push(serde_json::from_str(&r).map_err(bad)?);
    }
    let tz = match get("engine", "tz")? {
        Some(v) => serde_json::from_str(&v).map_err(bad)?,
        None => "UTC".to_string(),
    };
    let mode = match get("engine", "mode")? {
        Some(v) => serde_json::from_str(&v).map_err(bad)?,
        None => Default::default(),
    };
    let folders = match get("library", "folders")? {
        Some(v) => serde_json::from_str(&v).map_err(bad)?,
        None => Default::default(),
    };
    let mut carts = Vec::new();
    let mut stmt = conn.prepare("SELECT page, slot, item FROM carts ORDER BY page, slot")?;
    for c in stmt.query_map([], |r| {
        Ok(crate::CartAssign {
            page: r.get(0)?,
            slot: r.get(1)?,
            item: radio_model::ItemId(r.get::<_, i64>(2)? as u64),
        })
    })? {
        carts.push(c?);
    }
    let mut item_meta = std::collections::BTreeMap::new();
    let mut stmt =
        conn.prepare("SELECT key, value FROM settings_by_part WHERE part = 'item_meta'")?;
    for kv in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (k, v) = kv?;
        let id: u64 = k
            .parse()
            .map_err(|_| EngineError::Store("corrupt item_meta key".into()))?;
        item_meta.insert(
            radio_model::ItemId(id),
            serde_json::from_str(&v).map_err(bad)?,
        );
    }
    let mut settings: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, serde_json::Value>,
    > = std::collections::BTreeMap::new();
    let mut stmt =
        conn.prepare("SELECT part, key, value FROM settings_by_part WHERE part LIKE 'ui:%'")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (part, key, value) = row?;
        let part = part.trim_start_matches(crate::UI_PREFIX).to_owned();
        let value = serde_json::from_str(&value).map_err(bad)?;
        settings.entry(part).or_default().insert(key, value);
    }
    let mut state = State::from_parts(tz, records, mode, carts, folders, item_meta)?;
    state.settings = settings;
    let mut stmt = conn.prepare("SELECT key, value FROM settings_by_part WHERE part = 'mixer'")?;
    for kv in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (k, v) = kv?;
        state
            .strips
            .insert(k, serde_json::from_str(&v).map_err(bad)?);
    }
    // A row that does not parse or is out of range counts as no row: playout keeps its
    // default, and the station still opens.
    state.dynamics = get("dynamics", crate::DYNAMICS_KEY)?
        .and_then(|v| serde_json::from_str::<radio_model::DynamicsSettings>(&v).ok())
        .filter(|d| d.validate().is_ok());
    state.loudness = get("dynamics", crate::LOUDNESS_KEY)?
        .and_then(|v| serde_json::from_str::<radio_model::LoudnessSettings>(&v).ok())
        .filter(|l| l.validate().is_ok())
        .unwrap_or_default();
    // A measurement row that does not parse is dropped: playout measures that file again.
    let mut stmt =
        conn.prepare("SELECT key, value FROM settings_by_part WHERE part = 'loudness'")?;
    for kv in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (hash, v) = kv?;
        if let Ok(l) = serde_json::from_str::<radio_model::Loudness>(&v) {
            state.measured.insert(hash, l);
        }
    }
    let mut stmt = conn.prepare("SELECT value FROM settings_by_part WHERE part = 'carts'")?;
    for v in stmt.query_map([], |r| r.get::<_, String>(0))? {
        state
            .cart_settings
            .push(serde_json::from_str(&v?).map_err(bad)?);
    }
    state.cart_settings.sort_by_key(|c| (c.page, c.slot));
    if let Some(v) = get("library", "missing")? {
        state.missing = serde_json::from_str(&v).map_err(bad)?;
    }
    if let Some(v) = get("library", "hashes")? {
        state.hashes = serde_json::from_str(&v).map_err(bad)?;
    }
    if let Some(v) = get("library", "trims")? {
        state.trims = serde_json::from_str(&v).map_err(bad)?;
    }
    if let Some(v) = get("alarms", "acked")? {
        state.acked_alarms = serde_json::from_str(&v).map_err(bad)?;
    }
    Ok(Some(state))
}
