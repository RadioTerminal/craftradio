//! A mock view source for the UI preview. It makes a fake timeline. It plays no audio.
//!
//! We invented all artists and titles.

use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan, Zoned};

use crate::context_menu::NEEDS_ENGINE;
use crate::folders;
use crate::meta;
use crate::rules_script;
use crate::view::{
    Constraint, ConstraintKind, DeckCommand, DeckState, DeckView, DropTarget, Edit, FitView,
    FolderSource, ItemMeta, LibraryItem, MetaTarget, MeterView, MoveTo, Origin, Pick, Place,
    Placement, PlaylistTab, PublishMode, Row, RuleView, StationView, Trigger, ViewSource,
};

const MUSIC: [u8; 3] = [0x5B, 0xAA, 0x5B];
const JINGLES: [u8; 3] = [0xE0, 0xA0, 0x30];
const ADS: [u8; 3] = [0xD0, 0x48, 0x48];
const NEWS: [u8; 3] = [0x4A, 0x90, 0xD9];
const SHOWS: [u8; 3] = [0x8E, 0x6A, 0xD8];
const BEDS: [u8; 3] = [0x3F, 0xB5, 0xA8];
/// Colours for playlists made from folders, in turn.
const FOLDER_COLOURS: [[u8; 3]; 4] = [
    [0x3F, 0xB5, 0xA8],
    [0xC8, 0x6F, 0xC9],
    [0x6F, 0x9F, 0xE8],
    [0xD9, 0x8C, 0x4A],
];
/// The station name. A hidden stream title shows it.
const STATION: &str = "Radio Preview";

/// How the stream title of an item from `playlist` is made at first.
fn publish_for(playlist: &str) -> PublishMode {
    match playlist {
        "Jingles" | "Ads" | "Beds" => PublishMode::KeepPrevious,
        _ => PublishMode::FromTags,
    }
}

/// The name of the built-in sample music once a folder takes its place.
const SAMPLE: &str = "Music (sample)";

const HOURS: i64 = 6;
/// The as-played log covers this many hours before now.
const PAST_HOURS: i64 = 3;
/// The level of a silent channel, in dBFS.
const SILENCE_DB: f32 = -90.0;
/// Manual rows take ids from this base, so they never meet ids made from start times.
const MANUAL_ID: u64 = 1 << 62;
const TRACKS: usize = 200;
const TRACKS_PER_JINGLE: usize = 3;
const ADS_PER_BREAK: usize = 3;
const AD_MINUTES: [i8; 2] = [20, 50];
const SHOW_HOUR: i8 = 16;

const ARTISTS: [&str; 20] = [
    "Mira Solenne",
    "Copper Lantern",
    "Juniper Vale",
    "The Quiet Engines",
    "Kolibri Static",
    "Marlo Fenwick",
    "Silver Tram",
    "Lumen Drift",
    "Ottilie Rask",
    "Nightjar Avenue",
    "Pela Morrow",
    "The Linden Hours",
    "Corvo Bay",
    "Ines Valtier",
    "Glass Orchard",
    "Tavi Ondrej",
    "Brindle & Moss",
    "Saffron Relay",
    "Halvard Pine",
    "Velka Strand",
];
const NOUNS: [&str; 10] = [
    "Lanterns",
    "Signals",
    "Tides",
    "Gardens",
    "Satellites",
    "Echoes",
    "Harbours",
    "Embers",
    "Orchards",
    "Meridians",
];
const PLACES: [&str; 10] = [
    "Kelmora", "Tavissa", "Brendhal", "Ulvenik", "Sarrow", "Miravel", "Quenby", "Hollin", "Draska",
    "Vintmoor",
];
const DEVICES: [&str; 4] = [
    "Built-in Output",
    "Built-in Microphone",
    "USB Audio Interface",
    "Headphones (USB)",
];
const AD_NAMES: [&str; 6] = [
    "Kesten Bakery",
    "Lumo Bikes",
    "Orrin Hardware",
    "Pavla Opticians",
    "Riverside Market",
    "Tamsin Travel",
];
const CARTS: [&str; 32] = [
    "Station ID 1",
    "Station ID 2",
    "Station ID 3",
    "Sweep",
    "Horn",
    "Laugh",
    "Applause",
    "Bed 1",
    "Bed 2",
    "Drum Roll",
    "Rimshot",
    "Cheer",
    "Countdown",
    "Stinger",
    "Whoosh",
    "Bell",
    "Weather Intro",
    "Traffic Intro",
    "Sports Intro",
    "News Intro",
    "Top of Hour",
    "Promo 1",
    "Promo 2",
    "Promo 3",
    "Phone Ring",
    "Crowd",
    "Siren",
    "Clock Tick",
    "Bass Drop",
    "Reverse Swell",
    "Outro",
    "Silence 5s",
];

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
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
    /// Edits to the timeline, in order. The mock applies them to each new snapshot.
    ops: Vec<Op>,
    next_manual: u64,
    /// The tracks of the main playlist when it is not the sample music.
    main_tracks: Option<Vec<(String, Duration)>>,
    last_top: Option<Zoned>,
    last_now: Timestamp,
    /// Tags that a person changed, by row id.
    row_meta: std::collections::HashMap<u64, ItemMeta>,
    /// Deck A stopped at this instant.
    paused_at: Option<Timestamp>,
    /// Deck A fades out from this instant over this many seconds.
    fade: Option<(Timestamp, f32)>,
}

