//! The view-model the panels draw from. The UI depends only on these types, so a later
//! adapter can map the real scheduler into a [`StationView`] without touching the panels.

use std::time::Duration;

/// What a deck is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckState {
    Playing,
    /// Stopped in the middle of the item.
    Paused,
    /// Fading out. The next item follows.
    Fading,
    Ready,
    Empty,
}

/// One of the four decks.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckView {
    pub label: char,
    pub title: String,
    pub elapsed: Duration,
    pub len: Duration,
    pub state: DeckState,
}

/// How well a fixed-time item fits its slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitView {
    pub error_s: f32,
    pub ok: bool,
}

/// One row of the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub start: jiff::Zoned,
    pub playlist: String,
    pub color: [u8; 3],
    pub title: String,
    pub len: Duration,
    pub is_main: bool,
    pub fixed: Option<FitView>,
    /// What put the row in the timeline.
    pub origin: Origin,
    /// True when a person locked the row in place.
    pub locked: bool,
    /// The tags and the stream title of the item.
    pub meta: ItemMeta,
}

/// What Icecast gets as the stream title for an item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PublishMode {
    /// "Artist – Title" from the tags.
    #[default]
    FromTags,
    /// The custom text, with placeholders such as `{artist}`.
    Custom,
    /// The title of the item before stays. Jingles and ads use this.
    KeepPrevious,
    /// The station name.
    Hide,
}

impl PublishMode {
    pub const ALL: [Self; 4] = [Self::FromTags, Self::Custom, Self::KeepPrevious, Self::Hide];

    pub fn label(self) -> &'static str {
        match self {
            Self::FromTags => "From tags",
            Self::Custom => "Custom text",
            Self::KeepPrevious => "Keep previous",
            Self::Hide => "Hide",
        }
    }
}

/// The tags of an item, and how its stream title is made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub genre: String,
    pub comment: String,
    pub publish: PublishMode,
    pub custom: String,
}

/// An item whose tags a person can edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaTarget {
    Row(u64),
    Library(u64),
}

/// A command for a deck.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeckCommand {
    PlayPause,
    /// Fade out over this many seconds, then start the next item.
    Fade(f32),
    /// Fade out at once and start the next item.
    Skip,
}

/// What put a row in the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The main playlist.
    Main,
    /// A rule of another playlist.
    Rule,
    /// A person, by hand.
    Manual,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Rule => "rule",
            Self::Manual => "manual",
        }
    }
}

/// One file in the media library.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryItem {
    pub id: u64,
    pub title: String,
    pub artist: Option<String>,
    /// The length. Zero means that the length is not known.
    pub len: Duration,
    pub category: String,
    pub path: String,
    /// The tags and the stream title of the item.
    pub meta: ItemMeta,
}

/// Where a library item was dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// Into the main playlist, after the row with this id.
    Main { after: u64 },
    /// Onto the playlist with this name.
    Playlist(String),
    /// Onto the cart with this index.
    Cart(usize),
}

/// A named playlist's tab in the scheduled pane.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistTab {
    pub name: String,
    pub color: [u8; 3],
    pub pick: String,
    pub rules: Vec<String>,
    pub items: Vec<(String, Duration)>,
    /// The pick order as data, for the rules editor.
    pub order: Pick,
    /// The rules as data. The visual editor shows these rows.
    pub visual: Vec<RuleView>,
    /// The rules as a JavaScript script. The visual rows generate it.
    pub script: String,
    /// True when somebody changed the script by hand after the last generation.
    pub script_by_hand: bool,
    /// The folder that this playlist comes from, if a folder backs it.
    pub source: Option<FolderSource>,
}

/// A folder that backs a playlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderSource {
    pub path: String,
    /// Include the files in subfolders.
    pub recursive: bool,
    /// Follow changes in the folder. The library engine does this later.
    pub watch: bool,
}

/// The order in which a playlist picks its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    InOrder,
    Rotate,
    Shuffle,
    Latest,
    ByDate,
    Weighted,
}

impl Pick {
    pub const ALL: [Self; 6] = [
        Self::InOrder,
        Self::Rotate,
        Self::Shuffle,
        Self::Latest,
        Self::ByDate,
        Self::Weighted,
    ];

