//! A mock view source for the UI preview. It makes a fake timeline. It plays no audio.
//!
//! We invented all artists and titles.

mod alarms;
mod data;
mod decks;
mod edit;
mod folders;
mod source;
mod stream;
mod stress;
mod tabs;
mod timeline;

use std::collections::HashMap;
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan, Zoned};

use crate::context_menu::NEEDS_ENGINE;
use crate::rules_script;
use crate::view::PlayMode;
use crate::view::{
    CartChange, CartSlot, CartwallView, Constraint, ConstraintKind, DeckCommand, DeckState,
    DeckView, DeviceRole, DeviceRoles, DropTarget, Edit, FitView, FolderSource, FolderView,
    IcecastView, Imported, ItemMeta, LibraryItem, MetaTarget, MeterView, MoveTo, Origin,
    OutputStatus, Pick, Place, Placement, PlaylistTab, PublishMode, Route, Row, RuleView, Settings,
    StationView, Status, StreamFormat, StripChange, Timing, Trigger, ViewSource,
};

pub use stress::{STRESS_TITLES, StressSource, stress, stress_url};

use data::*;
use decks::*;
use folders::{Limits, Scan};
use stream::from_title;
use tabs::*;
use timeline::*;

/// The state of one mixer strip.
#[derive(Clone, Copy, Debug, Default)]
struct Strip {
    gain_db: f32,
    mute: bool,
    pfl: bool,
    route: Route,
}

/// A folder in the library and the result of its last scan.
struct Folder {
    path: String,
    scan: Scan,
}

/// The mock source. It builds about six hours of rows from the top of the current hour.
pub struct MockSource {
    tz: TimeZone,
    tracks: Vec<(String, Duration)>,
    cache: Option<(Timestamp, Vec<Row>)>,
    tabs: Vec<PlaylistTab>,
    main: PlaylistTab,
    library: Vec<LibraryItem>,
    carts: Vec<String>,
    cartwall: CartwallView,
    /// Edits to the timeline, in order. The mock applies them to each new snapshot.
    ops: Vec<Op>,
    next_manual: u64,
    /// The tracks of the main playlist when it is not the sample music.
    main_tracks: Option<Vec<(String, Duration)>>,
    last_top: Option<Zoned>,
    last_now: Timestamp,
    /// The decks of the last snapshot and when it came, for the meters between snapshots.
    last_decks: Option<([DeckView; 4], std::time::Instant)>,
    /// History and timeline of the last snapshot, in time order.
    last_all: Vec<Row>,
    /// Tags that a person changed, by row id.
    row_meta: HashMap<u64, ItemMeta>,
    /// Timings that a person set, by row id.
    row_timing: HashMap<u64, Timing>,
    /// Deck A stopped at this instant.
    paused_at: Option<Timestamp>,
    /// Deck A fades out from this instant over this many seconds.
    fade: Option<(Timestamp, f32)>,
    strips: HashMap<String, Strip>,
    icecast: IcecastView,
    /// The streaming switch. The preview sends no audio, so its status stays off.
    streaming: bool,
    dynamics: radio_model::DynamicsSettings,
    loudness: radio_model::LoudnessSettings,
    device_roles: DeviceRoles,
    folders: Vec<Folder>,
    limits: Limits,
    settings: Settings,
    /// The timeline covers this many hours from the top of the current hour.
    hours: i64,
    next_item_id: u64,
    undo: Vec<edit::Saved>,
    redo: Vec<edit::Saved>,
    /// How deep the current edit runs inside other edits.
    edit_depth: usize,
    play_mode: PlayMode,
    stop_after_current: bool,
    /// The sample alarms that a person acknowledged.
    acked_alarms: std::collections::HashSet<u64>,
}

impl MockSource {
    /// A mock with a longer timeline, for performance checks with many rows.
    pub fn with_hours(tz: TimeZone, hours: i64) -> Self {
        Self {
            hours: hours.max(1),
            ..Self::new(tz)
        }
    }

