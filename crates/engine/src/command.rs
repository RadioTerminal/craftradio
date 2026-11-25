use jiff::{Timestamp, civil};
use radio_model::{FixMode, Item, ItemId, PlaylistId, Repeat};
use schemars::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn yes() -> bool {
    true
}

fn hard() -> FixMode {
    FixMode::Hard
}

fn once() -> Repeat {
    Repeat::Once
}

/// The tolerance of a fixed event that nobody set (spec 6.3).
pub const DEFAULT_TOLERANCE: std::time::Duration = std::time::Duration::from_secs(1);

fn default_tolerance() -> std::time::Duration {
    DEFAULT_TOLERANCE
}

/// Operating mode (spec 6.15).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum Mode {
    #[default]
    Auto,
    Assist,
    Manual,
}

/// How a cart plays when somebody fires it again.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum CartMode {
    #[default]
    OneShot,
    Loop,
    Restart,
}

/// Who may run a command (spec 6.14). MCP exposes both tiers and marks `ReadOnly` tools as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum Tier {
    ReadOnly,
    OnAir,
}

/// A change of the compressor on Program. A field that is absent keeps its value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompressorPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub threshold_db: Option<f32>,
    #[serde(default)]
    pub ratio: Option<f32>,
    #[serde(default)]
    pub attack_ms: Option<f32>,
    #[serde(default)]
    pub release_ms: Option<f32>,
    #[serde(default)]
    pub makeup_db: Option<f32>,
}

/// A change of the limiter on Program. A field that is absent keeps its value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LimiterPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub ceiling_db: Option<f32>,
    #[serde(default)]
    pub release_ms: Option<f32>,
}

impl CompressorPatch {
    /// Puts the values of the patch on `c`.
    pub fn apply(&self, c: &mut radio_model::CompressorSettings) {
        if let Some(v) = self.enabled {
            c.enabled = v;
        }
        if let Some(v) = self.threshold_db {
            c.threshold_db = v;
        }
        if let Some(v) = self.ratio {
            c.ratio = v;
        }
        if let Some(v) = self.attack_ms {
            c.attack_ms = v;
        }
        if let Some(v) = self.release_ms {
            c.release_ms = v;
        }
        if let Some(v) = self.makeup_db {
            c.makeup_db = v;
        }
    }
}

impl LimiterPatch {
    /// Puts the values of the patch on `l`.
    pub fn apply(&self, l: &mut radio_model::LimiterSettings) {
        if let Some(v) = self.enabled {
            l.enabled = v;
        }
        if let Some(v) = self.ceiling_db {
            l.ceiling_db = v;
        }
        if let Some(v) = self.release_ms {
            l.release_ms = v;
        }
    }
}