    /// The words a person reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::InOrder => "in order",
            Self::Rotate => "rotate in order",
            Self::Shuffle => "shuffle",
            Self::Latest => "latest item",
            Self::ByDate => "by date",
            Self::Weighted => "weighted",
        }
    }

    /// The name in the rules script.
    pub fn key(self) -> &'static str {
        match self {
            Self::InOrder => "in-order",
            Self::Rotate => "rotate",
            Self::Shuffle => "shuffle",
            Self::Latest => "latest",
            Self::ByDate => "by-date",
            Self::Weighted => "weighted",
        }
    }
}

/// What starts a rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// After every `tracks` main tracks.
    EveryTracks,
    /// At the first boundary after each minute in `minutes`, every hour.
    AtMinutes,
    /// At `time` every day.
    Daily,
}

impl Trigger {
    pub const ALL: [Self; 3] = [Self::EveryTracks, Self::AtMinutes, Self::Daily];

    pub fn label(self) -> &'static str {
        match self {
            Self::EveryTracks => "every N tracks",
            Self::AtMinutes => "at minutes",
            Self::Daily => "daily at",
        }
    }
}

/// Where the picked items go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Between two main tracks.
    Between,
    /// At a fixed time that must not move.
    FixedHard,
    /// At a fixed time that can move a little.
    FixedSoft,
    /// Under the main item, as a bed.
    Under,
}

impl Placement {
    pub const ALL: [Self; 4] = [Self::Between, Self::FixedHard, Self::FixedSoft, Self::Under];

    pub fn label(self) -> &'static str {
        match self {
            Self::Between => "between tracks",
            Self::FixedHard => "fixed (hard)",
            Self::FixedSoft => "fixed (soft)",
            Self::Under => "under talk",
        }
    }

    pub fn is_fixed(self) -> bool {
        matches!(self, Self::FixedHard | Self::FixedSoft)
    }
}

/// A kind of limit on a rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintKind {
    NotAdjacentToSelf,
    MaxPerDay,
    ArtistSeparation,
    NeverTrimmed,
    CutMainToFit,
}

impl ConstraintKind {
    pub const ALL: [Self; 5] = [
        Self::NotAdjacentToSelf,
        Self::MaxPerDay,
        Self::ArtistSeparation,
        Self::NeverTrimmed,
        Self::CutMainToFit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::NotAdjacentToSelf => "not back-to-back",
            Self::MaxPerDay => "max per day",
            Self::ArtistSeparation => "artist separation",
            Self::NeverTrimmed => "never trimmed",
            Self::CutMainToFit => "cut main to fit",
        }
    }

    /// The function name in the rules script.
    pub fn key(self) -> &'static str {
        match self {
            Self::NotAdjacentToSelf => "notAdjacentToSelf",
            Self::MaxPerDay => "maxPerDay",
            Self::ArtistSeparation => "artistSeparation",
            Self::NeverTrimmed => "neverTrimmed",
            Self::CutMainToFit => "cutMainToFit",
        }
    }

    /// True when the constraint takes a number.
    pub fn has_value(self) -> bool {
        matches!(self, Self::MaxPerDay | Self::ArtistSeparation)
    }
}

/// One limit on a rule, with its number when the kind takes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub n: u32,
}

/// One rule as data: trigger, placement, take and constraints.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleView {
    pub trigger: Trigger,
    /// The track count for [`Trigger::EveryTracks`].
    pub tracks: u32,
    /// The minutes past the hour for [`Trigger::AtMinutes`].
    pub minutes: Vec<u32>,
    /// The hour and minute for [`Trigger::Daily`].
    pub time: (u32, u32),
    pub place: Placement,
    /// The allowed fit error in seconds, for a fixed placement.
    pub tolerance_s: f32,
    pub take: u32,
    pub constraints: Vec<Constraint>,
}

impl RuleView {
    /// A new rule with plain defaults.
    pub fn new() -> Self {
        Self {
            trigger: Trigger::EveryTracks,
            tracks: 3,
            minutes: vec![0],
            time: (12, 0),
            place: Placement::Between,
            tolerance_s: 1.0,
            take: 1,
            constraints: Vec::new(),
        }
    }

    /// The rule in words, as one line.
    pub fn describe(&self) -> String {
        let when = match self.trigger {
            Trigger::EveryTracks => format!("Every {} tracks", self.tracks),
            Trigger::AtMinutes if self.minutes == [0] => "Every full hour".to_owned(),
            Trigger::AtMinutes => {
                let m: Vec<String> = self.minutes.iter().map(|m| format!(":{m:02}")).collect();
                m.join(" and ")
            }
            Trigger::Daily => format!("{:02}:{:02} daily", self.time.0, self.time.1),
        };
        let mut parts = vec![
            when,
            self.place.label().to_owned(),
            format!("take {}", self.take),
        ];
        for c in &self.constraints {
            parts.push(if c.kind.has_value() {
                format!("{} {}", c.kind.label(), c.n)
            } else {
                c.kind.label().to_owned()
            });
        }
        parts.join(" · ")
    }
}