    pub fn new(tz: TimeZone) -> Self {
        let mut tracks = Vec::with_capacity(TRACKS);
        let mut library = Vec::new();
        for i in 0..TRACKS {
            let artist = ARTISTS[i % ARTISTS.len()];
            let n = (i * 7) % (NOUNS.len() * PLACES.len());
            let mut title = format!("{} of {}", NOUNS[n % NOUNS.len()], PLACES[n / NOUNS.len()]);
            if i >= NOUNS.len() * PLACES.len() {
                title.push_str(" (Night Version)");
            }
            // Lengths run from 2:30 to 5:00.
            let len = secs(150 + (i as u64 * 37) % 151);
            tracks.push((format!("{artist} – {title}"), len));
            library.push(LibraryItem {
                id: 0,
                path: format!("music/{artist} - {title}.mp3"),
                meta: ItemMeta {
                    title: title.clone(),
                    artist: artist.to_owned(),
                    ..ItemMeta::default()
                },
                title,
                artist: Some(artist.to_owned()),
                len,
                category: "Music".to_owned(),
            });
        }
        let mut add = |category: &str, folder: &str, title: String, len: Duration| {
            let file = title
                .to_lowercase()
                .replace([' ', '·'], "-")
                .replace("--", "-");
            library.push(LibraryItem {
                id: 0,
                path: format!("{folder}/{file}.wav"),
                meta: from_title(&title, publish_for(category)),
                title,
                artist: None,
                len,
                category: category.to_owned(),
            });
        };
        for n in 1..=3 {
            add("Jingles", "jingles", format!("Station ID {n}"), secs(6));
        }
        for name in AD_NAMES {
            add("Ads", "ads", name.to_owned(), secs(30));
        }
        for (name, len) in [("Warm Pad", 120), ("Soft Pulse", 95), ("Night Drive", 140)] {
            add("Beds", "beds", format!("Bed · {name}"), secs(len));
        }
        add("Shows", "shows", "Jazz Hour".to_owned(), secs(58 * 60 + 40));
        for (i, item) in library.iter_mut().enumerate() {
            item.id = i as u64 + 1;
        }
        // Every playlist item has an id, so a person can select it.
        let mut tabs = tabs();
        let mut next_item_id = TAB_ITEM_ID;
        for tab in &mut tabs {
            tab.ids = (next_item_id..).take(tab.items.len()).collect();
            next_item_id += tab.items.len() as u64;
        }
        let carts: Vec<String> = CARTS.iter().map(|c| (*c).to_owned()).collect();
        let pages = carts.len().div_ceil(CARTS_PER_PAGE);
        Self {
            tz,
            tracks,
            cache: None,
            tabs,
            main: main_tab(),
            library,
            cartwall: CartwallView {
                pages: (1..=pages).map(|n| format!("Page {n}")).collect(),
                per_page: CARTS_PER_PAGE,
                slots: vec![DEFAULT_SLOT; carts.len()],
            },
            carts,
            ops: Vec::new(),
            next_manual: MANUAL_ID,
            main_tracks: None,
            last_top: None,
            last_now: Timestamp::UNIX_EPOCH,
            last_decks: None,
            last_all: Vec::new(),
            row_meta: HashMap::new(),
            row_timing: HashMap::new(),
            paused_at: None,
            fade: None,
            strips: HashMap::new(),
            streaming: false,
            icecast: IcecastView {
                host: "stream.example.org".to_owned(),
                port: ICECAST_PORT,
                mount: "/live".to_owned(),
                user: "source".to_owned(),
                password: String::new(),
                password_set: false,
                keychain_error: None,
                tls: true,
                format: StreamFormat::Opus,
                kbps: ICECAST_KBPS,
            },
            dynamics: radio_model::DynamicsSettings::default(),
            loudness: radio_model::LoudnessSettings::default(),
            device_roles: DeviceRoles {
                out: DEVICES[0].to_owned(),
                pfl: DEVICES[3].to_owned(),
                input: DEVICES[1].to_owned(),
            },
            folders: Vec::new(),
            limits: Limits::DEFAULT,
            settings: Settings::new(),
            hours: HOURS,
            next_item_id,
            undo: Vec::new(),
            redo: Vec::new(),
            edit_depth: 0,
            play_mode: PlayMode::Auto,
            stop_after_current: false,
            acked_alarms: std::collections::HashSet::new(),
        }
    }

    pub(super) fn library_item(&self, id: u64) -> Option<&LibraryItem> {
        self.library.iter().find(|l| l.id == id)
    }

    pub(super) fn tab_mut(&mut self, name: &str) -> Option<&mut PlaylistTab> {
        if self.main.name == name {
            Some(&mut self.main)
        } else {
            self.tabs.iter_mut().find(|t| t.name == name)
        }
    }

