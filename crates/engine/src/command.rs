use jiff::Timestamp;
use radio_model::{Item, ItemId, PlaylistId};
use schemars::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Operating mode (spec 6.15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Auto,
    Assist,
    Manual,
}

/// How a cart plays when somebody fires it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CartMode {
    #[default]
    OneShot,
    Loop,
    Restart,
}

/// Where a mixer strip sends its sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Route {
    #[default]
    Out,
    OutAndCue,
    CueOnly,
}

/// Who may run a command (spec 6.14). MCP gets `ReadOnly` by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    ReadOnly,
    OnAir,
}

/// Every action on the station. Serialised as `{"id": "playlist.create", "args": {...}}`.
///
/// `source` and `meta` carry JSON until the model's `FolderSource` and `ItemMeta` types land,
/// when they become typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Items added by hand are locked (spec 6.15).
    #[serde(rename = "playlist.add_items")]
    PlaylistAddItems {
        playlist: PlaylistId,
        items: Vec<Item>,
        at: Option<usize>,
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
    #[serde(rename = "playlist.set_source")]
    PlaylistSetSource { playlist: PlaylistId, source: Value },
    #[serde(rename = "item.lock")]
    ItemLock { item: ItemId },
    #[serde(rename = "item.unlock")]
    ItemUnlock { item: ItemId },
    #[serde(rename = "item.set_meta")]
    ItemSetMeta { item: ItemId, meta: Value },
    #[serde(rename = "deck.load")]
    DeckLoad { deck: u8, item: ItemId },
    /// With `expect_item`, the command fails with `StaleTarget` if the deck holds another item.
    #[serde(rename = "deck.play")]
    DeckPlay {
        deck: u8,
        #[serde(default)]
        expect_item: Option<ItemId>,
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
    },
    /// With `expect_item`, the command fails with `StaleTarget` if the deck holds another item.
    #[serde(rename = "deck.skip")]
    DeckSkip {
        deck: u8,
        #[serde(default)]
        expect_item: Option<ItemId>,
    },
    #[serde(rename = "mode.set")]
    ModeSet { mode: Mode },
    #[serde(rename = "station.stop_after_current")]
    StationStopAfterCurrent,
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
    /// Takes back the last playlist or cart edit.
    #[serde(rename = "history.undo")]
    HistoryUndo,
    /// Does again the last edit that `history.undo` took back.
    #[serde(rename = "history.redo")]
    HistoryRedo,
    /// Runs station edits as one step: all of them, or none. Undo takes them back together.
    #[serde(rename = "station.batch")]
    StationBatch { commands: Vec<Command> },
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
    /// Sets the fader, mute, PFL or route of a mixer strip (A–D, Cart, Live, Out).
    #[serde(rename = "mixer.set_strip")]
    MixerSetStrip {
        strip: String,
        #[serde(default)]
        gain_db: Option<f32>,
        #[serde(default)]
        mute: Option<bool>,
        #[serde(default)]
        pfl: Option<bool>,
        #[serde(default)]
        route: Option<Route>,
    },
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
            Self::PlaylistMoveItems { .. } => "playlist.move_items",
            Self::PlaylistSetRulesScript { .. } => "playlist.set_rules_script",
            Self::PlaylistSetSource { .. } => "playlist.set_source",
            Self::ItemLock { .. } => "item.lock",
            Self::ItemUnlock { .. } => "item.unlock",
            Self::ItemSetMeta { .. } => "item.set_meta",
            Self::DeckLoad { .. } => "deck.load",
            Self::DeckPlay { .. } => "deck.play",
            Self::DeckPause { .. } => "deck.pause",
            Self::DeckFade { .. } => "deck.fade",
            Self::DeckSkip { .. } => "deck.skip",
            Self::ModeSet { .. } => "mode.set",
            Self::StationStopAfterCurrent => "station.stop_after_current",
            Self::PlayoutNext => "playout.next",
            Self::CartAssign { .. } => "cart.assign",
            Self::CartFire { .. } => "cart.fire",
            Self::CartStopAll => "cart.stop_all",
            Self::LibraryAddFolder { .. } => "library.add_folder",
            Self::LibraryRemoveFolder { .. } => "library.remove_folder",
            Self::StationGet => "station.get",
            Self::HistoryList { .. } => "history.list",
            Self::HistoryExportCsv { .. } => "history.export_csv",
            Self::HistoryUndo => "history.undo",
            Self::HistoryRedo => "history.redo",
            Self::StationBatch { .. } => "station.batch",
            Self::StationContinue => "station.continue",
            Self::SettingsSet { .. } => "settings.set",
            Self::PlaylistSetColor { .. } => "playlist.set_color",
            Self::PlaylistSetPick { .. } => "playlist.set_pick",
            Self::PlaylistDuplicate { .. } => "playlist.duplicate",
            Self::CartClear { .. } => "cart.clear",
            Self::CartSet { .. } => "cart.set",
            Self::MixerSetStrip { .. } => "mixer.set_strip",
        }
    }

    /// Queries are read-only; everything that changes the station is on-air.
    pub fn tier(&self) -> Tier {
        match self {
            Self::StationGet | Self::HistoryList { .. } | Self::HistoryExportCsv { .. } => {
                Tier::ReadOnly
            }
            Self::PlaylistCreate { .. }
            | Self::PlaylistRename { .. }
            | Self::PlaylistDelete { .. }
            | Self::PlaylistSetMain { .. }
            | Self::PlaylistEnable { .. }
            | Self::PlaylistSetFillPool { .. }
            | Self::PlaylistSetOnInterrupt { .. }
            | Self::PlaylistAddItems { .. }
            | Self::PlaylistRemoveItems { .. }
            | Self::PlaylistMoveItems { .. }
            | Self::PlaylistSetRulesScript { .. }
            | Self::PlaylistSetSource { .. }
            | Self::ItemLock { .. }
            | Self::ItemUnlock { .. }
            | Self::ItemSetMeta { .. }
            | Self::DeckLoad { .. }
            | Self::DeckPlay { .. }
            | Self::DeckPause { .. }
            | Self::DeckFade { .. }
            | Self::DeckSkip { .. }
            | Self::ModeSet { .. }
            | Self::StationStopAfterCurrent
            | Self::PlayoutNext
            | Self::CartAssign { .. }
            | Self::CartFire { .. }
            | Self::CartStopAll
            | Self::LibraryAddFolder { .. }
            | Self::LibraryRemoveFolder { .. }
            | Self::HistoryUndo
            | Self::HistoryRedo
            | Self::StationBatch { .. }
            | Self::StationContinue
            | Self::SettingsSet { .. }
            | Self::PlaylistSetColor { .. }
            | Self::PlaylistSetPick { .. }
            | Self::PlaylistDuplicate { .. }
            | Self::CartClear { .. }
            | Self::CartSet { .. }
            | Self::MixerSetStrip { .. } => Tier::OnAir,
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

/// Every command the engine accepts, one entry each.
pub fn registry() -> Vec<CommandSpec> {
    use Tier::{OnAir as A, ReadOnly as R};
    let p = ("playlist", "integer");
    let d = ("deck", "integer");
    vec![
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
        spec(
            "playlist.add_items",
            "Add items (locked) at a position, or at the end",
            A,
            &[p, ("items", "array"), ("at", "integer")],
        ),
        spec(
            "playlist.remove_items",
            "Remove items from a playlist",
            A,
            &[p, ("items", "array")],
        ),
        spec(
            "playlist.move_items",
            "Move items (locked) to a position",
            A,
            &[p, ("items", "array"), ("to", "integer")],
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
            "deck.load",
            "Load an item into a deck",
            A,
            &[d, ("item", "integer")],
        ),
        with_optional(spec("deck.play", "Start a deck", A, &[d]), "expect_item"),
        with_optional(spec("deck.pause", "Pause a deck", A, &[d]), "expect_item"),
        with_optional(spec("deck.fade", "Fade a deck out", A, &[d]), "expect_item"),
        with_optional(
            spec("deck.skip", "Skip what a deck is playing", A, &[d]),
            "expect_item",
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
            "Set the fader, mute, PFL or route of a mixer strip",
            A,
            &[("strip", "string")],
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
    ]
}