/// One edit to the timeline.
#[derive(Clone)]
enum Op {
    Insert {
        at: Place,
        id: u64,
        title: String,
        len: Duration,
    },
    Remove(u64),
    Move {
        id: u64,
        at: Place,
    },
    Lock(u64),
    /// End the row early at `at`, so the next item starts then.
    Skip {
        id: u64,
        at: Timestamp,
    },
}

impl MockSource {
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
                meta: meta::from_title(&title, publish_for(category)),
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
        Self {
            tz,
            tracks,
            cache: None,
            tabs: tabs(),
            main: main_tab(),
            library,
            carts: CARTS.iter().map(|c| (*c).to_owned()).collect(),
            ops: Vec::new(),
            next_manual: MANUAL_ID,
            main_tracks: None,
            last_top: None,
            last_now: Timestamp::UNIX_EPOCH,
            row_meta: std::collections::HashMap::new(),
            paused_at: None,
            fade: None,
        }
    }

    /// Apply the edits to `rows`. It returns false when the rows no longer fit the fixed items.
    fn apply_ops(&self, rows: &mut Vec<Row>, ops: &[Op]) -> bool {
        for op in ops {
            match op {
                Op::Insert { at, id, title, len } => {
                    let Some(i) = index_of(rows, *at) else {
                        continue;
                    };
                    let Some(base) = rows.get(i.saturating_sub(1)).or(rows.first()).cloned() else {
                        continue;
                    };
                    rows.insert(
                        i,
                        Row {
                            id: *id,
                            playlist: self.main.name.clone(),
                            color: self.main.color,
                            title: title.clone(),
                            len: *len,
                            is_main: true,
                            fixed: None,
                            origin: Origin::Manual,
                            locked: false,
                            meta: meta::from_title(title, PublishMode::FromTags),
                            ..base
                        },
                    );
                }
                Op::Remove(id) => rows.retain(|r| r.id != *id),
                Op::Move { id, at } => {
                    let Some(from) = rows.iter().position(|r| r.id == *id) else {
                        continue;
                    };
                    let row = rows.remove(from);
                    match index_of(rows, *at) {
                        Some(i) => rows.insert(i, row),
                        None => rows.insert(from, row),
                    }
                }
                Op::Lock(id) => {
                    if let Some(r) = rows.iter_mut().find(|r| r.id == *id) {
                        r.locked = !r.locked;
                    }
                }
                Op::Skip { id, at } => {
                    let Some(i) = rows.iter().position(|r| r.id == *id) else {
                        continue;
                    };
                    let played = remaining(rows[i].start.timestamp(), *at);
                    if played.is_zero() || played >= rows[i].len {
                        continue;
                    }
                    rows[i].len = played;
                    let fixed = rows[i + 1..]
                        .iter()
                        .position(|r| r.fixed.is_some())
                        .map_or(rows.len(), |k| i + 1 + k);
                    let music = rows[i + 1..fixed].iter().any(|r| r.origin == Origin::Main);
                    if !music {
                        // Nothing can fill the gap before the fixed item. A filler track starts now.
                        let list = self.main_tracks.as_ref().unwrap_or(&self.tracks);
                        let pick = (*id as usize) % list.len().max(1);
                        if let Some((title, _)) = list.get(pick) {
                            let mut filler = rows[i].clone();
                            filler.id = id.wrapping_add(1) | MANUAL_ID;
                            filler.title.clone_from(title);
                            filler.meta = meta::from_title(title, PublishMode::FromTags);
                            filler.len = Duration::ZERO;
                            filler.origin = Origin::Main;
                            filler.is_main = true;
                            filler.fixed = None;
                            filler.locked = false;
                            filler.start = at.to_zoned(self.tz.clone());
                            rows.insert(i + 1, filler);
                        }
                    }
                }
            }
        }
        fit_all(rows, &self.tz)
    }

    /// The rows of the last snapshot window, with all edits.
    fn current(&mut self) -> Vec<Row> {
        let Some(top) = self.last_top.clone() else {
            return Vec::new();
        };
        let mut rows = self.rows_from(&top).to_vec();
        let ops = self.ops.clone();
        self.apply_ops(&mut rows, &ops);
        rows
    }

    /// Keep `op` when the rows still fit with it.
    fn try_op(&mut self, op: Op, done: String) -> Result<String, String> {
        let Some(top) = self.last_top.clone() else {
            return Err("preview — no timeline yet".to_owned());
        };
        let mut rows = self.rows_from(&top).to_vec();
        let mut ops = self.ops.clone();
        ops.push(op);
        if self.apply_ops(&mut rows, &ops) {
            self.ops = ops;
            Ok(done)
        } else {
            Err("preview — that does not fit before the next fixed item".to_owned())
        }
    }

    /// End the item on air at `at`, so the next item starts then.
    fn skip_on_air(&mut self, at: Timestamp) -> bool {
        let rows = self.current();
        let Some(r) = rows
            .iter()
            .find(|r| r.start.timestamp() <= at && at < end_of(r))
        else {
            return false;
        };
        let op = Op::Skip { id: r.id, at };
        self.paused_at = None;
        self.try_op(op, String::new()).is_ok()
    }

    fn deck_command(&mut self, deck: usize, command: DeckCommand) -> Result<String, String> {
        let name = ['A', 'B', 'C', 'D'].get(deck).copied().unwrap_or('?');
        if deck != 0 {
            return Err(format!(
                "preview — deck {name} is not on air · {NEEDS_ENGINE}"
            ));
        }
        let now = self.last_now;
        match command {
            DeckCommand::PlayPause => {
                if self.paused_at.take().is_some() {
                    Ok("preview — deck A plays again; the timeline kept time".to_owned())
                } else {
                    self.paused_at = Some(now);
                    Ok("preview — deck A paused here only; the timeline keeps time".to_owned())
                }
            }
            DeckCommand::Fade(secs) => {
                self.fade = Some((now, secs));
                Ok(format!(
                    "preview — deck A fades out over {secs:.1} s, then the next item starts"
                ))
            }
            DeckCommand::Skip => {
                self.fade = None;
                if self.skip_on_air(now) {
                    Ok("preview — skipped to the next item (not saved)".to_owned())
                } else {
                    Err("preview — nothing to skip".to_owned())
                }
            }
        }
    }

    fn library_item(&self, id: u64) -> Option<&LibraryItem> {
        self.library.iter().find(|l| l.id == id)
    }

    fn tab_mut(&mut self, name: &str) -> Option<&mut PlaylistTab> {
        if self.main.name == name {
            Some(&mut self.main)
        } else {
            self.tabs.iter_mut().find(|t| t.name == name)
        }
    }

    fn unique_name(&self, base: &str) -> String {
        let taken = |n: &str| self.main.name == n || self.tabs.iter().any(|t| t.name == n);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|k| format!("{base} {k}"))
            .find(|n| !taken(n))
            .unwrap_or_else(|| base.to_owned())
    }

    fn rows_from(&mut self, top: &Zoned) -> &[Row] {
        let key = top.timestamp();
        let stale = self.cache.as_ref().is_none_or(|(k, _)| *k != key);
        if stale {
            let mut rows = Vec::new();
            let mut hour = top
                .checked_sub(PAST_HOURS.hours())
                .unwrap_or_else(|_| top.clone());
            for _ in 0..PAST_HOURS + HOURS {
                let Ok(next) = hour.checked_add(1.hour()) else {
                    break;
                };
                self.fill_hour(&hour, &next, &mut rows);
                hour = next;
            }
            self.cache = Some((key, rows));
        }
        self.cache.as_ref().map_or(&[], |(_, rows)| rows.as_slice())
    }

    /// Fill one hour. A fixed item opens it, then music, jingles and ads run to the next hour.
    fn fill_hour(&self, hour: &Zoned, next: &Zoned, rows: &mut Vec<Row>) {
        let end = next.timestamp();
        let mut t = hour.timestamp();
        let push = |rows: &mut Vec<Row>, t: &mut Timestamp, item: Item| {
            let start = t.to_zoned(self.tz.clone());
            let Ok(span) = SignedDuration::try_from(item.len) else {
                return;
            };
            let main = item.playlist == "Music";
            *t += span;
            rows.push(Row {
                id: start.timestamp().as_millisecond().unsigned_abs(),
                start,
                playlist: if main {
                    self.main.name.clone()
                } else {
                    item.playlist.to_owned()
                },
                color: if main { self.main.color } else { item.color },
                len: item.len,
                is_main: main,
                fixed: item.fixed,
                origin: if main { Origin::Main } else { Origin::Rule },
                locked: false,
                meta: meta::from_title(&item.title, publish_for(item.playlist)),
                title: item.title,
            });
        };

        let opener = if hour.hour() == SHOW_HOUR {
            Item::fixed("Shows", SHOWS, "Jazz Hour.mp3", secs(58 * 60 + 40), 0.8)
        } else {
            Item::fixed("News", NEWS, "News", secs(5 * 60), -0.4)
        };
        push(rows, &mut t, opener);

        let hour_no = hour.timestamp().as_second().div_euclid(3600);
        let mut track = (hour_no.rem_euclid(TRACKS as i64) as usize * 17) % TRACKS;
        let mut since_jingle = 0;
        let mut jingle_no = hour_no.rem_euclid(3) as usize;
        let mut breaks_done = [false; AD_MINUTES.len()];
        let mut ad_no = 0;
        let mut ads_due = false;

        while t < end {
            let left = remaining(t, end);
            if ads_due {
                ads_due = false;
                let ad_len = secs(30);
                if left >= ad_len * ADS_PER_BREAK as u32 {
                    for _ in 0..ADS_PER_BREAK {
                        let name = AD_NAMES[ad_no % AD_NAMES.len()];
                        let title = format!("Ad {} · {name}", ad_no % ADS_PER_BREAK + 1);
                        push(rows, &mut t, Item::new("Ads", ADS, title, ad_len));
                        ad_no += 1;
                    }
                }
                continue;
            }
            if since_jingle == TRACKS_PER_JINGLE {
                since_jingle = 0;
                let len = secs(6);
                if left >= len {
                    let title = format!("Station ID {}", jingle_no % 3 + 1);
                    push(rows, &mut t, Item::new("Jingles", JINGLES, title, len));
                    jingle_no += 1;
                }
                continue;
            }
            let list = self.main_tracks.as_ref().unwrap_or(&self.tracks);
            let Some((title, len)) = list.get(track % list.len().max(1)) else {
                break;
            };
            track += 1;
            since_jingle += 1;
            // The last track before the next hour ends early, so the fixed item starts on time.
            push(
                rows,
                &mut t,
                Item::new("Music", MUSIC, title.clone(), (*len).min(left)),
            );
            let minute = t.to_zoned(self.tz.clone()).minute();
            let same_hour = t < end;
            for (done, at) in breaks_done.iter_mut().zip(AD_MINUTES) {
                if same_hour && !*done && minute >= at {
                    *done = true;
                    ads_due = true;
                }
            }
        }
    }
}

