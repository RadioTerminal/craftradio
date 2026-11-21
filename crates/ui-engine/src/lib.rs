//! The real station behind the desktop UI: a `ViewSource` over the engine and the playout.

mod bg;
mod edit;

pub use edit::{AUDIO_PART, file_item};
mod fader;
mod folders;
mod icecast;

pub use icecast::{
    ICECAST_PART, KEYCHAIN_SERVICE, KeyringStore, MemoryStore, SecretStore, icecast_config,
    icecast_view,
};
mod map;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, Zoned};
use radio_audio::Strip;
use radio_engine::{LogStatus, Mode, State};
use radio_model::{Item, ItemId, PlaylistId, Source};
use radio_playout::{DeckPhase, LocalOutput, Playout, PlayoutView};
use radio_scheduler::Origin as SOrigin;
use radio_stream::StreamStatus;
use radio_ui_egui::view::{
    CartMode, CartSlot, CartwallView, DeckState, DeckView, DeviceRoles, DropTarget, Edit, FitView,
    FolderSource, Imported, LibraryItem, MeterView, Origin, OutputStatus, PlayMode, PlaylistTab,
    Route, Row, StationView, Status, StreamState, ViewSource,
};

use map::{display_title, effective_meta, item_len, view_meta};

/// The history pane shows the as-played rows of the last three hours.
const HISTORY_HOURS: i64 = 3;
/// The history pane shows at most this many rows.
const HISTORY_ROWS: u32 = 300;
/// The source reads the as-played log again after this time, or after a deck change.
const HISTORY_EVERY: SignedDuration = SignedDuration::from_secs(5);
/// The number of carts on one page when the cartwall settings give no grid.
const CARTS_PER_PAGE: usize = 16;
/// Row ids of the as-played log have this bit, so they never meet timeline row ids.
const HISTORY_BIT: u64 = 1 << 62;
/// A repeat of an item in the timeline keeps its occurrence number above this shift.
const REPEAT_SHIFT: u32 = 40;
/// The part of `settings_by_part` that holds the cartwall settings.
pub const CARTWALL_PART: &str = "cartwall";

/// Settings for an [`EngineSource`].
#[derive(Clone, Debug)]
pub struct SourceOptions {
    /// The station time zone, for the clock and the row times.
    pub tz: TimeZone,
    /// The stream title of items that hide their own title.
    pub station_name: String,
}

/// The real station as a [`ViewSource`].
pub struct EngineSource {
    /// The fader thread holds a sender of the playout, so it drops first.
    merger: Option<fader::Merger>,
    /// The answers to merged commands, checked for errors.
    replies: fader::Replies,
    /// The meter reader of the mixer, for the meters of every frame. A read never blocks.
    meter_reader: std::sync::Mutex<radio_audio::MeterReader>,
    playout: Playout,
    tz: TimeZone,
    station: String,
    devices: Vec<String>,
    /// The state revision of the caches below.
    revision: Option<u64>,
    state: Arc<State>,
    items: HashMap<ItemId, (PlaylistId, Item)>,
    tabs: Vec<PlaylistTab>,
    main: Option<PlaylistTab>,
    library: Vec<LibraryItem>,
    /// What each row id of the last snapshot stands for.
    rows: HashMap<u64, RowRef>,
    /// The row ids of the last snapshot, in order.
    row_order: Vec<u64>,
    /// The stream title before each row of the last snapshot.
    previous: HashMap<u64, String>,
    /// The items on the decks in the last snapshot. Deck commands expect these items.
    decks: [Option<ItemId>; 4],
    /// The states of the decks in the last snapshot.
    deck_states: [DeckState; 4],
    /// The rows on air in the last snapshot.
    on_air_rows: HashSet<u64>,
    /// The fixed rows of the last snapshot.
    fixed_rows: HashSet<u64>,
    history: Vec<Row>,
    history_at: Option<(Timestamp, [Option<ItemId>; 4])>,
    /// The next item id that the source gives out, or 0 to start after the largest id.
    next_id: u64,
    /// Library files that no playlist holds.
    files: Vec<folders::LibFile>,
    next_file: u64,
    limits: radio_library::Limits,
    worker: Option<bg::Worker>,
    /// Commands on their way to the engine.
    pending: Vec<edit::Pending>,
    /// Messages for the UI: results and errors of work that ran later.
    notices: Vec<String>,
    /// What to do with the result of each probe on the background lane.
    probes: HashMap<u64, edit::ProbeAfter>,
    next_req: u64,
    folders: Vec<folders::Folder>,
    /// The watches of folder playlists, by playlist.
    watches: HashMap<PlaylistId, (radio_model::FolderSource, radio_library::Watcher)>,
    /// Watch problems, for the warnings.
    watch_errors: Vec<String>,
    secrets: Arc<dyn SecretStore + Sync>,
    /// The keychain entry of the mount and whether it holds a password.
    secret_known: Option<(String, Option<Result<bool, String>>)>,
    /// Fader values on their way to the engine, by strip.
    strip_gain: HashMap<String, f32>,
    /// Part settings on their way to the engine.
    pending_settings: HashMap<(String, String), serde_json::Value>,
}

