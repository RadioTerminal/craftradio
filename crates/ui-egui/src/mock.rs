//! A mock view source for the UI preview. It makes a fake timeline. It plays no audio.
//!
//! We invented all artists and titles.

use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan, Zoned};

use crate::rules_script;
use crate::view::{
    Constraint, ConstraintKind, DeckState, DeckView, DropTarget, FitView, LibraryItem, MeterView,
    Origin, Pick, Placement, PlaylistTab, Row, RuleView, StationView, Trigger, ViewSource,
};

const MUSIC: [u8; 3] = [0x5B, 0xAA, 0x5B];
const JINGLES: [u8; 3] = [0xE0, 0xA0, 0x30];
const ADS: [u8; 3] = [0xD0, 0x48, 0x48];
const NEWS: [u8; 3] = [0x4A, 0x90, 0xD9];
const SHOWS: [u8; 3] = [0x8E, 0x6A, 0xD8];
const BEDS: [u8; 3] = [0x3F, 0xB5, 0xA8];

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
    /// Library items dropped into the main playlist: after which row, and which item.
    inserts: Vec<(u64, u64)>,
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
            inserts: Vec::new(),
        }
    }

    /// Put the dropped library items into `rows`. Items that do not fit stay out.
    fn apply_inserts(&self, rows: &mut Vec<Row>) {
        for (n, (after, item)) in self.inserts.iter().enumerate() {
            let Some(lib) = self.library.iter().find(|l| l.id == *item) else {
                continue;
            };
            let title = match &lib.artist {
                Some(a) => format!("{a} – {}", lib.title),
                None => lib.title.clone(),
            };
            let len = if lib.len.is_zero() {
                secs(180)
            } else {
                lib.len
            };
            insert_manual(rows, *after, MANUAL_ID + n as u64, title, len, &self.tz);
        }
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
            *t += span;
            rows.push(Row {
                id: start.timestamp().as_millisecond().unsigned_abs(),
                start,
                playlist: item.playlist.to_owned(),
                color: item.color,
                title: item.title,
                len: item.len,
                is_main: item.playlist == "Music",
                fixed: item.fixed,
                origin: if item.playlist == "Music" {
                    Origin::Main
                } else {
                    Origin::Rule
                },
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
            let (title, len) = &self.tracks[track % TRACKS];
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

/// Insert a manual row after the row `after`. The music before the next fixed row gets shorter,
/// so every fixed row keeps its start. It returns false when the item does not fit.
fn insert_manual(
    rows: &mut Vec<Row>,
    after: u64,
    id: u64,
    title: String,
    len: Duration,
    tz: &TimeZone,
) -> bool {
    let Some(i) = rows.iter().position(|r| r.id == after) else {
        return false;
    };
    let fixed = rows[i + 1..]
        .iter()
        .position(|r| r.fixed.is_some())
        .map_or(rows.len(), |k| i + 1 + k);
    let room: Duration = rows[i + 1..fixed]
        .iter()
        .filter(|r| r.origin == Origin::Main)
        .map(|r| r.len)
        .sum();
    if room < len {
        return false;
    }
    let mut owed = len;
    for r in rows[i + 1..fixed].iter_mut().rev() {
        if owed.is_zero() {
            break;
        }
        if r.origin == Origin::Main {
            let cut = owed.min(r.len);
            r.len -= cut;
            owed -= cut;
        }
    }
    let mut row = rows[i].clone();
    row.id = id;
    row.playlist = "Music".to_owned();
    row.color = MUSIC;
    row.title = title;
    row.len = len;
    row.is_main = true;
    row.fixed = None;
    row.origin = Origin::Manual;
    rows.insert(i + 1, row);
    let fixed = fixed + 1;
    let mut j = i + 1;
    let mut end = fixed;
    while j < end {
        if rows[j].len.is_zero() {
            rows.remove(j);
            end -= 1;
            continue;
        }
        let prev = &rows[j - 1];
        let start = SignedDuration::try_from(prev.len)
            .map_or(prev.start.timestamp(), |d| prev.start.timestamp() + d);
        rows[j].start = start.to_zoned(tz.clone());
        j += 1;
    }
    true
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
        let mut all = self.rows_from(&top).to_vec();
        self.apply_inserts(&mut all);
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
        let decks = [
            deck('A', at(0), DeckState::Playing, elapsed),
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
            DropTarget::Main { after } => {
                self.inserts.push((after, item));
                true
            }
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

    fn import_files(&mut self, paths: Vec<String>) {
        for path in paths {
            let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
            let id = self.library.iter().map(|l| l.id).max().unwrap_or(0) + 1;
            self.library.push(LibraryItem {
                id,
                title: name,
                artist: None,
                len: Duration::ZERO,
                category: "Unsorted".to_owned(),
                path,
            });
        }
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