fn remaining(from: Timestamp, to: Timestamp) -> Duration {
    Duration::try_from(to.duration_since(from)).unwrap_or_default()
}

struct Item {
    playlist: &'static str,
    color: [u8; 3],
    title: String,
    len: Duration,
    fixed: Option<FitView>,
}

impl Item {
    fn new(playlist: &'static str, color: [u8; 3], title: String, len: Duration) -> Self {
        Self {
            playlist,
            color,
            title,
            len,
            fixed: None,
        }
    }

    fn fixed(playlist: &'static str, color: [u8; 3], title: &str, len: Duration, err: f32) -> Self {
        Self {
            fixed: Some(FitView {
                error_s: err,
                ok: true,
            }),
            ..Self::new(playlist, color, title.to_owned(), len)
        }
    }
}

fn deck(label: char, row: Option<&Row>, state: DeckState, elapsed: Duration) -> DeckView {
    match row {
        Some(r) => DeckView {
            label,
            title: r.title.clone(),
            elapsed,
            len: r.len,
            state,
        },
        None => DeckView {
            label,
            title: String::new(),
            elapsed: Duration::ZERO,
            len: Duration::ZERO,
            state: DeckState::Empty,
        },
    }
}

fn rule(trigger: Trigger, place: Placement, take: u32, constraints: &[Constraint]) -> RuleView {
    RuleView {
        trigger,
        place,
        take,
        constraints: constraints.to_vec(),
        ..RuleView::new()
    }
}

