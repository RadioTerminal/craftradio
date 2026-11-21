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
    /// The start time of the item, as the timeline plans it. `None` for an empty deck.
    pub start: Option<jiff::Zoned>,
    /// The playlist of the item, and its colour.
    pub playlist: String,
    pub playlist_color: [u8; 3],
    /// True when the item is a hard fixed event: it starts on its time sharp.
    pub hard: bool,
    /// True when the deck waits for Next (Assist mode) before it starts.
    pub waiting: bool,
    /// The id of the item, as `PlaylistTab::ids` has it. `None` when it is not known.
    pub item: Option<u64>,
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
    /// A timing that a person set for a fixed row.
    pub timing: Option<Timing>,
    /// The state of a stream row. `None` for other rows.
    pub stream: Option<StreamState>,
    /// The deck that holds the item of this row (0 is deck A), if a deck holds it.
    pub deck: Option<usize>,
}

/// How strictly a fixed row keeps its time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub hard: bool,
    pub tolerance_s: f32,
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
            Self::Hide => "Station name",
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
    /// For a stream: true when it reconnects until its planned end. `None` for other items.
    pub keep_alive: Option<bool>,
}

/// Who starts the next item: the schedule, the operator with help, or the operator alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayMode {
    #[default]
    Auto,
    Assist,
    Manual,
}

impl PlayMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Assist, Self::Manual];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Assist => "Assist",
            Self::Manual => "Manual",
        }
    }
}

/// The state of a stream row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Connected,
    /// The stream dropped. A bed plays while the source tries again.
    Reconnecting {
        attempt: u32,
        next_in_s: u32,
    },
    /// The stream dropped and keep-alive is off.
    Down,
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
            Self::Main => "Main playlist",
            Self::Rule => "Rule",
            Self::Manual => "Manual",
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

/// A named playlist's tab beside Upcoming.
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
    /// False when a person switched the playlist off.
    pub enabled: bool,
    /// The weight of each item, for the weighted pick order. It has one entry per item.
    pub weights: Vec<u32>,
    /// A stable id for each item. It has one entry per item.
    pub ids: Vec<u64>,
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
    /// The play mode as data. `mode` holds its label.
    pub play_mode: PlayMode,
    /// True when the station stops after the item on air.
    pub stop_after_current: bool,
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
    /// The device for each job.
    pub device_roles: DeviceRoles,
    pub carts: Vec<String>,
    /// Pages and settings of the carts in `carts`.
    pub cartwall: CartwallView,
    pub icecast: IcecastView,
    pub output: OutputStatus,
    /// Folders in the library.
    pub folders: Vec<FolderView>,
    /// Settings of each part of the app, by part and key.
    pub settings: Settings,
    pub warnings: usize,
    /// True for the UI preview. The preview plays no audio and saves nothing.
    pub preview: bool,
}

impl StationView {
    /// The text for the preview or for a real station, by the source.
    pub fn say<'a>(&self, preview: &'a str, real: &'a str) -> &'a str {
        if self.preview { preview } else { real }
    }
}

/// Settings by part of the app, then by key.
pub type Settings =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, serde_json::Value>>;

/// One meter of the mixer, in dBFS, left and right.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterView {
    pub label: String,
    pub level_db: [f32; 2],
    pub peak_db: [f32; 2],
    pub gain_db: f32,
    pub mute: bool,
    pub pfl: bool,
    pub route: Route,
}

/// The meter levels of one instant, in the order of `StationView::meters`, in dBFS.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeterFrame {
    pub level_db: Vec<[f32; 2]>,
    pub peak_db: Vec<[f32; 2]>,
}

impl MeterFrame {
    /// True when any meter shows sound.
    pub fn moving(&self) -> bool {
        self.level_db
            .iter()
            .any(|l| l[0].max(l[1]) > METER_SILENT_DB)
    }
}

/// A meter below this level is silent, for the choice of the repaint rate.
pub const METER_SILENT_DB: f32 = -58.0;

/// Where a mixer strip sends its sound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Route {
    #[default]
    Out,
    OutAndCue,
    CueOnly,
}