/// What a timeline row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowRef {
    pub(crate) playlist: PlaylistId,
    pub(crate) item: ItemId,
}

impl EngineSource {
    /// A source over a running playout.
    pub fn new(playout: Playout, opts: SourceOptions) -> Self {
        let devices = radio_audio_device::list_outputs()
            .into_iter()
            .map(|d| d.name)
            .collect();
        let state = playout.snapshot().state;
        let replies: fader::Replies = std::sync::Arc::default();
        let pending = std::sync::Arc::clone(&replies);
        let merger = playout.command_sender().and_then(|s| {
            fader::Merger::start(move |key, c| {
                if let Ok(rx) = s.send_with_reply(c)
                    && let Ok(mut list) = pending.lock()
                {
                    list.push((key, rx));
                }
            })
        });
        let meter_reader = std::sync::Mutex::new(playout.meters());
        Self {
            merger,
            replies,
            meter_reader,
            playout,
            tz: opts.tz,
            station: opts.station_name,
            devices,
            revision: None,
            state,
            items: HashMap::new(),
            tabs: Vec::new(),
            main: None,
            library: Vec::new(),
            rows: HashMap::new(),
            row_order: Vec::new(),
            previous: HashMap::new(),
            decks: [None; 4],
            deck_states: [DeckState::Empty; 4],
            on_air_rows: HashSet::new(),
            fixed_rows: HashSet::new(),
            history: Vec::new(),
            history_at: None,
            next_id: 0,
            files: Vec::new(),
            next_file: 0,
            limits: radio_library::Limits::DEFAULT,
            worker: bg::Worker::start(),
            pending: Vec::new(),
            notices: Vec::new(),
            probes: HashMap::new(),
            next_req: 0,
            folders: Vec::new(),
            watches: HashMap::new(),
            watch_errors: Vec::new(),
            secrets: Arc::new(MemoryStore::default()),
            secret_known: None,
            strip_gain: HashMap::new(),
            pending_settings: HashMap::new(),
        }
    }

    /// The same source with another store for the Icecast password.
    pub fn with_secrets(mut self, secrets: Arc<dyn SecretStore + Sync>) -> Self {
        self.secrets = secrets;
        self.secret_known = None;
        self
    }