/// Every action on the station. Serialised as `{"id": "playlist.create", "args": {...}}`.
///
/// `source` and `meta` carry JSON until the model's `FolderSource` and `ItemMeta` types land,
/// when they become typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "id", content = "args")]
pub enum Command {
    #[serde(rename = "playlist.create")]
    PlaylistCreate { name: String },
    #[serde(rename = "playlist.rename")]
    PlaylistRename { playlist: PlaylistId, name: String },
    #[serde(rename = "playlist.delete")]
    PlaylistDelete { playlist: PlaylistId },
    #[serde(rename = "playlist.set_main")]
    PlaylistSetMain { playlist: PlaylistId },
    #[serde(rename = "playlist.enable")]
    PlaylistEnable { playlist: PlaylistId, enabled: bool },
    /// The fitter fills gaps from fill pools, and playout plays beds from them.
    #[serde(rename = "playlist.set_fill_pool")]
    PlaylistSetFillPool {
        playlist: PlaylistId,
        fill_pool: bool,
    },
    /// What a restart does with an item of this playlist that a crash cut off.
    #[serde(rename = "playlist.set_on_interrupt")]
    PlaylistSetOnInterrupt {
        playlist: PlaylistId,
        on_interrupt: radio_model::OnInterrupt,
    },
    /// With `lock`, the engine locks the items, because a person placed them (spec 6.15). A
    /// folder, a watch or an import adds items with `lock` false.
    #[serde(rename = "playlist.add_items")]
    PlaylistAddItems {
        playlist: PlaylistId,
        items: Vec<Item>,
        at: Option<usize>,
        #[serde(default = "yes")]
        lock: bool,
    },
    /// Removes every item of a playlist but the items on decks, and the locked items unless
    /// `remove_locked`. `expect_count` is the item count that the person saw; a playlist that
    /// changed since fails. A folder playlist stops watching its folder.
    #[serde(rename = "playlist.clear")]
    PlaylistClear {
        playlist: PlaylistId,
        expect_count: usize,
        #[serde(default = "yes")]
        remove_locked: bool,
    },
    #[serde(rename = "playlist.remove_items")]
    PlaylistRemoveItems {
        playlist: PlaylistId,
        items: Vec<ItemId>,
    },
    /// Moves the items, in the order given, so the first lands at index `to` of the remaining
    /// list. Moved items are locked.
    #[serde(rename = "playlist.move_items")]
    PlaylistMoveItems {
        playlist: PlaylistId,
        items: Vec<ItemId>,
        to: usize,
    },
    #[serde(rename = "playlist.set_rules_script")]
    PlaylistSetRulesScript {
        playlist: PlaylistId,
        script: String,
    },
    /// A rules script evaluated away from the playout control thread.
    #[serde(rename = "playlist.set_rules")]
    PlaylistSetRules {
        playlist: PlaylistId,
        pick: radio_model::PickOrder,
        rules: Vec<radio_model::Rule>,
        constraints: Vec<radio_model::Constraint>,
        script: String,
    },
    #[serde(rename = "playlist.set_source")]
    PlaylistSetSource { playlist: PlaylistId, source: Value },
    #[serde(rename = "item.lock")]
    ItemLock { item: ItemId },
    #[serde(rename = "item.unlock")]
    ItemUnlock { item: ItemId },
    #[serde(rename = "item.set_meta")]
    ItemSetMeta { item: ItemId, meta: Value },
    /// Marks file items as missing (playout skips them) or found again.
    #[serde(rename = "item.mark_missing")]
    ItemMarkMissing { items: Vec<ItemId>, missing: bool },
    /// Points a file item at a new path, for example a file that moved, and clears missing.
    #[serde(rename = "item.relink")]
    ItemRelink { item: ItemId, path: String },
    /// Stores the content hash (hex) and the size of the file of an item, for a relink.
    #[serde(rename = "item.set_hash")]
    ItemSetHash {
        item: ItemId,
        hash: String,
        size: u64,
    },
    /// Stores the loudness of the file of an item: an EBU R128 measurement, by content hash
    /// (hex) and size. Items with the same file share it. Playout sends this after it
    /// measures a file.
    #[serde(rename = "item.set_loudness")]
    ItemSetLoudness {
        item: ItemId,
        hash: String,
        size: u64,
        lufs: f32,
        #[serde(default)]
        peak_db: Option<f32>,
    },
    /// Sets the manual gain trim of an item, in dB, on top of its loudness gain. 0 clears it.
    #[serde(rename = "item.set_gain_trim")]
    ItemSetGainTrim { item: ItemId, db: f32 },
    /// Keep-alive of a stream item: it reconnects until its planned end.
    #[serde(rename = "item.set_keep_alive")]
    ItemSetKeepAlive { item: ItemId, keep_alive: bool },
    #[serde(rename = "deck.load")]
    DeckLoad { deck: u8, item: ItemId },
    /// With `expect_item`, the command fails with `StaleTarget` if the deck holds another item.
    /// A deck that has not started fades in over `fade_in`. The item on air fades out then. The
    /// decks that waited before this deck give up their items.
    #[serde(rename = "deck.play")]
    DeckPlay {
        deck: u8,
        #[serde(default)]
        expect_item: Option<ItemId>,
        #[serde(default)]
        fade_in: std::time::Duration,
    },
    /// With `expect_item`, the command fails with `StaleTarget` if the deck holds another item.
    #[serde(rename = "deck.pause")]
    DeckPause {
        deck: u8,
        #[serde(default)]
        expect_item: Option<ItemId>,
    },
    /// With `expect_item`, the command fails with `StaleTarget` if the deck holds another item.
    #[serde(rename = "deck.fade")]
    DeckFade {
        deck: u8,
        #[serde(default)]
        expect_item: Option<ItemId>,
        #[serde(default)]
        over: Option<std::time::Duration>,
    },
    /// With `expect_item`, the command fails with `StaleTarget` if the deck holds another item.
    #[serde(rename = "deck.skip")]
    DeckSkip {
        deck: u8,
        #[serde(default)]
        expect_item: Option<ItemId>,
    },
    /// Moves the file on a deck to `position`, counted from its cue-in: on a deck
    /// that waits, is paused or is on air. The planned end moves with it. With
    /// `expect_item`, the command fails with `StaleTarget` if the deck holds another item.
    #[serde(rename = "deck.seek")]
    DeckSeek {
        deck: u8,
        position: std::time::Duration,
        #[serde(default)]
        expect_item: Option<ItemId>,
    },
    #[serde(rename = "mode.set")]
    ModeSet { mode: Mode },
    #[serde(rename = "station.stop_after_current")]
    StationStopAfterCurrent,
    /// Plays the item right after the item on air, before the plan goes on.
    #[serde(rename = "playout.play_next")]
    PlayoutPlayNext { item: ItemId },
    /// Starts the next item now. In `Assist` mode the next item waits for this command.
    #[serde(rename = "playout.next")]
    PlayoutNext,
    #[serde(rename = "cart.assign")]
    CartAssign { page: u32, slot: u32, item: ItemId },
    #[serde(rename = "cart.fire")]
    CartFire { page: u32, slot: u32 },
    #[serde(rename = "cart.stop_all")]
    CartStopAll,
    #[serde(rename = "library.add_folder")]
    LibraryAddFolder { path: String },
    #[serde(rename = "library.remove_folder")]
    LibraryRemoveFolder { path: String },
    #[serde(rename = "station.get")]
    StationGet,
    #[serde(rename = "history.list")]
    HistoryList { since: Timestamp, limit: u32 },
    #[serde(rename = "history.export_csv")]
    HistoryExportCsv { since: Timestamp, until: Timestamp },
    /// The plays per artist and title in a range, for music royalty reporting. With a playlist
    /// name, only the plays of that playlist, so ads and jingles stay out.
    #[serde(rename = "history.plays_csv")]
    HistoryPlaysCsv {
        since: Timestamp,
        until: Timestamp,
        #[serde(default)]
        playlist: Option<String>,
    },
    /// Every airing in a range with its real start and end, for proof of airing. With a
    /// playlist name, only the airings of that playlist.
    #[serde(rename = "history.airings_csv")]
    HistoryAiringsCsv {
        since: Timestamp,
        until: Timestamp,
        #[serde(default)]
        playlist: Option<String>,
    },
    /// Takes back the last playlist or cart edit.
    #[serde(rename = "history.undo")]
    HistoryUndo,
    /// Does again the last edit that `history.undo` took back.
    #[serde(rename = "history.redo")]
    HistoryRedo,
    /// Runs station edits as one step: all of them, or none. Undo takes them back together.
    #[serde(rename = "station.batch")]
    StationBatch { commands: Vec<Command> },
    /// Replaces all playlists with those of a station file. Scripts and folder sources go.
    #[serde(rename = "station.import")]
    StationImport { station: radio_model::Station },
    /// Station edits that the system makes, such as a watched folder. They run as one step
    /// and are not undoable, so the undo history starts again after them.
    #[serde(rename = "station.system")]
    StationSystem { commands: Vec<Command> },
    /// Clears a stop-after-current, so the station plays on.
    #[serde(rename = "station.continue")]
    StationContinue,
    /// Stores one setting of one part of the UI.
    #[serde(rename = "settings.set")]
    SettingsSet {
        part: String,
        key: String,
        value: Value,
    },
    /// Pairs the station with a station server: writes the Icecast push, the WebRTC public
    /// stream and the `server` settings from the server's pairing document. Settings typed by
    /// hand since the last pairing are overwritten. The station key is never an argument;
    /// the app keeps it in the keychain.
    #[serde(rename = "server.pair")]
    ServerPair {
        /// The server's address, as the person typed it.
        address: String,
        /// What the server answered at `GET /api/station`.
        document: radio_model::PairingDocument,
    },
    /// Unpairs the station: removes every setting `server.pair` wrote, so each is back to its
    /// default. A station that is not paired keeps its settings.
    #[serde(rename = "server.unpair")]
    ServerUnpair,
    #[serde(rename = "playlist.set_color")]
    PlaylistSetColor {
        playlist: PlaylistId,
        color: [u8; 3],
    },
    #[serde(rename = "playlist.set_pick")]
    PlaylistSetPick {
        playlist: PlaylistId,
        pick: radio_model::PickOrder,
    },
    /// Copies a playlist with new item ids. The data is the id of the copy.
    #[serde(rename = "playlist.duplicate")]
    PlaylistDuplicate { playlist: PlaylistId, name: String },
    #[serde(rename = "cart.clear")]
    CartClear { page: u32, slot: u32 },
    /// Sets the duck depth and the play mode of a cart.
    #[serde(rename = "cart.set")]
    CartSet {
        page: u32,
        slot: u32,
        duck_db: f32,
        mode: CartMode,
    },
    /// Sets the fader, mute or SND of a mixer strip (A–D, Cart, Live, Out). SND sends the
    /// strip to the monitor device; `pfl` is its old name.
    #[serde(rename = "mixer.set_strip")]
    MixerSetStrip {
        strip: String,
        #[serde(default)]
        gain_db: Option<f32>,
        #[serde(default)]
        mute: Option<bool>,
        #[serde(default, alias = "pfl")]
        snd: Option<bool>,
        /// Gone: mute and SND replace routes. A value is refused with a message.
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        route: Option<Value>,
    },
    /// Sets the compressor and the limiter on Program. Absent fields keep their values.
    #[serde(rename = "mixer.set_dynamics")]
    MixerSetDynamics {
        #[serde(default)]
        compressor: Option<CompressorPatch>,
        #[serde(default)]
        limiter: Option<LimiterPatch>,
    },
    /// Sets loudness normalisation on the decks. Absent fields keep their values.
    #[serde(rename = "mixer.set_loudness")]
    MixerSetLoudness {
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        target_lufs: Option<f32>,
        #[serde(default)]
        max_boost_db: Option<f32>,
    },
    /// Acknowledges one alarm or warning by its id: it stops counting as new.
    #[serde(rename = "alarm.ack")]
    AlarmAck { id: u64 },
    /// Acknowledges the alarms and warnings with these ids. The engine does not hold the
    /// alarms themselves (playout and the plan raise them), so the caller names the ids.
    #[serde(rename = "alarm.ack_all")]
    AlarmAckAll { ids: Vec<u64> },
    /// Adds a one-off item to Upcoming. It starts at `start`, a local time of the station.
    /// The system playlist One-off holds it, with one At rule. The data is the item id. An
    /// id of 0 gets a new id. A time in a DST gap fails; a time that happens twice airs
    /// the first time, and the note says so.
    #[serde(rename = "schedule.add")]
    ScheduleAdd {
        one_off: Box<Item>,
        start: civil::DateTime,
        #[serde(default = "hard")]
        fix: FixMode,
        #[serde(default = "default_tolerance")]
        tolerance: std::time::Duration,
        #[serde(default = "once")]
        repeat: Repeat,
    },
    /// Replaces the item, the time and the timing of the one-off `id`. It keeps the id.
    #[serde(rename = "schedule.update")]
    ScheduleUpdate {
        id: ItemId,
        one_off: Box<Item>,
        start: civil::DateTime,
        #[serde(default = "hard")]
        fix: FixMode,
        #[serde(default = "default_tolerance")]
        tolerance: std::time::Duration,
        #[serde(default = "once")]
        repeat: Repeat,
    },
    /// Removes the one-off `id` and its rule.
    #[serde(rename = "schedule.remove")]
    ScheduleRemove { id: ItemId },
}