impl Route {
    pub const ALL: [Self; 3] = [Self::Out, Self::OutAndCue, Self::CueOnly];

    pub fn label(self) -> &'static str {
        match self {
            Self::Out => "Out",
            Self::OutAndCue => "Out + cue",
            Self::CueOnly => "Cue only",
        }
    }
}

/// A change to one mixer strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StripChange {
    Gain(f32),
    Mute(bool),
    Pfl(bool),
    Route(Route),
}

/// The codec of the Icecast stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamFormat {
    #[default]
    Opus,
    Mp3,
}

impl StreamFormat {
    pub const ALL: [Self; 2] = [Self::Opus, Self::Mp3];

    pub fn label(self) -> &'static str {
        match self {
            Self::Opus => "Opus",
            Self::Mp3 => "MP3",
        }
    }
}

/// The Icecast mount settings.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct IcecastView {
    pub host: String,
    pub port: u16,
    pub mount: String,
    pub user: String,
    /// A new password to store. A source never fills it in a snapshot, and empty keeps the old one.
    pub password: String,
    /// True when the source keeps a password for the mount.
    pub password_set: bool,
    /// Why the keychain could not answer, when it could not.
    pub keychain_error: Option<String>,
    /// Encrypt the connection (TLS). Off sends the password in clear text.
    pub tls: bool,
    pub format: StreamFormat,
    pub kbps: u32,
}

// The password never shows in a debug print.
impl IcecastView {
    /// True when the password goes over the network in clear text: TLS is off, a password
    /// is set, and the host is not this computer.
    pub fn password_in_clear(&self) -> bool {
        !self.tls && self.password_set && !is_loopback(&self.host)
    }
}

/// True for a host name or address of this computer.
pub fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl std::fmt::Debug for IcecastView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IcecastView")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("mount", &self.mount)
            .field("user", &self.user)
            .field("password", &"…")
            .field("password_set", &self.password_set)
            .field("keychain_error", &self.keychain_error)
            .field("tls", &self.tls)
            .field("format", &self.format)
            .field("kbps", &self.kbps)
            .finish()
    }
}

/// The state of an output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The preview shows the output, but no sound goes out.
    Preview,
    NotConnected,
    Connected,
    /// The connection dropped; the output tries again.
    Reconnecting {
        attempt: u32,
    },
    Failed(String),
}

/// The state of the local output and of Icecast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputStatus {
    pub local: Status,
    pub icecast: Status,
    /// The listener count from the Icecast server, when it reports one.
    pub listeners: Option<u32>,
}

/// The job of an audio device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceRole {
    Out,
    Pfl,
    Input,
}

/// The device for each job, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRoles {
    pub out: String,
    pub pfl: String,
    pub input: String,
}

/// How a cart plays when somebody presses it again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CartMode {
    #[default]
    OneShot,
    Loop,
    Restart,
}

impl CartMode {
    pub const ALL: [Self; 3] = [Self::OneShot, Self::Loop, Self::Restart];

    pub fn label(self) -> &'static str {
        match self {
            Self::OneShot => "One-shot",
            Self::Loop => "Loop",
            Self::Restart => "Restart",
        }
    }
}

/// The settings of one cart. The label is in `StationView::carts`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CartSlot {
    /// How much the cart lowers the main output while it plays, in dB. Zero means off.
    pub duck_db: f32,
    pub mode: CartMode,
    /// An index into the cart colours of the theme.
    pub colour: Option<usize>,
}

/// The pages of the cartwall. Page `p` holds the carts from `p * per_page`.
#[derive(Clone, Debug, PartialEq)]
pub struct CartwallView {
    pub pages: Vec<String>,
    pub per_page: usize,
    /// One entry per cart.
    pub slots: Vec<CartSlot>,
}

/// A change to one cart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CartChange {
    Duck(f32),
    Mode(CartMode),
    Colour(Option<usize>),
}

/// A folder in the library, as the source read it.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderView {
    pub path: String,
    pub files: Vec<LibraryItem>,
    /// True when the folder holds more files than the source lists.
    pub truncated: bool,
    /// Problems while reading the folder. They do not stop the reading.
    pub errors: Vec<String>,
}