    /// The same source with other limits for folder reads.
    pub fn with_limits(mut self, limits: radio_library::Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The playout under this source, for a headless run or a test.
    pub fn playout(&self) -> &Playout {
        &self.playout
    }

    /// Rebuilds the caches that depend on the engine state.
    fn rebuild(&mut self, state: &Arc<State>) {
        self.state = Arc::clone(state);
        let station = &state.station;
        self.items = station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| (i.id, (p.id, i.clone()))))
            .collect();
        self.tabs = station
            .playlists
            .iter()
            .filter(|p| !p.is_main)
            .map(|p| map::tab(p, state))
            .collect();
        self.main = station
            .playlists
            .iter()
            .find(|p| p.is_main)
            .map(|p| map::tab(p, state));
        self.library = self.library_items();
        self.sync_folders();
        self.sync_watches();
    }

    /// The media library: each file and stream of each playlist, once per source.
    fn library_items(&self) -> Vec<LibraryItem> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for p in &self.state.station.playlists {
            for item in &p.items {
                let path = source_text(&item.source);
                if !seen.insert(path.clone()) {
                    continue;
                }
                let meta = effective_meta(item, self.state.item_meta(item.id));
                let missing = self.state.missing.contains(&item.id);
                out.push(LibraryItem {
                    id: item.id.0,
                    title: if missing {
                        format!("⚠ missing · {}", item.title)
                    } else {
                        item.title.clone()
                    },
                    artist: item.artist.clone(),
                    len: item_len(item),
                    category: p.name.clone(),
                    path,
                    meta: view_meta(item, &meta),
                });
            }
        }
        out
    }

    fn zoned(&self, t: Timestamp) -> Zoned {
        t.to_zoned(self.tz.clone())
    }

    fn playlist_look(&self, id: PlaylistId) -> (String, [u8; 3], bool) {
        self.state
            .station
            .playlists
            .iter()
            .find(|p| p.id == id)
            .map_or_else(
                || (String::new(), [0x80; 3], false),
                |p| (p.name.clone(), p.color, p.is_main),
            )
    }

    fn row_for(&self, id: u64, item: &Item, playlist: PlaylistId, start: Timestamp) -> Row {
        let (name, color, is_main) = self.playlist_look(playlist);
        let meta = effective_meta(item, self.state.item_meta(item.id));
        Row {
            id,
            start: self.zoned(start),
            playlist: name,
            color,
            title: display_title(item),
            len: item_len(item),
            is_main,
            fixed: None,
            origin: Origin::Main,
            locked: item.locked,
            meta: view_meta(item, &meta),
            timing: None,
            stream: None,
            deck: None,
        }
    }

    /// The timeline rows, with their ids and the chain of stream titles.
    fn timeline_rows(&mut self, pv: &PlayoutView) -> Vec<Row> {
        let mut seen: HashMap<ItemId, u64> = HashMap::new();
        let mut rows = Vec::with_capacity(pv.timeline.entries.len());
        let mut refs = HashMap::new();
        let mut order = Vec::new();
        self.on_air_rows.clear();
        self.fixed_rows.clear();
        for e in &pv.timeline.entries {
            let Some((_, item)) = self.items.get(&e.item) else {
                continue;
            };
            let k = seen.entry(e.item).or_insert(0);
            let first = *k == 0;
            let id = e.item.0 | (*k << REPEAT_SHIFT);
            *k += 1;
            let mut row = self.row_for(id, item, e.playlist, e.start);
            // The first row of an item that a deck holds shows that deck.
            if first {
                row.deck = pv.decks.iter().position(|d| d.item == Some(e.item));
            }
            row.len = e.len;
            row.locked = e.locked;
            row.origin = match e.origin {
                SOrigin::Main => Origin::Main,
                SOrigin::Rule { .. } | SOrigin::Fill => Origin::Rule,
                SOrigin::Manual => Origin::Manual,
            };
            row.fixed = e.fixed.as_ref().map(|f| {
                let fit = pv
                    .timeline
                    .fits
                    .iter()
                    .find(|x| x.playlist == e.playlist && x.target == f.target);
                FitView {
                    error_s: fit.map_or(0.0, |x| x.error.as_secs_f32()),
                    ok: fit.is_none_or(|x| x.within_tolerance),
                }
            });
            row.stream = pv
                .stream_states
                .iter()
                .find(|s| s.item == e.item)
                .map(|s| stream_state(s, pv.now));
            refs.insert(
                id,
                RowRef {
                    playlist: e.playlist,
                    item: e.item,
                },
            );
            let end = e
                .start
                .checked_add(SignedDuration::try_from(e.len).unwrap_or_default())
                .unwrap_or(e.start);
            if e.start <= pv.now && pv.now < end {
                self.on_air_rows.insert(id);
            }
            if row.fixed.is_some() {
                self.fixed_rows.insert(id);
            }
            order.push(id);
            rows.push(row);
        }
        self.previous = self.title_chain(&rows);
        self.rows = refs;
        self.row_order = order;
        rows
    }

    /// The stream title before each row. The snapshot works it out once.
    fn title_chain(&self, rows: &[Row]) -> HashMap<u64, String> {
        let mut previous = self.station.clone();
        let mut out = HashMap::with_capacity(rows.len());
        for r in rows {
            out.insert(r.id, previous.clone());
            previous = radio_model::stream_title(&meta_item(r), &previous, &self.station);
        }
        out
    }

    fn decks(&mut self, pv: &PlayoutView, now: Timestamp) -> [DeckView; 4] {
        self.decks = std::array::from_fn(|i| pv.decks[i].item);
        std::array::from_fn(|i| {
            let d = &pv.decks[i];
            let state = match d.phase {
                DeckPhase::Empty => DeckState::Empty,
                DeckPhase::Loading | DeckPhase::Ready | DeckPhase::Cued => DeckState::Ready,
                DeckPhase::Playing => DeckState::Playing,
                DeckPhase::Paused => DeckState::Paused,
            };
            let len = match (d.start, d.end) {
                (Some(s), Some(e)) if e > s => e.duration_since(s).unsigned_abs(),
                _ => Duration::ZERO,
            };
            let elapsed = match (state, d.start) {
                (DeckState::Playing | DeckState::Paused, Some(s)) if now > s => {
                    now.duration_since(s).unsigned_abs().min(len)
                }
                _ => Duration::ZERO,
            };
            let (playlist, playlist_color) = d.item.and_then(|id| self.items.get(&id)).map_or(
                (String::new(), [0x80; 3]),
                |(p, _)| {
                    let (name, color, _) = self.playlist_look(*p);
                    (name, color)
                },
            );
            let hard = d.item.is_some_and(|id| {
                pv.timeline.entries.iter().any(|e| {
                    e.item == id
                        && e.fixed
                            .as_ref()
                            .is_some_and(|f| f.mode == radio_model::FixMode::Hard)
                })
            });
            // In Assist mode the first waiting deck waits for Next.
            let first_ready = pv
                .decks
                .iter()
                .enumerate()
                .filter(|(_, x)| matches!(x.phase, DeckPhase::Ready | DeckPhase::Cued))
                .min_by_key(|(_, x)| x.start)
                .map(|(k, _)| k);
            let waiting = pv.waiting_for_next && first_ready == Some(i);
            DeckView {
                playlist,
                playlist_color,
                hard,
                waiting,
                item: d.item.map(|id| id.0),
                label: ['A', 'B', 'C', 'D'][i],
                title: d
                    .item
                    .and_then(|id| self.items.get(&id))
                    .map_or_else(|| d.title.clone(), |(_, item)| display_title(item)),
                elapsed,
                len,
                state,
                start: d.start.map(|s| self.zoned(s)),
            }
        })
    }

    /// The as-played rows. The source reads them again after a while or after a deck change.
    fn history_rows(&mut self, now: Timestamp) -> Vec<Row> {
        let fresh = self.history_at.is_some_and(|(at, decks)| {
            decks == self.decks && now >= at && now.duration_since(at) < HISTORY_EVERY
        });
        if fresh {
            return self.history.clone();
        }
        self.history_at = Some((now, self.decks));
        let since = now
            .checked_sub(SignedDuration::from_hours(HISTORY_HOURS))
            .unwrap_or(now);
        // The background lane reads the log; a later snapshot shows the rows.
        if let Some(w) = &self.worker {
            let _ = w.jobs.send(bg::Job::History {
                log: self.playout.log(),
                since,
                limit: HISTORY_ROWS,
            });
        }
        self.history.clone()
    }

    fn history_row(&self, r: &radio_engine::AsPlayed, now: Timestamp) -> Row {
        let end = r.end.unwrap_or(now);
        let mut item = plain_item(&r.entry.title);
        item.artist.clone_from(&r.entry.artist);
        item.planned_len = Some(end.duration_since(r.entry.start).unsigned_abs());
        item.meta = effective_meta(&item, None);
        let mut title = display_title(&item);
        if r.status == Some(LogStatus::Interrupted) {
            title.push_str(" (cut off)");
        }
        Row {
            id: HISTORY_BIT | u64::try_from(r.id.0).unwrap_or(0),
            start: self.zoned(r.entry.start),
            playlist: r.entry.playlist.clone(),
            color: self
                .state
                .station
                .playlists
                .iter()
                .find(|p| p.name == r.entry.playlist)
                .map_or([0x80; 3], |p| p.color),
            title,
            len: item_len(&item),
            is_main: r.entry.origin == "music",
            fixed: None,
            origin: match r.entry.origin.as_str() {
                "music" => Origin::Main,
                "rule" | "fill" => Origin::Rule,
                _ => Origin::Manual,
            },
            locked: false,
            meta: view_meta(&item, &item.meta),
            timing: None,
            stream: None,
            deck: None,
        }
    }

    /// The carts per page, from the grid in the cartwall settings.
    fn per_page(&self) -> usize {
        let settings = self.state.settings(CARTWALL_PART);
        let num = |k: &str| settings.and_then(|s| s.get(k)).and_then(|v| v.as_u64());
        match (num("cols"), num("rows")) {
            (Some(c), Some(r)) => usize::try_from(c * r).unwrap_or(CARTS_PER_PAGE).max(1),
            _ => CARTS_PER_PAGE,
        }
    }

    fn cartwall(&self) -> (Vec<String>, CartwallView) {
        let per_page = self.per_page();
        let named: Vec<String> = self
            .state
            .settings(CARTWALL_PART)
            .and_then(|s| s.get("pages"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let used = self
            .state
            .carts
            .iter()
            .map(|c| c.page as usize + 1)
            .max()
            .unwrap_or(1);
        let pages: Vec<String> = (0..used.max(named.len()).max(1))
            .map(|p| {
                named
                    .get(p)
                    .cloned()
                    .unwrap_or_else(|| format!("Page {}", p + 1))
            })
            .collect();
        let mut carts = vec![String::new(); pages.len() * per_page];
        for c in &self.state.carts {
            let i = c.page as usize * per_page + c.slot as usize;
            if let (Some(label), Some((_, item))) = (carts.get_mut(i), self.items.get(&c.item)) {
                label.clone_from(&item.title);
            }
        }
        let slot = CartSlot {
            duck_db: 0.0,
            mode: CartMode::OneShot,
            colour: None,
        };
        let mut slots = vec![slot; carts.len()];
        for c in &self.state.cart_settings {
            let i = c.page as usize * per_page + c.slot as usize;
            if let Some(s) = slots.get_mut(i) {
                s.duck_db = c.duck_db;
                s.mode = match c.mode {
                    radio_engine::CartMode::OneShot => CartMode::OneShot,
                    radio_engine::CartMode::Loop => CartMode::Loop,
                    radio_engine::CartMode::Restart => CartMode::Restart,
                };
            }
        }
        let colours = self
            .state
            .settings(CARTWALL_PART)
            .and_then(|s| s.get("colours"))
            .and_then(|v| v.as_object());
        for (k, v) in colours.into_iter().flatten() {
            if let Some(slot) = k.parse::<usize>().ok().and_then(|i| slots.get_mut(i)) {
                slot.colour = v.as_u64().and_then(|c| usize::try_from(c).ok());
            }
        }
        let wall = CartwallView {
            pages,
            per_page,
            slots,
        };
        (carts, wall)
    }

    fn output(pv: &PlayoutView) -> OutputStatus {
        let local = match &pv.outputs.local {
            LocalOutput::Device { .. } => Status::Connected,
            LocalOutput::Null => Status::Failed("No sound card: nothing plays the sound".into()),
            LocalOutput::Offline => Status::Preview,
        };
        let icecast = match &pv.outputs.icecast {
            None | Some(StreamStatus::Stopped) => Status::NotConnected,
            Some(StreamStatus::Connecting) => Status::Reconnecting { attempt: 0 },
            Some(StreamStatus::Connected { .. }) => Status::Connected,
            Some(StreamStatus::Retrying { last_error, .. }) => Status::Failed(last_error.clone()),
        };
        OutputStatus {
            local,
            icecast,
            listeners: None,
        }
    }

    fn meters(&mut self, pv: &PlayoutView) -> Vec<MeterView> {
        let strips = &pv.state.strips;
        // A fader value leaves the overlay once the engine state shows it.
        self.strip_gain.retain(|k, g| {
            strips
                .get(k)
                .is_none_or(|s| (s.gain_db - *g).abs() > f32::EPSILON)
        });
        Strip::ALL
            .iter()
            .map(|s| {
                let m = pv.meters.strip(*s);
                let label = strip_label(*s);
                let st = strips.get(label).copied().unwrap_or_default();
                MeterView {
                    label: label.to_owned(),
                    level_db: m.rms_db,
                    peak_db: m.peak_hold_db,
                    gain_db: self.strip_gain.get(label).copied().unwrap_or(st.gain_db),
                    mute: st.mute,
                    pfl: st.pfl,
                    route: match st.route {
                        radio_engine::Route::Out => Route::Out,
                        radio_engine::Route::OutAndCue => Route::OutAndCue,
                        radio_engine::Route::CueOnly => Route::CueOnly,
                    },
                }
            })
            .collect()
    }

    /// The part settings, with the values on their way to the engine.
    fn settings(&mut self) -> radio_ui_egui::view::Settings {
        let mut out = self.state.ui_settings().clone();
        let state = &self.state;
        self.pending_settings
            .retain(|(p, k), v| state.settings(p).and_then(|s| s.get(k)) != Some(v));
        for ((p, k), v) in &self.pending_settings {
            out.entry(p.clone())
                .or_default()
                .insert(k.clone(), v.clone());
        }
        out
    }
}

/// The name of a mixer strip, as the mixer window shows it.
pub(crate) fn strip_label(s: Strip) -> &'static str {
    match s {
        Strip::Deck(d) => ["A", "B", "C", "D"][d.index()],
        Strip::Cart => "Cart",
        Strip::Live => "Live",
        Strip::Out => "Out",
    }
}