const fn limit(kind: ConstraintKind, n: u32) -> Constraint {
    Constraint { kind, n }
}

fn playlist(
    name: &str,
    color: [u8; 3],
    order: Pick,
    visual: Vec<RuleView>,
    items: Vec<(String, Duration)>,
) -> PlaylistTab {
    let mut tab = PlaylistTab {
        name: name.to_owned(),
        color,
        pick: String::new(),
        rules: Vec::new(),
        items,
        order,
        visual,
        script: String::new(),
        script_by_hand: false,
        source: None,
    };
    rules_script::refresh(&mut tab);
    tab
}

fn main_tab() -> PlaylistTab {
    let every = RuleView {
        tracks: 1,
        ..rule(
            Trigger::EveryTracks,
            Placement::Between,
            1,
            &[
                limit(ConstraintKind::ArtistSeparation, 3),
                limit(ConstraintKind::NotAdjacentToSelf, 0),
            ],
        )
    };
    playlist("Music", MUSIC, Pick::Shuffle, vec![every], Vec::new())
}

fn tabs() -> Vec<PlaylistTab> {
    use ConstraintKind as K;
    use Placement as P;
    use Trigger as T;
    let jingles = rule(
        T::EveryTracks,
        P::Between,
        1,
        &[limit(K::NotAdjacentToSelf, 0)],
    );
    let ads = RuleView {
        minutes: vec![20, 50],
        ..rule(
            T::AtMinutes,
            P::Between,
            3,
            &[limit(K::NeverTrimmed, 0), limit(K::MaxPerDay, 24)],
        )
    };
    let news = RuleView {
        minutes: vec![0],
        tolerance_s: 0.5,
        ..rule(T::AtMinutes, P::FixedHard, 1, &[limit(K::CutMainToFit, 0)])
    };
    let shows = RuleView {
        time: (16, 0),
        ..rule(T::Daily, P::FixedHard, 1, &[limit(K::CutMainToFit, 0)])
    };
    let beds = RuleView {
        tracks: 1,
        ..rule(T::EveryTracks, P::Under, 1, &[])
    };
    vec![
        playlist(
            "Jingles",
            JINGLES,
            Pick::Rotate,
            vec![jingles],
            (1..=3)
                .map(|n| (format!("Station ID {n}"), secs(6)))
                .collect(),
        ),
        playlist(
            "Ads",
            ADS,
            Pick::InOrder,
            vec![ads],
            AD_NAMES
                .iter()
                .map(|n| ((*n).to_owned(), secs(30)))
                .collect(),
        ),
        playlist(
            "News",
            NEWS,
            Pick::Latest,
            vec![news],
            vec![("News".to_owned(), secs(5 * 60))],
        ),
        playlist(
            "Shows",
            SHOWS,
            Pick::ByDate,
            vec![shows],
            vec![("Jazz Hour.mp3".to_owned(), secs(58 * 60 + 40))],
        ),
        playlist(
            "Beds",
            BEDS,
            Pick::Shuffle,
            vec![beds],
            vec![
                ("Bed 1 · Warm Pad".to_owned(), secs(120)),
                ("Bed 2 · Soft Pulse".to_owned(), secs(95)),
                ("Bed 3 · Night Drive".to_owned(), secs(140)),
            ],
        ),
    ]
}