impl Command {
    /// The command's id string, as used by the CLI, MCP and scripts.
    pub fn id(&self) -> &'static str {
        match self {
            Self::PlaylistCreate { .. } => "playlist.create",
            Self::PlaylistRename { .. } => "playlist.rename",
            Self::PlaylistDelete { .. } => "playlist.delete",
            Self::PlaylistSetMain { .. } => "playlist.set_main",
            Self::PlaylistEnable { .. } => "playlist.enable",
            Self::PlaylistSetFillPool { .. } => "playlist.set_fill_pool",
            Self::PlaylistSetOnInterrupt { .. } => "playlist.set_on_interrupt",
            Self::PlaylistAddItems { .. } => "playlist.add_items",
            Self::PlaylistRemoveItems { .. } => "playlist.remove_items",
            Self::PlaylistClear { .. } => "playlist.clear",
            Self::PlaylistMoveItems { .. } => "playlist.move_items",
            Self::PlaylistSetRules { .. } => "playlist.set_rules",
            Self::PlaylistSetRulesScript { .. } => "playlist.set_rules_script",
            Self::PlaylistSetSource { .. } => "playlist.set_source",
            Self::ItemLock { .. } => "item.lock",
            Self::ItemUnlock { .. } => "item.unlock",
            Self::ItemSetMeta { .. } => "item.set_meta",
            Self::ItemSetKeepAlive { .. } => "item.set_keep_alive",
            Self::ItemMarkMissing { .. } => "item.mark_missing",
            Self::ItemRelink { .. } => "item.relink",
            Self::ItemSetHash { .. } => "item.set_hash",
            Self::ItemSetLoudness { .. } => "item.set_loudness",
            Self::ItemSetGainTrim { .. } => "item.set_gain_trim",
            Self::DeckLoad { .. } => "deck.load",
            Self::DeckPlay { .. } => "deck.play",
            Self::DeckPause { .. } => "deck.pause",
            Self::DeckFade { .. } => "deck.fade",
            Self::DeckSkip { .. } => "deck.skip",
            Self::DeckSeek { .. } => "deck.seek",
            Self::ModeSet { .. } => "mode.set",
            Self::StationStopAfterCurrent => "station.stop_after_current",
            Self::PlayoutNext => "playout.next",
            Self::PlayoutPlayNext { .. } => "playout.play_next",
            Self::CartAssign { .. } => "cart.assign",
            Self::CartFire { .. } => "cart.fire",
            Self::CartStopAll => "cart.stop_all",
            Self::LibraryAddFolder { .. } => "library.add_folder",
            Self::LibraryRemoveFolder { .. } => "library.remove_folder",
            Self::StationGet => "station.get",
            Self::HistoryList { .. } => "history.list",
            Self::HistoryExportCsv { .. } => "history.export_csv",
            Self::HistoryPlaysCsv { .. } => "history.plays_csv",
            Self::HistoryAiringsCsv { .. } => "history.airings_csv",
            Self::HistoryUndo => "history.undo",
            Self::HistoryRedo => "history.redo",
            Self::StationBatch { .. } => "station.batch",
            Self::StationContinue => "station.continue",
            Self::StationSystem { .. } => "station.system",
            Self::StationImport { .. } => "station.import",
            Self::SettingsSet { .. } => "settings.set",
            Self::ServerPair { .. } => "server.pair",
            Self::ServerUnpair => "server.unpair",
            Self::PlaylistSetColor { .. } => "playlist.set_color",
            Self::PlaylistSetPick { .. } => "playlist.set_pick",
            Self::PlaylistDuplicate { .. } => "playlist.duplicate",
            Self::CartClear { .. } => "cart.clear",
            Self::CartSet { .. } => "cart.set",
            Self::MixerSetStrip { .. } => "mixer.set_strip",
            Self::MixerSetDynamics { .. } => "mixer.set_dynamics",
            Self::MixerSetLoudness { .. } => "mixer.set_loudness",
            Self::AlarmAck { .. } => "alarm.ack",
            Self::AlarmAckAll { .. } => "alarm.ack_all",
            Self::ScheduleAdd { .. } => "schedule.add",
            Self::ScheduleUpdate { .. } => "schedule.update",
            Self::ScheduleRemove { .. } => "schedule.remove",
        }
    }

    /// Queries are read-only; everything that changes the station is on-air.
    pub fn tier(&self) -> Tier {
        match self {
            Self::StationGet
            | Self::HistoryList { .. }
            | Self::HistoryExportCsv { .. }
            | Self::HistoryPlaysCsv { .. }
            | Self::HistoryAiringsCsv { .. } => Tier::ReadOnly,
            Self::PlaylistCreate { .. }
            | Self::PlaylistRename { .. }
            | Self::PlaylistDelete { .. }
            | Self::PlaylistSetMain { .. }
            | Self::PlaylistEnable { .. }
            | Self::PlaylistSetFillPool { .. }
            | Self::PlaylistSetOnInterrupt { .. }
            | Self::PlaylistAddItems { .. }
            | Self::PlaylistRemoveItems { .. }
            | Self::PlaylistClear { .. }
            | Self::PlaylistMoveItems { .. }
            | Self::PlaylistSetRules { .. }
            | Self::PlaylistSetRulesScript { .. }
            | Self::PlaylistSetSource { .. }
            | Self::ItemLock { .. }
            | Self::ItemUnlock { .. }
            | Self::ItemSetMeta { .. }
            | Self::ItemSetKeepAlive { .. }
            | Self::ItemMarkMissing { .. }
            | Self::ItemRelink { .. }
            | Self::ItemSetHash { .. }
            | Self::ItemSetLoudness { .. }
            | Self::ItemSetGainTrim { .. }
            | Self::DeckLoad { .. }
            | Self::DeckPlay { .. }
            | Self::DeckPause { .. }
            | Self::DeckFade { .. }
            | Self::DeckSkip { .. }
            | Self::DeckSeek { .. }
            | Self::ModeSet { .. }
            | Self::StationStopAfterCurrent
            | Self::PlayoutNext
            | Self::PlayoutPlayNext { .. }
            | Self::CartAssign { .. }
            | Self::CartFire { .. }
            | Self::CartStopAll
            | Self::LibraryAddFolder { .. }
            | Self::LibraryRemoveFolder { .. }
            | Self::HistoryUndo
            | Self::HistoryRedo
            | Self::StationBatch { .. }
            | Self::StationContinue
            | Self::StationSystem { .. }
            | Self::StationImport { .. }
            | Self::SettingsSet { .. }
            | Self::ServerPair { .. }
            | Self::ServerUnpair
            | Self::PlaylistSetColor { .. }
            | Self::PlaylistSetPick { .. }
            | Self::PlaylistDuplicate { .. }
            | Self::CartClear { .. }
            | Self::CartSet { .. }
            | Self::MixerSetStrip { .. }
            | Self::MixerSetDynamics { .. }
            | Self::MixerSetLoudness { .. }
            | Self::AlarmAck { .. }
            | Self::AlarmAckAll { .. }
            | Self::ScheduleAdd { .. }
            | Self::ScheduleUpdate { .. }
            | Self::ScheduleRemove { .. } => Tier::OnAir,
        }
    }
}