    pub(super) fn unique_name(&self, base: &str) -> String {
        let taken = |n: &str| self.main.name == n || self.tabs.iter().any(|t| t.name == n);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|k| format!("{base} {k}"))
            .find(|n| !taken(n))
            .unwrap_or_else(|| base.to_owned())
    }

    fn next_library_id(&self) -> u64 {
        self.library.iter().map(|l| l.id).max().unwrap_or(0) + 1
    }

    /// Add audio files to the library under `category`. It returns the ids of the new items.
    fn add_library_files(&mut self, files: &[String], category: &str) -> Vec<u64> {
        let mut ids = Vec::new();
        for path in files {
            if let Some(found) = self.library.iter().find(|l| l.path == *path) {
                ids.push(found.id);
                continue;
            }
            let name = folders::stem_of(path).to_owned();
            let id = self.next_library_id();
            self.library.push(LibraryItem {
                id,
                meta: from_title(&name, PublishMode::FromTags),
                title: name,
                artist: None,
                len: Duration::ZERO,
                category: category.to_owned(),
                path: path.clone(),
            });
            ids.push(id);
        }
        ids
    }

    /// The folders of the library, for the snapshot.
    fn folder_views(&self) -> Vec<FolderView> {
        self.folders
            .iter()
            .map(|f| {
                let paths: std::collections::HashSet<&str> =
                    f.scan.files.iter().map(String::as_str).collect();
                FolderView {
                    path: f.path.clone(),
                    files: self
                        .library
                        .iter()
                        .filter(|l| paths.contains(l.path.as_str()))
                        .cloned()
                        .collect(),
                    truncated: f.scan.truncated,
                    errors: f.scan.errors.clone(),
                }
            })
            .collect()
    }

    /// Scan a folder and add its audio files to the library. It returns the number of files.
    fn scan_folder(&mut self, path: &str) -> usize {
        let scan = folders::scan(std::path::Path::new(path), false, self.limits);
        let name = folders::name_of(path).to_owned();
        let files = scan.files.clone();
        self.add_library_files(&files, &name);
        let n = files.len();
        match self.folders.iter_mut().find(|f| f.path == path) {
            Some(f) => f.scan = scan,
            None => self.folders.push(Folder {
                path: path.to_owned(),
                scan,
            }),
        }
        n
    }

    /// Read a part setting as an unsigned number.
    fn setting_u64(&self, part: &str, key: &str) -> Option<u64> {
        self.settings.get(part)?.get(key)?.as_u64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_run_in_order_and_fixed_rows_start_on_the_hour() {
        let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
        let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
        let view = MockSource::new(tz).snapshot(now);

        assert!(!view.rows.is_empty());
        for pair in view.rows.windows(2) {
            assert!(pair[0].start < pair[1].start, "rows are sorted by start");
            // Start times are cumulative. Each row starts where the row before it ends.
            let len = SignedDuration::try_from(pair[0].len).expect("len");
            assert_eq!(pair[0].start.timestamp() + len, pair[1].start.timestamp());
        }
        let fixed: Vec<_> = view.rows.iter().filter(|r| r.fixed.is_some()).collect();
        assert!(!fixed.is_empty(), "the mock has fixed rows");
        for row in fixed {
            let s = &row.start;
            assert_eq!(
                (s.minute(), s.second(), s.subsec_nanosecond()),
                (0, 0, 0),
                "{} starts exactly on the hour, not at {s}",
                row.title
            );
        }
    }

    #[test]
    fn a_one_off_starts_at_its_time_and_undo_brings_back_a_removed_one() {
        use crate::view::{OneOffRepeat, OneOffSource, OneOffView};
        let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
        let now: Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
        let mut source = MockSource::new(tz);
        source.snapshot(now);
        let start = jiff::civil::date(2026, 11, 21).at(15, 37, 0, 0);
        let added = source.edit(Edit::ScheduleOneOff(OneOffView {
            id: 0,
            source: OneOffSource::Url {
                url: "https://relay.example.org/live".to_owned(),
                title: "Relay".to_owned(),
                keep_alive: true,
                len: secs(120),
            },
            start,
            hard: true,
            tolerance_s: 1.0,
            repeat: OneOffRepeat::Once,
        }));
        assert_eq!(added, Ok(1));
        let view = source.snapshot(now);
        let row = view
            .rows
            .iter()
            .find(|r| r.one_off.is_some())
            .expect("the one-off has a row");
        assert_eq!(row.start.datetime(), start, "it starts at its time");
        assert_eq!(row.title, "Relay");
        for pair in view.rows.windows(2) {
            assert!(pair[0].start < pair[1].start, "rows stay in order");
        }
        let id = row.id;
        assert_eq!(source.edit(Edit::Remove(vec![id])), Ok(1));
        let gone = source.snapshot(now);
        assert!(gone.rows.iter().all(|r| r.one_off.is_none()));
        assert_eq!(source.edit(Edit::Undo), Ok(1));
        let back = source.snapshot(now);
        assert!(back.rows.iter().any(|r| r.id == id && r.one_off.is_some()));
    }

    #[test]
    fn a_scan_stops_at_its_limit_and_keeps_the_first_files_in_name_order() {
        let dir = std::env::temp_dir().join(format!("radio-scan-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        for name in ["e.mp3", "c.mp3", "a.mp3", "d.mp3", "b.mp3", "notes.txt"] {
            std::fs::write(dir.join(name), b"").expect("temp file");
        }
        let limits = Limits {
            max_files: 3,
            ..Limits::DEFAULT
        };
        let scan = folders::scan(&dir, false, limits);
        let names: Vec<&str> = scan.files.iter().map(|f| folders::name_of(f)).collect();
        assert!(
            scan.truncated,
            "a limit that the folder passes marks the scan"
        );
        assert_eq!(
            names,
            ["a.mp3", "b.mp3", "c.mp3"],
            "the first files by name stay"
        );
        let all = folders::scan(&dir, false, Limits::DEFAULT);
        assert!(!all.truncated);
        assert_eq!(all.files.len(), 5, "only audio files count");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_custom_stream_title_fills_its_placeholders() {
        let meta = ItemMeta {
            title: "Lanterns of Kelmora".to_owned(),
            artist: "Mira Solenne".to_owned(),
            year: "2024".to_owned(),
            publish: PublishMode::Custom,
            custom: "{artist} – {title} ({year}) on air".to_owned(),
            ..ItemMeta::default()
        };
        let text = stream::title_for(&meta, "before", "Station");
        assert_eq!(text, "Mira Solenne – Lanterns of Kelmora (2024) on air");
        assert!(
            !text.contains('{'),
            "no placeholder stays in the stream title"
        );
    }

    #[test]
    fn a_value_with_a_placeholder_is_not_filled_again() {
        let meta = ItemMeta {
            title: "Real Title".to_owned(),
            artist: "The {title} Band".to_owned(),
            ..ItemMeta::default()
        };
        let text = stream::render("{artist} – {title}", &meta);
        assert_eq!(text, "The {title} Band – Real Title");
    }

    #[test]
    fn a_kept_alive_stream_reconnects_and_then_plays_on() {
        use crate::view::StreamState;
        let tz = TimeZone::get("Europe/Ljubljana").expect("tz");
        let mut source = MockSource::new(tz);
        let at = |s: &str| -> Timestamp { s.parse().expect("ts") };
        let view = source.snapshot(at("2026-11-21T13:58:00Z"));
        let main = view.main_playlist.clone();
        let added = source.edit(Edit::AddStream {
            playlist: main,
            url: "https://radio.example.org/live".to_owned(),
            title: "Night Stream".to_owned(),
            planned_len: Duration::from_secs(60),
            keep_alive: true,
            at: Some(view.rows.len() - 1),
        });
        assert_eq!(added, Ok(1));
        let state_at = |source: &mut MockSource, now: &str| {
            let view = source.snapshot(at(now));
            view.rows
                .iter()
                .find(|r| r.title == "Night Stream")
                .and_then(|r| r.stream)
        };
        let dropped = state_at(&mut source, "2026-11-21T13:58:25Z");
        assert!(
            matches!(dropped, Some(StreamState::Reconnecting { .. })),
            "a dropped stream with keep-alive reconnects, not skips: {dropped:?}"
        );
        let back = state_at(&mut source, "2026-11-21T13:58:45Z");
        assert_eq!(back, Some(StreamState::Connected), "the stream comes back");
    }
}