/// The end instant of a row.
fn end_of(r: &Row) -> Timestamp {
    let start = r.start.timestamp();
    SignedDuration::try_from(r.len).map_or(start, |d| start + d)
}

/// Where a place is in `rows`, as an index to insert at.
fn index_of(rows: &[Row], at: Place) -> Option<usize> {
    match at {
        Place::Before(id) => rows.iter().position(|r| r.id == id),
        Place::After(id) => rows.iter().position(|r| r.id == id).map(|i| i + 1),
        Place::End => Some(rows.len()),
    }
}

/// The mock may shorten or lengthen this row to fit: a main row that nobody locked.
/// The mock tries the last rows of an hour first, so past rows rarely change.
fn adjustable(r: &Row) -> bool {
    r.origin == Origin::Main && !r.locked
}

/// Make every fixed row start on its time again. The mock cuts or lengthens future main rows,
/// then sets each start to the end of the row before. It returns false when a gap stays.
fn fit_all(rows: &mut Vec<Row>, tz: &TimeZone) -> bool {
    let mut ok = true;
    let mut p = 0;
    while p < rows.len() {
        let next_fixed = |rows: &[Row]| {
            rows.iter()
                .skip(p + 1)
                .position(|r| r.fixed.is_some())
                .map(|k| p + 1 + k)
        };
        let Some(f) = next_fixed(rows) else {
            let n = rows.len();
            retime(rows, p, n, tz);
            break;
        };
        let target = rows[f].start.timestamp();
        let begin = end_of(&rows[p]);
        let have: Duration = rows[p + 1..f].iter().map(|r| r.len).sum();
        let want = Duration::try_from(target.duration_since(begin)).unwrap_or_default();
        if have > want {
            let mut owed = have - want;
            for r in rows[p + 1..f].iter_mut().rev() {
                if owed.is_zero() {
                    break;
                }
                if adjustable(r) {
                    let cut = owed.min(r.len);
                    r.len -= cut;
                    owed -= cut;
                }
            }
            ok &= owed.is_zero();
        } else if have < want {
            match rows[p + 1..f].iter_mut().rev().find(|r| adjustable(r)) {
                Some(r) => r.len += want - have,
                None => ok = false,
            }
        }
        let mut i = p + 1;
        while i < rows.len() && rows[i].fixed.is_none() {
            if rows[i].len.is_zero() {
                rows.remove(i);
            } else {
                i += 1;
            }
        }
        let f = next_fixed(rows).unwrap_or(rows.len());
        retime(rows, p, f, tz);
        p = f;
    }
    ok
}

/// Set the start of each row in `from + 1 .. to` to the end of the row before it.
fn retime(rows: &mut [Row], from: usize, to: usize, tz: &TimeZone) {
    for j in from + 1..to.min(rows.len()) {
        let start = end_of(&rows[j - 1]);
        rows[j].start = start.to_zoned(tz.clone());
    }
}