/// Description of one command, for help text, the CLI and MCP tool lists.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub id: &'static str,
    pub summary: &'static str,
    pub tier: Tier,
    /// JSON Schema of the `args` object.
    pub args_schema: Schema,
}

fn spec(
    id: &'static str,
    summary: &'static str,
    tier: Tier,
    props: &[(&str, &str)],
) -> CommandSpec {
    let properties: serde_json::Map<String, Value> = props
        .iter()
        .map(|(k, t)| ((*k).to_string(), json!({ "type": t })))
        .collect();
    let required: Vec<&str> = props.iter().map(|(k, _)| *k).collect();
    let schema = json!({ "type": "object", "properties": properties, "required": required });
    CommandSpec {
        id,
        summary,
        tier,
        args_schema: Schema::try_from(schema).unwrap_or_else(|_| Schema::from(true)),
    }
}

/// Adds an optional integer argument `name` to a spec.
fn with_optional(mut spec: CommandSpec, name: &str) -> CommandSpec {
    let mut schema = spec.args_schema.as_value().clone();
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert(name.to_string(), json!({ "type": ["integer", "null"] }));
    }
    if let Ok(s) = Schema::try_from(schema) {
        spec.args_schema = s;
    }
    spec
}

/// Adds an optional argument `name` with the JSON schema `prop` to a spec.
fn with_optional_prop(mut spec: CommandSpec, name: &str, prop: Value) -> CommandSpec {
    let mut schema = spec.args_schema.as_value().clone();
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert(name.to_string(), prop);
    }
    if let Ok(s) = Schema::try_from(schema) {
        spec.args_schema = s;
    }
    spec
}

