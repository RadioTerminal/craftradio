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
    #[serde(rename = "deck.play")]
    DeckPlay { deck: u8 },
    #[serde(rename = "deck.pause")]
    DeckPause { deck: u8 },
    #[serde(rename = "deck.fade")]
    DeckFade { deck: u8 },
    #[serde(rename = "deck.skip")]
    DeckSkip { deck: u8 },
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
            | Self::LibraryRemoveFolder { .. } => Tier::OnAir,
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
        spec("deck.play", "Start a deck", A, &[d]),
        spec("deck.pause", "Pause a deck", A, &[d]),
        spec("deck.fade", "Fade a deck out", A, &[d]),
        spec("deck.skip", "Skip what a deck is playing", A, &[d]),
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