/// A made-up length for a file that nobody decoded yet. The same name always gives the same length.
fn estimated_len(name: &str) -> Duration {
    let hash = name
        .bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    secs(150 + hash % 151)
}

/// Simulated meter levels. Only the playing deck and the output move.
fn meters(now: Timestamp, decks: &[DeckView; 4]) -> Vec<MeterView> {
    let t = now.as_millisecond() as f64 / 1000.0;
    let wave = |ch: f64| {
        let l = -14.0 + 5.0 * (t * 1.3 + ch * 0.7).sin() + 3.0 * (t * 3.1 + ch).sin();
        let peak = (l + 2.5 + 1.5 * (t * 0.5 + ch).sin().abs()).min(0.0);
        (l as f32, peak as f32)
    };
    let silent = |label: String| MeterView {
        label,
        level_db: [SILENCE_DB; 2],
        peak_db: [SILENCE_DB; 2],
        gain_db: 0.0,
    };
    let live = |label: String, offset: f64| {
        let (l0, p0) = wave(offset);
        let (l1, p1) = wave(offset + 0.4);
        MeterView {
            label,
            level_db: [l0, l1],
            peak_db: [p0, p1],
            gain_db: 0.0,
        }
    };
    let mut out: Vec<MeterView> = decks
        .iter()
        .map(|d| {
            let label = d.label.to_string();
            if d.state == DeckState::Playing {
                live(label, 0.0)
            } else {
                silent(label)
            }
        })
        .collect();
    out.push(silent("Cart".to_owned()));
    out.push(silent("Live".to_owned()));
    let playing = decks.iter().any(|d| d.state == DeckState::Playing);
    out.push(if playing {
        live("Out".to_owned(), 0.1)
    } else {
        silent("Out".to_owned())
    });
    out
}

impl ViewSource for MockSource {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let local = now.to_zoned(self.tz.clone());
        let top = local
            .with()
            .minute(0)
            .second(0)
            .subsec_nanosecond(0)
            .build()
            .unwrap_or_else(|_| local.clone());
        self.last_top = Some(top.clone());
        self.last_now = now;
        if let Some((from, secs)) = self.fade {
            let end = SignedDuration::try_from_secs_f32(secs).map_or(from, |d| from + d);
            if now >= end {
                self.fade = None;
                self.skip_on_air(end);
            }
        }
        let mut all = self.rows_from(&top).to_vec();
        let ops = self.ops.clone();
        self.apply_ops(&mut all, &ops);
        for r in &mut all {
            if let Some(m) = self.row_meta.get(&r.id) {
                r.meta = m.clone();
            }
        }
        let since = now
            .checked_sub(SignedDuration::from_hours(PAST_HOURS))
            .unwrap_or(now);
        let history = all
            .iter()
            .filter(|r| r.start.timestamp() >= since && r.start.timestamp() <= now)
            .cloned()
            .collect();
        let rows: Vec<Row> = all.into_iter().filter(|r| r.start >= top).collect();

        let on_air = rows.iter().position(|r| {
            let start = r.start.timestamp();
            let end = SignedDuration::try_from(r.len).map_or(start, |d| start + d);
            start <= now && now < end
        });
        let elapsed = on_air.map_or(Duration::ZERO, |i| {
            remaining(rows[i].start.timestamp(), now)
        });
        let at = |k: usize| on_air.and_then(|i| rows.get(i + k));
        let a_state = if self.fade.is_some() {
            DeckState::Fading
        } else if self.paused_at.is_some() {
            DeckState::Paused
        } else {
            DeckState::Playing
        };
        let elapsed = match self.paused_at {
            Some(p) => on_air.map_or(elapsed, |i| remaining(rows[i].start.timestamp(), p)),
            None => elapsed,
        };
        let decks = [
            deck('A', at(0), a_state, elapsed),
            deck('B', at(1), DeckState::Ready, Duration::ZERO),
            deck('C', at(2), DeckState::Ready, Duration::ZERO),
            deck('D', at(3), DeckState::Ready, Duration::ZERO),
        ];
        let meters = meters(now, &decks);