impl Default for RuleView {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything the window shows at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct StationView {
    pub now: jiff::Zoned,
    pub mode: String,
    pub decks: [DeckView; 4],
    pub rows: Vec<Row>,
    pub tabs: Vec<PlaylistTab>,
    /// The main playlist, with its pick order and rules.
    pub main: PlaylistTab,
    /// The name of the main playlist.
    pub main_playlist: String,
    /// The station name. A hidden stream title shows this.
    pub station: String,
    /// Level meters: one per deck, then the cart bus, the live input and the output.
    pub meters: Vec<MeterView>,
    /// The as-played log, oldest first.
    pub history: Vec<Row>,
    /// The media library.
    pub library: Vec<LibraryItem>,
    /// Audio devices that the mixer can pick.
    pub devices: Vec<String>,
    pub carts: Vec<String>,
    pub warnings: usize,
}

/// One meter of the mixer, in dBFS, left and right.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterView {
    pub label: String,
    pub level_db: [f32; 2],
    pub peak_db: [f32; 2],
    pub gain_db: f32,
}

/// A place in the main timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Before(u64),
    After(u64),
    /// After the last row.
    End,
}

/// Where to move a row in its hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveTo {
    /// Right after the row that is on air.
    Next,
    /// First after the fixed item that opens the hour.
    Top,
    /// Last before the next fixed item.
    Bottom,
}

/// A change to the station. Panels ask for changes only through these commands.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Put a library item into the main timeline.
    Insert {
        item: u64,
        at: Place,
    },
    /// Take rows out of the timeline.
    Remove(Vec<u64>),
    Move {
        id: u64,
        to: MoveTo,
    },
    /// Lock or unlock a row in place.
    ToggleLock(u64),
    RenamePlaylist {
        from: String,
        to: String,
    },
    DuplicatePlaylist(String),
    DeletePlaylist(String),
    RemovePlaylistItem {
        playlist: String,
        index: usize,
    },
    MovePlaylistItem {
        playlist: String,
        index: usize,
        up: bool,
    },
    /// Put a library item on a cart, or clear the cart with `None`.
    AssignCart {
        cart: usize,
        item: Option<u64>,
    },
    /// Set the tags of items.
    SetMeta(Vec<(MetaTarget, ItemMeta)>),
    /// Control a deck by its index (0 is deck A).
    Deck {
        deck: usize,
        command: DeckCommand,
    },
}

/// Something that can describe the station at a given instant.
pub trait ViewSource {
    fn snapshot(&mut self, now: jiff::Timestamp) -> StationView;

    /// Store edited rules for the playlist `tab.name`. It returns true if the source kept them.
    /// No source applies them to a scheduler yet.
    fn apply_rules(&mut self, tab: PlaylistTab) -> bool {
        let _ = tab;
        false
    }

    /// Put library item `item` at `target`. It returns true if the source kept the change.
    fn drop_item(&mut self, item: u64, target: DropTarget) -> bool {
        let _ = (item, target);
        false
    }

    /// Add files to the library under `category`, by path. It returns the new item ids.
    fn import_files(&mut self, paths: Vec<String>, category: &str) -> Vec<u64> {
        let _ = (paths, category);
        Vec::new()
    }

    /// Make a playlist from the audio files of a folder. It returns the playlist name.
    fn playlist_from_folder(&mut self, folder: FolderSource, files: Vec<String>) -> Option<String> {
        let _ = (folder, files);
        None
    }

    /// Apply one edit. It returns the message for the person, or why the edit failed.
    fn edit(&mut self, edit: Edit) -> Result<String, String> {
        let _ = edit;
        Err("preview — this source cannot change".to_owned())
    }

    /// The library item for a title, made when the library does not have one.
    fn library_ref(&mut self, title: &str, len: Duration) -> Option<u64> {
        let _ = (title, len);
        None
    }

    /// Make the playlist `name` the main playlist. It returns true on success.
    fn use_as_main(&mut self, name: &str) -> bool {
        let _ = name;
        false
    }
}