/// Adds the optional `fade_in` argument of `deck.play`: a duration as seconds and nanos.
fn with_fade_in(spec: CommandSpec) -> CommandSpec {
    let fade = json!({
        "type": "object",
        "properties": {"secs": {"type": "integer"}, "nanos": {"type": "integer"}},
        "required": ["secs", "nanos"],
    });
    with_optional_prop(spec, "fade_in", fade)
}

/// Every command the engine accepts, one entry each.
pub fn registry() -> Vec<CommandSpec> {
    use Tier::{OnAir as A, ReadOnly as R};
    let p = ("playlist", "integer");
    let d = ("deck", "integer");
    let mut specs = vec![
        spec(
            "playlist.create",
            "Create an empty named playlist",
            A,
            &[("name", "string")],
        ),
        spec(
            "playlist.rename",
            "Rename a playlist",
            A,
            &[p, ("name", "string")],
        ),
        spec("playlist.delete", "Delete a playlist", A, &[p]),
        spec("playlist.set_main", "Make a playlist the main one", A, &[p]),
        spec(
            "playlist.enable",
            "Enable or disable a playlist",
            A,
            &[p, ("enabled", "boolean")],
        ),
        spec(
            "playlist.set_fill_pool",
            "Make a playlist a fill pool, or not",
            A,
            &[p, ("fill_pool", "boolean")],
        ),
        spec(
            "playlist.set_on_interrupt",
            "Resume, Replay or Skip an item that a crash cut off",
            A,
            &[p, ("on_interrupt", "string")],
        ),
        with_optional_prop(
            spec(
                "playlist.add_items",
                "Add items at a position, or at the end; `lock` (default true) locks them",
                A,
                &[p, ("items", "array"), ("at", "integer")],
            ),
            "lock",
            json!({ "type": "boolean" }),
        ),
        spec(
            "playlist.remove_items",
            "Remove items from a playlist",
            A,
            &[p, ("items", "array")],
        ),
        spec(
            "playlist.clear",
            "Remove all items of a playlist but those on decks",
            A,
            &[p, ("expect_count", "integer"), ("remove_locked", "boolean")],
        ),
        spec(
            "playlist.move_items",
            "Move items (locked) to a position",
            A,
            &[p, ("items", "array"), ("to", "integer")],
        ),
        spec(
            "playlist.set_rules",
            "Apply rules evaluated on a worker",
            A,
            &[
                p,
                ("pick", "object"),
                ("rules", "array"),
                ("constraints", "array"),
                ("script", "string"),
            ],
        ),
        spec(
            "playlist.set_rules_script",
            "Replace a playlist's rules with a script; errors carry the line",
            A,
            &[p, ("script", "string")],
        ),
        spec(
            "playlist.set_source",
            "Set the folder a playlist follows",
            A,
            &[p, ("source", "object")],
        ),
        spec(
            "item.lock",
            "Pin an item so fitting never moves it",
            A,
            &[("item", "integer")],
        ),
        spec(
            "item.unlock",
            "Remove an item's pin",
            A,
            &[("item", "integer")],
        ),
        spec(
            "item.set_meta",
            "Edit an item's metadata",
            A,
            &[("item", "integer"), ("meta", "object")],
        ),
        spec(
            "item.mark_missing",
            "Mark file items as missing (skipped) or found",
            A,
            &[("items", "array"), ("missing", "boolean")],
        ),
        spec(
            "item.relink",
            "Point a file item at a new path",
            A,
            &[("item", "integer"), ("path", "string")],
        ),
        spec(
            "item.set_hash",
            "Store the content hash and size of an item's file",
            A,
            &[("item", "integer"), ("hash", "string"), ("size", "integer")],
        ),
        spec(
            "item.set_keep_alive",
            "Make a stream reconnect until its planned end, or not",
            A,
            &[("item", "integer"), ("keep_alive", "boolean")],
        ),
        spec(
            "deck.load",
            "Load an item into a deck",
            A,
            &[d, ("item", "integer")],
        ),
        with_fade_in(with_optional(
            spec(
                "deck.play",
                "Start a deck with a fade-in; the item on air fades out",
                A,
                &[d],
            ),
            "expect_item",
        )),
        with_optional(spec("deck.pause", "Pause a deck", A, &[d]), "expect_item"),
        with_optional_prop(
            with_optional(spec("deck.fade", "Fade a deck out", A, &[d]), "expect_item"),
            "over",
            json!({"type": ["object", "null"], "properties": {"secs": {"type": "integer", "minimum": 0}, "nanos": {"type": "integer", "minimum": 0, "maximum": 999999999}}, "required": ["secs", "nanos"]}),
        ),
        with_optional(
            spec("deck.skip", "Skip what a deck is playing", A, &[d]),
            "expect_item",
        ),
        with_optional_prop(
            with_optional(
                spec(
                    "deck.seek",
                    "Move the file on a deck to a position, counted from its cue-in",
                    A,
                    &[d, ("position", "object")],
                ),
                "expect_item",
            ),
            "position",
            json!({"type": "object", "properties": {"secs": {"type": "integer", "minimum": 0}, "nanos": {"type": "integer", "minimum": 0, "maximum": 999999999}}, "required": ["secs", "nanos"]}),
        ),
        spec(
            "mode.set",
            "Set Auto, Assist or Manual mode",
            A,
            &[("mode", "string")],
        ),
        spec(
            "station.stop_after_current",
            "Stop after the current item",
            A,
            &[],
        ),
        spec("playout.next", "Start the next item now", A, &[]),
        spec(
            "playout.play_next",
            "Play an item right after the item on air",
            A,
            &[("item", "integer")],
        ),
        spec(
            "cart.assign",
            "Put an item on a cartwall slot",
            A,
            &[
                ("page", "integer"),
                ("slot", "integer"),
                ("item", "integer"),
            ],
        ),
        spec(
            "cart.fire",
            "Fire a cartwall slot",
            A,
            &[("page", "integer"), ("slot", "integer")],
        ),
        spec("cart.stop_all", "Stop every playing cart", A, &[]),
        spec(
            "library.add_folder",
            "Add a folder to the library",
            A,
            &[("path", "string")],
        ),
        spec(
            "library.remove_folder",
            "Remove a folder from the library",
            A,
            &[("path", "string")],
        ),
        spec(
            "history.undo",
            "Take back the last playlist or cart edit",
            A,
            &[],
        ),
        spec(
            "history.redo",
            "Do again the edit that undo took back",
            A,
            &[],
        ),
        spec(
            "station.batch",
            "Run station edits as one step (all or none)",
            A,
            &[("commands", "array")],
        ),
        spec(
            "station.system",
            "Run system edits as one step; the undo history starts again",
            A,
            &[("commands", "array")],
        ),
        spec(
            "station.import",
            "Replace all playlists with those of a station file",
            A,
            &[("station", "object")],
        ),
        spec(
            "station.continue",
            "Clear stop-after-current; play on",
            A,
            &[],
        ),
        spec(
            "settings.set",
            "Store one setting of one part of the UI",
            A,
            &[("part", "string"), ("key", "string"), ("value", "object")],
        ),
        spec(
            "server.pair",
            "Pair with a station server: its pairing document sets the Icecast push and links",
            A,
            &[("address", "string"), ("document", "object")],
        ),
        spec(
            "server.unpair",
            "Unpair from the station server: the settings pairing wrote go back to defaults",
            A,
            &[],
        ),
        spec(
            "playlist.set_color",
            "Set the colour of a playlist",
            A,
            &[p, ("color", "array")],
        ),
        spec(
            "playlist.set_pick",
            "Set the pick order (and weights) of a playlist",
            A,
            &[p, ("pick", "object")],
        ),
        spec(
            "playlist.duplicate",
            "Copy a playlist with new item ids",
            A,
            &[p, ("name", "string")],
        ),
        spec(
            "cart.clear",
            "Clear a cartwall slot",
            A,
            &[("page", "integer"), ("slot", "integer")],
        ),
        spec(
            "cart.set",
            "Set the duck depth and play mode of a cart",
            A,
            &[
                ("page", "integer"),
                ("slot", "integer"),
                ("duck_db", "number"),
                ("mode", "string"),
            ],
        ),
        spec(
            "mixer.set_strip",
            "Set the fader, mute or SND (monitor send) of a mixer strip",
            A,
            &[("strip", "string")],
        ),
        spec(
            "mixer.set_dynamics",
            "Set the compressor and the limiter on Program",
            A,
            &[("compressor", "object"), ("limiter", "object")],
        ),
        spec(
            "mixer.set_loudness",
            "Set loudness normalisation on the decks: on/off, target LUFS, max boost",
            A,
            &[
                ("enabled", "boolean"),
                ("target_lufs", "number"),
                ("max_boost_db", "number"),
            ],
        ),
        spec(
            "item.set_loudness",
            "Store the measured loudness of the file of an item",
            A,
            &[
                ("item", "integer"),
                ("hash", "string"),
                ("size", "integer"),
                ("lufs", "number"),
            ],
        ),
        spec(
            "item.set_gain_trim",
            "Set the manual gain trim of an item, in dB",
            A,
            &[("item", "integer"), ("db", "number")],
        ),
        spec(
            "alarm.ack",
            "Acknowledge one alarm or warning by its id",
            A,
            &[("id", "integer")],
        ),
        spec(
            "alarm.ack_all",
            "Acknowledge the alarms and warnings with these ids",
            A,
            &[("ids", "array")],
        ),
        spec(
            "schedule.add",
            "Add a one-off item at a local time: hard or soft, with a tolerance and a repeat",
            A,
            &[("one_off", "object"), ("start", "string")],
        ),
        spec(
            "schedule.update",
            "Change the item, time or timing of a one-off",
            A,
            &[
                ("id", "integer"),
                ("one_off", "object"),
                ("start", "string"),
            ],
        ),
        spec(
            "schedule.remove",
            "Remove a one-off and its rule",
            A,
            &[("id", "integer")],
        ),
        spec("station.get", "Read the station state", R, &[]),
        spec(
            "history.list",
            "List as-played rows since a time",
            R,
            &[("since", "string"), ("limit", "integer")],
        ),
        spec(
            "history.export_csv",
            "Export as-played rows as CSV",
            R,
            &[("since", "string"), ("until", "string")],
        ),
        with_optional_prop(
            spec(
                "history.plays_csv",
                "Plays and air time per artist and title as CSV, for royalty reporting",
                R,
                &[("since", "string"), ("until", "string")],
            ),
            "playlist",
            json!({ "type": ["string", "null"] }),
        ),
        with_optional_prop(
            spec(
                "history.airings_csv",
                "Every airing with its real start and end as CSV, for proof of airing",
                R,
                &[("since", "string"), ("until", "string")],
            ),
            "playlist",
            json!({ "type": ["string", "null"] }),
        ),
    ];
    let schema = schemars::schema_for!(Command).to_value();
    if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        for spec in &mut specs {
            if let Some(variant) = variants
                .iter()
                .find(|v| v["properties"]["id"]["const"] == spec.id)
            {
                let mut args = variant["properties"].get("args").cloned().unwrap_or_else(
                    || json!({"type": "object", "properties": {}, "additionalProperties": false}),
                );
                if let Some(defs) = schema.get("$defs") {
                    args["$defs"] = defs.clone();
                }
                if let Ok(args) = Schema::try_from(args) {
                    spec.args_schema = args;
                }
            }
        }
    }
    specs
}