        StationView {
            now: local,
            mode: "AUTO".to_owned(),
            decks,
            rows,
            tabs: self.tabs.clone(),
            main: self.main.clone(),
            main_playlist: self.main.name.clone(),
            station: STATION.to_owned(),
            meters,
            history,
            library: self.library.clone(),
            carts: self.carts.clone(),
            devices: DEVICES.iter().map(|d| (*d).to_owned()).collect(),
            warnings: 0,
        }
    }

    fn drop_item(&mut self, item: u64, target: DropTarget) -> bool {
        let Some(lib) = self.library.iter().find(|l| l.id == item) else {
            return false;
        };
        let entry = (lib.title.clone(), lib.len);
        match target {
            DropTarget::Main { after } => self
                .edit(Edit::Insert {
                    item,
                    at: Place::After(after),
                })
                .is_ok(),
            DropTarget::Playlist(name) => {
                let tab = if self.main.name == name {
                    Some(&mut self.main)
                } else {
                    self.tabs.iter_mut().find(|t| t.name == name)
                };
                tab.map(|t| t.items.push(entry)).is_some()
            }
            DropTarget::Cart(i) => self.carts.get_mut(i).map(|c| *c = entry.0).is_some(),
        }
    }

    fn import_files(&mut self, paths: Vec<String>, category: &str) -> Vec<u64> {
        let mut ids = Vec::new();
        for path in paths {
            let name = folders::stem_of(&path).to_owned();
            let id = self.library.iter().map(|l| l.id).max().unwrap_or(0) + 1;
            self.library.push(LibraryItem {
                id,
                meta: meta::from_title(&name, PublishMode::FromTags),
                title: name,
                artist: None,
                len: Duration::ZERO,
                category: category.to_owned(),
                path,
            });
            ids.push(id);
        }
        ids
    }

    fn edit(&mut self, edit: Edit) -> Result<String, String> {
        let not_saved = |what: String| format!("preview — {what} (not saved)");
        match edit {
            Edit::Insert { item, at } => {
                let Some(lib) = self.library_item(item) else {
                    return Err("preview — that library item is gone".to_owned());
                };
                let title = match &lib.artist {
                    Some(a) => format!("{a} – {}", lib.title),
                    None => lib.title.clone(),
                };
                let len = if lib.len.is_zero() {
                    estimated_len(&title)
                } else {
                    lib.len
                };
                let id = self.next_manual;
                self.next_manual += 1;
                let done = not_saved(format!("added {title} to {}", self.main.name));
                self.try_op(Op::Insert { at, id, title, len }, done)
            }
            Edit::Remove(ids) => {
                let n = ids.len();
                let mut ops = self.ops.clone();
                ops.extend(ids.into_iter().map(Op::Remove));
                let Some(top) = self.last_top.clone() else {
                    return Err("preview — no timeline yet".to_owned());
                };
                let mut rows = self.rows_from(&top).to_vec();
                if self.apply_ops(&mut rows, &ops) {
                    self.ops = ops;
                    Ok(not_saved(format!("removed {n} rows")))
                } else {
                    Err(
                        "preview — nothing left to fill the gap before the next fixed item"
                            .to_owned(),
                    )
                }
            }
            Edit::Move { id, to } => {
                let rows = self.current();
                let Some(i) = rows.iter().position(|r| r.id == id) else {
                    return Err("preview — that row is gone".to_owned());
                };
                let at = match to {
                    MoveTo::Next => {
                        let now = self.last_now;
                        match rows
                            .iter()
                            .find(|r| r.start.timestamp() <= now && now < end_of(r))
                        {
                            Some(r) => Place::After(r.id),
                            None => return Err("preview — nothing is on air".to_owned()),
                        }
                    }
                    MoveTo::Top => match rows[..i].iter().rev().find(|r| r.fixed.is_some()) {
                        Some(r) => Place::After(r.id),
                        None => Place::Before(rows[0].id),
                    },
                    MoveTo::Bottom => match rows[i + 1..].iter().find(|r| r.fixed.is_some()) {
                        Some(r) => Place::Before(r.id),
                        None => Place::End,
                    },
                };
                self.try_op(Op::Move { id, at }, not_saved("moved the row".to_owned()))
            }
            Edit::ToggleLock(id) => {
                self.try_op(Op::Lock(id), not_saved("changed the lock".to_owned()))
            }
            Edit::RenamePlaylist { from, to } => {
                let to = to.trim().to_owned();
                if to.is_empty() || self.unique_name(&to) != to {
                    return Err(format!("preview — the name {to:?} is empty or taken"));
                }
                let Some(tab) = self.tab_mut(&from) else {
                    return Err(format!("preview — no playlist {from}"));
                };
                tab.name.clone_from(&to);
                rules_script::refresh(tab);
                self.cache = None;
                Ok(not_saved(format!("renamed {from} to {to}")))
            }
            Edit::DuplicatePlaylist(name) => {
                let Some(tab) = self
                    .tabs
                    .iter()
                    .chain([&self.main])
                    .find(|t| t.name == name)
                else {
                    return Err(format!("preview — no playlist {name}"));
                };
                let mut copy = tab.clone();
                copy.name = self.unique_name(&format!("{name} copy"));
                rules_script::refresh(&mut copy);
                let msg = not_saved(format!("made {}", copy.name));
                self.tabs.push(copy);
                Ok(msg)
            }
            Edit::DeletePlaylist(name) => {
                if self.main.name == name {
                    return Err(
                        "preview — the main playlist cannot go; make another one main first"
                            .to_owned(),
                    );
                }
                let before = self.tabs.len();
                self.tabs.retain(|t| t.name != name);
                if self.tabs.len() == before {
                    return Err(format!("preview — no playlist {name}"));
                }
                Ok(not_saved(format!("deleted {name}")))
            }
            Edit::RemovePlaylistItem { playlist, index } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(format!("preview — no playlist {playlist}"));
                };
                if index >= tab.items.len() {
                    return Err("preview — that item is gone".to_owned());
                }
                let (title, _) = tab.items.remove(index);
                Ok(not_saved(format!("removed {title} from {playlist}")))
            }
            Edit::MovePlaylistItem {
                playlist,
                index,
                up,
            } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(format!("preview — no playlist {playlist}"));
                };
                let other = if up {
                    index.checked_sub(1)
                } else {
                    Some(index + 1)
                };
                match other.filter(|o| *o < tab.items.len() && index < tab.items.len()) {
                    Some(o) => {
                        tab.items.swap(index, o);
                        Ok(not_saved("moved the item".to_owned()))
                    }
                    None => Err("preview — the item is already at the end".to_owned()),
                }
            }
            Edit::SetMeta(list) => {
                let n = list.len();
                for (target, m) in list {
                    match target {
                        MetaTarget::Row(id) => {
                            self.row_meta.insert(id, m);
                        }
                        MetaTarget::Library(id) => {
                            if let Some(l) = self.library.iter_mut().find(|l| l.id == id) {
                                l.meta = m;
                            }
                        }
                    }
                }
                Ok(not_saved(format!("changed the tags of {n} items")))
            }
            Edit::Deck { deck, command } => self.deck_command(deck, command),
            Edit::AssignCart { cart, item } => {
                let label = match item {
                    Some(id) => match self.library_item(id) {
                        Some(l) => l.title.clone(),
                        None => return Err("preview — that library item is gone".to_owned()),
                    },
                    None => String::new(),
                };
                let Some(slot) = self.carts.get_mut(cart) else {
                    return Err("preview — no such cart".to_owned());
                };
                let msg = if label.is_empty() {
                    format!("cleared cart {}", cart + 1)
                } else {
                    format!("assigned {label} to cart {}", cart + 1)
                };
                *slot = label;
                Ok(not_saved(msg))
            }
        }
    }

    fn library_ref(&mut self, title: &str, len: Duration) -> Option<u64> {
        let found = self.library.iter().find(|l| {
            l.title == title
                || l.artist
                    .as_ref()
                    .is_some_and(|a| format!("{a} – {}", l.title) == title)
        });
        if let Some(l) = found {
            return Some(l.id);
        }
        let id = self.library.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        self.library.push(LibraryItem {
            id,
            title: title.to_owned(),
            artist: None,
            len,
            category: "Unsorted".to_owned(),
            path: "—".to_owned(),
            meta: meta::from_title(title, PublishMode::FromTags),
        });
        Some(id)
    }

    fn playlist_from_folder(&mut self, folder: FolderSource, files: Vec<String>) -> Option<String> {
        let name = self.unique_name(folders::name_of(&folder.path));
        let colour = FOLDER_COLOURS[self.tabs.len() % FOLDER_COLOURS.len()];
        let items = files
            .iter()
            .map(|f| (folders::stem_of(f).to_owned(), Duration::ZERO))
            .collect();
        let mut tab = playlist(&name, colour, Pick::Shuffle, Vec::new(), items);
        tab.source = Some(folder);
        rules_script::refresh(&mut tab);
        self.tabs.push(tab);
        Some(name)
    }

    fn use_as_main(&mut self, name: &str) -> bool {
        let Some(i) = self.tabs.iter().position(|t| t.name == name) else {
            return false;
        };
        let sample = self.tabs[i].name == SAMPLE;
        if self.tabs[i].items.is_empty() && !sample {
            return false;
        }
        let tab = self.tabs.remove(i);
        let mut old = std::mem::replace(&mut self.main, tab);
        if old.source.is_none() && old.items.is_empty() && old.name == "Music" {
            old.name = SAMPLE.to_owned();
        }
        self.tabs.push(old);
        self.main_tracks = if sample {
            None
        } else {
            Some(
                self.main
                    .items
                    .iter()
                    .map(|(t, l)| (t.clone(), if l.is_zero() { estimated_len(t) } else { *l }))
                    .collect(),
            )
        };
        self.cache = None;
        self.ops.clear();
        true
    }

    fn apply_rules(&mut self, tab: PlaylistTab) -> bool {
        if self.main.name == tab.name {
            self.main = tab;
            return true;
        }
        match self.tabs.iter_mut().find(|t| t.name == tab.name) {
            Some(slot) => {
                *slot = tab;
                true
            }
            None => false,
        }
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
}