/// The result of adding files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub ids: Vec<u64>,
    /// Files that are not audio, or that the source could not read.
    pub skipped: usize,
}

/// The placeholders that a custom stream title can use.
pub const PLACEHOLDERS: [&str; 6] = [
    "{artist}",
    "{title}",
    "{album}",
    "{year}",
    "{genre}",
    "{comment}",
];

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
    /// Make an empty playlist. The name is a wish; a taken name gets a number.
    NewPlaylist(String),
    DeletePlaylist(String),
    /// Remove items of a playlist, by their ids, as one undo step.
    RemovePlaylistItems {
        playlist: String,
        ids: Vec<u64>,
    },
    /// Remove library items from the playlists that hold them, as one undo step. The files
    /// stay on disk. Files of a folder leave only with their folder.
    RemoveLibraryItems(Vec<u64>),
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
    /// Set or clear the timing of a fixed row.
    SetTiming {
        row: u64,
        timing: Option<Timing>,
    },
    SetPlaylistEnabled {
        playlist: String,
        enabled: bool,
    },
    /// Give a playlist a colour of the palette, by its index in `theme::PALETTE`.
    SetPlaylistColour {
        playlist: String,
        colour: usize,
    },
    SetWeight {
        playlist: String,
        index: usize,
        weight: u32,
    },
    SetStrip {
        strip: String,
        change: StripChange,
    },
    SetIcecast(IcecastView),
    SetDevice {
        role: DeviceRole,
        device: String,
    },
    AddCartPage {
        name: String,
    },
    RenameCartPage {
        page: usize,
        name: String,
    },
    RemoveCartPage {
        page: usize,
    },
    SetCart {
        cart: usize,
        change: CartChange,
    },
    /// Add a folder to the library. The source reads it.
    AddFolder {
        path: String,
    },
    RemoveFolder {
        path: String,
    },
    /// Read a folder of the library again.
    Rescan {
        folder: String,
    },
    SetMode(PlayMode),
    /// Stop after the item on air, or clear that stop.
    StopAfterCurrent(bool),
    /// Play cart `cart`, an index into `StationView::carts`.
    FireCart {
        cart: usize,
    },
    /// Stop every cart that plays.
    StopCarts,
    /// Remove every item of a playlist but the items on decks. `expect_count` is the count
    /// that the person saw; with `remove_locked` false, locked items stay too.
    ClearPlaylist {
        playlist: String,
        expect_count: usize,
        remove_locked: bool,
    },
    /// Remove the items of a playlist whose file another item of it has already. The first
    /// item of each file stays.
    RemoveDuplicates {
        playlist: String,
    },
    /// Undo the last playlist edit.
    Undo,
    /// Do again the last edit that Undo took back.
    Redo,
    /// Set one setting of one part of the app.
    SetPartSetting {
        part: String,
        key: String,
        value: serde_json::Value,
    },
    /// Add audio files and folders to a playlist at item index `at`, or at the end.
    /// For the main playlist, `at` is an index into `StationView::rows`.
    AddFiles {
        playlist: String,
        paths: Vec<String>,
        at: Option<usize>,
    },
    /// Add the audio files of a folder to a playlist once.
    AddFolderItems {
        playlist: String,
        path: String,
        recursive: bool,
    },
    /// Add an internet stream with a planned length.
    AddStream {
        playlist: String,
        url: String,
        title: String,
        planned_len: Duration,
        /// Reconnect until the planned end, and never skip to the next item.
        keep_alive: bool,
        at: Option<usize>,
    },
    /// Add the live input with a planned length.
    AddLive {
        playlist: String,
        device: String,
        channels: (u32, u32),
        planned_len: Duration,
        at: Option<usize>,
    },
}

/// The file extensions that count as audio.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "flac", "wav", "ogg", "m4a", "opus", "aac"];

/// Check a stream address. It returns why the address is not usable.
/// It returns the host, which is also the default title of the stream.
pub fn check_stream_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| "The address must start with http:// or https://.".to_owned())?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // A user name or password in the address would show in logs and in the stream list.
    if authority.contains('@') {
        return Err(
            "Remove the user name and password from the address. Put them in the stream settings."
                .to_owned(),
        );
    }
    let host = authority.split(':').next().unwrap_or_default();
    if host.is_empty() || authority.contains(char::is_whitespace) {
        return Err("The address has no host.".to_owned());
    }
    Ok(host.to_owned())
}