/// The text that identifies the source of an item in the library.
pub(crate) fn source_text(s: &Source) -> String {
    match s {
        Source::File { path } => path.clone(),
        Source::Stream { url, .. } => url.clone(),
        Source::LiveInput { device, channels } => {
            format!("live://{device}/{}-{}", channels[0], channels[1])
        }
    }
}

fn stream_state(s: &radio_playout::StreamState, now: Timestamp) -> StreamState {
    if s.up {
        return StreamState::Connected;
    }
    if !s.keep_alive {
        return StreamState::Down;
    }
    let down = s
        .down_since
        .map_or(0, |d| now.duration_since(d).as_secs().max(0));
    StreamState::Reconnecting {
        attempt: u32::try_from(down).unwrap_or(u32::MAX),
        next_in_s: 1,
    }
}

/// A file item with only a title, as a base for other items.
pub(crate) fn plain_item(title: &str) -> Item {
    Item {
        id: ItemId(0),
        meta: radio_model::ItemMeta::default(),
        title: title.to_owned(),
        artist: None,
        source: Source::File {
            path: String::new(),
        },
        timing: None,
        planned_len: None,
        movable: true,
        trimmable: true,
        locked: false,
        valid_from: None,
        valid_to: None,
        max_per_day: None,
    }
}

/// A model item with the tags of a row, for the stream title.
fn meta_item(r: &Row) -> Item {
    let mut item = plain_item(&r.meta.title);
    item.meta = map::model_meta(&r.meta);
    item
}

fn play_mode(m: Mode) -> PlayMode {
    match m {
        Mode::Auto => PlayMode::Auto,
        Mode::Assist => PlayMode::Assist,
        Mode::Manual => PlayMode::Manual,
    }
}

impl ViewSource for EngineSource {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let pv = self.playout.snapshot();
        if self.revision != Some(pv.revision) {
            self.rebuild(&pv.state);
            self.revision = Some(pv.revision);
        }
        self.take_done(now);
        self.poll_pending();
        self.poll_watches();
        let _ = self.fader_errors();
        let rows = self.timeline_rows(&pv);
        let decks = self.decks(&pv, now);
        self.deck_states = std::array::from_fn(|i| decks[i].state);
        let history = self.history_rows(now);
        let (carts, cartwall) = self.cartwall();
        let main = self.main.clone().unwrap_or_else(|| map::empty_tab("Main"));
        let mode = play_mode(pv.mode);
        let label = mode.label().to_uppercase();
        let open_alarms = pv.alarms.iter().filter(|a| a.cleared.is_none()).count();
        StationView {
            now: self.zoned(now),
            mode: if pv.emergency {
                format!("{label} · EMERGENCY")
            } else {
                label
            },
            play_mode: mode,
            stop_after_current: pv.stop_after_current,
            decks,
            rows,
            tabs: self.tabs.clone(),
            main_playlist: main.name.clone(),
            main,
            station: self.station.clone(),
            meters: self.meters(&pv),
            history,
            library: self
                .library
                .iter()
                .cloned()
                .chain(self.files.iter().map(|f| f.view.clone()))
                .collect(),
            devices: self.devices.clone(),
            device_roles: DeviceRoles {
                out: match &pv.outputs.local {
                    LocalOutput::Device { name } => name.clone(),
                    _ => String::new(),
                },
                pfl: String::new(),
                input: String::new(),
            },
            carts,
            cartwall,
            icecast: self.icecast(),
            output: Self::output(&pv),
            folders: self.folder_views(),
            settings: self.settings(),
            warnings: open_alarms + pv.timeline.warnings.len() + self.watch_errors.len(),
            preview: false,
        }
    }

    fn apply_rules(&mut self, tab: PlaylistTab) -> Result<(), String> {
        self.rules(tab)
    }

    fn drop_item(&mut self, item: u64, target: DropTarget) -> Result<usize, String> {
        self.drop_on(item, target)
    }

    fn import_files(&mut self, paths: Vec<String>, category: &str) -> Result<Imported, String> {
        self.import(paths, category)
    }

    fn take_notices(&mut self) -> Vec<String> {
        self.take_done(Timestamp::now());
        self.poll_pending();
        std::mem::take(&mut self.notices)
    }

    fn playlist_from_folder(&mut self, folder: FolderSource) -> Result<String, String> {
        self.folder_playlist(folder)
    }

    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        self.apply(edit)
    }

    fn library_ref(&mut self, title: &str, _len: Duration) -> Option<u64> {
        self.find_ref(title)
    }

    fn use_as_main(&mut self, name: &str) -> Result<(), String> {
        self.set_main(name)
    }

    fn duplicates(&self, playlist: &str) -> usize {
        self.playlist_id(playlist)
            .map_or(0, |p| self.duplicate_items(p).len())
    }

    fn meters(&self) -> Option<radio_ui_egui::view::MeterFrame> {
        let snap = self.meter_reader.lock().ok()?.read();
        // Before the first render block, the board holds zeros, which read as full scale.
        if snap.at_sample == 0 {
            return Some(radio_ui_egui::view::MeterFrame::default());
        }
        Some(radio_ui_egui::view::MeterFrame {
            level_db: Strip::ALL.iter().map(|s| snap.strip(*s).rms_db).collect(),
            peak_db: Strip::ALL
                .iter()
                .map(|s| snap.strip(*s).peak_hold_db)
                .collect(),
        })
    }

    fn stream_title(&self, row: &Row) -> String {
        let previous = self
            .previous
            .get(&row.id)
            .cloned()
            .unwrap_or_else(|| self.station.clone());
        radio_model::stream_title(&meta_item(row), &previous, &self.station)
    }
}