/// Something that can describe the station at a given instant.
pub trait ViewSource {
    fn snapshot(&mut self, now: jiff::Timestamp) -> StationView;

    /// Store edited rules for the playlist `tab.name`. No source applies them to a scheduler yet.
    fn apply_rules(&mut self, tab: PlaylistTab) -> Result<(), String> {
        let _ = tab;
        Err(NOT_SUPPORTED.to_owned())
    }

    /// Put library item `item` at `target`. It returns the number of items that it placed.
    fn drop_item(&mut self, item: u64, target: DropTarget) -> Result<usize, String> {
        let _ = (item, target);
        Err(NOT_SUPPORTED.to_owned())
    }

    /// Add files and folders to the library under `category`. A folder gives its audio files.
    fn import_files(&mut self, paths: Vec<String>, category: &str) -> Result<Imported, String> {
        let _ = (paths, category);
        Err(NOT_SUPPORTED.to_owned())
    }

    /// Make a playlist from the audio files of a folder. It returns the playlist name.
    fn playlist_from_folder(&mut self, folder: FolderSource) -> Result<String, String> {
        let _ = folder;
        Err(NOT_SUPPORTED.to_owned())
    }

    /// Apply one edit. It returns the number of items that changed, or why the edit failed.
    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        let _ = edit;
        Err(NOT_SUPPORTED.to_owned())
    }

    /// The library item for a title, made when the library does not have one.
    fn library_ref(&mut self, title: &str, len: Duration) -> Option<u64> {
        let _ = (title, len);
        None
    }

    /// Make the playlist `name` the main playlist.
    fn use_as_main(&mut self, name: &str) -> Result<(), String> {
        let _ = name;
        Err(NOT_SUPPORTED.to_owned())
    }

    /// The number of items of `playlist` whose file another item of it has already.
    fn duplicates(&self, playlist: &str) -> usize {
        let _ = playlist;
        0
    }

    /// Messages for the person: results and errors of work that finished after its edit.
    fn take_notices(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// The meter levels now. It never blocks: the UI reads it on every frame.
    fn meters(&self) -> Option<MeterFrame> {
        None
    }

    /// The exact stream title that Icecast gets for `row`, at its place in the timeline.
    fn stream_title(&self, row: &Row) -> String;
}

/// The error of a source that cannot do a command.
pub const NOT_SUPPORTED: &str = "preview — this source cannot do that";

#[cfg(test)]
mod tests {
    use super::{IcecastView, check_stream_url};

    /// The warning shows only when the password goes to another computer in clear text.
    #[test]
    fn the_clear_password_warning_needs_no_tls_a_password_and_a_remote_host() {
        let ice = |host: &str, tls, password_set| IcecastView {
            host: host.into(),
            tls,
            password_set,
            ..IcecastView::default()
        };
        assert!(ice("stream.example.org", false, true).password_in_clear());
        assert!(!ice("stream.example.org", true, true).password_in_clear());
        assert!(!ice("stream.example.org", false, false).password_in_clear());
        for local in ["localhost", "127.0.0.1", "::1"] {
            assert!(!ice(local, false, true).password_in_clear(), "{local}");
        }
    }

    #[test]
    fn stream_addresses_need_a_scheme_and_a_host_and_no_credentials() {
        assert_eq!(
            check_stream_url("https://radio.example.org:8000/live").as_deref(),
            Ok("radio.example.org")
        );
        assert!(check_stream_url("http://radio.example.org").is_ok());
        assert!(check_stream_url("ftp://radio.example.org/live").is_err());
        assert!(check_stream_url("https:///live").is_err());
        let with_user = check_stream_url("https://user:secret@radio.example.org/live");
        assert!(with_user.is_err(), "credentials in the address are refused");
        assert!(
            !with_user.unwrap_err().contains("secret"),
            "the error does not repeat the password"
        );
    }
}
