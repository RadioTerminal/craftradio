//! A mock view source for the UI preview. It makes a fake timeline. It plays no audio.
//!
//! We invented all artists and titles.

use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan, Zoned};

use crate::view::{DeckState, DeckView, FitView, PlaylistTab, Row, StationView, ViewSource};

const MUSIC: [u8; 3] = [0x5B, 0xAA, 0x5B];
const JINGLES: [u8; 3] = [0xE0, 0xA0, 0x30];
const ADS: [u8; 3] = [0xD0, 0x48, 0x48];
const NEWS: [u8; 3] = [0x4A, 0x90, 0xD9];
const SHOWS: [u8; 3] = [0x8E, 0x6A, 0xD8];
const BEDS: [u8; 3] = [0x3F, 0xB5, 0xA8];

const HOURS: i64 = 6;
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
const AD_NAMES: [&str; 6] = [
    "Kesten Bakery",
    "Lumo Bikes",
    "Orrin Hardware",
    "Pavla Opticians",
    "Riverside Market",
    "Tamsin Travel",
];
const CARTS: [&str; 16] = [
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
];

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// The mock source. It builds about six hours of rows from the top of the current hour.
pub struct MockSource {
    tz: TimeZone,
    tracks: Vec<(String, Duration)>,
    cache: Option<(Timestamp, Vec<Row>)>,
}

impl MockSource {
    pub fn new(tz: TimeZone) -> Self {
        let tracks = (0..TRACKS)
            .map(|i| {
                let artist = ARTISTS[i % ARTISTS.len()];
                let n = (i * 7) % (NOUNS.len() * PLACES.len());
                let mut title =
                    format!("{} of {}", NOUNS[n % NOUNS.len()], PLACES[n / NOUNS.len()]);
                if i >= NOUNS.len() * PLACES.len() {
                    title.push_str(" (Night Version)");
                }
                // Lengths run from 2:30 to 5:00.
                let len = 150 + (i as u64 * 37) % 151;
                (format!("{artist} – {title}"), secs(len))
            })
            .collect();
        Self {
            tz,
            tracks,
            cache: None,
        }
    }

    fn rows_from(&mut self, top: &Zoned) -> &[Row] {
        let key = top.timestamp();
        let stale = self.cache.as_ref().is_none_or(|(k, _)| *k != key);
        if stale {
            let mut rows = Vec::new();
            let mut hour = top.clone();
            for _ in 0..HOURS {
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

fn tabs() -> Vec<PlaylistTab> {
    let tab = |name: &str, color, pick: &str, rules: &[&str], items: Vec<(String, Duration)>| {
        PlaylistTab {
            name: name.to_owned(),
            color,
            pick: pick.to_owned(),
            rules: rules.iter().map(|r| (*r).to_owned()).collect(),
            items,
        }
    };
    vec![
        tab(
            "Jingles",
            JINGLES,
            "rotate in order",
            &["Every 3 tracks · not back-to-back"],
            (1..=3)
                .map(|n| (format!("Station ID {n}"), secs(6)))
                .collect(),
        ),
        tab(
            "Ads",
            ADS,
            "in order",
            &[":20 and :50 · take 3 · never trimmed"],
            AD_NAMES
                .iter()
                .map(|n| ((*n).to_owned(), secs(30)))
                .collect(),
        ),
        tab(
            "News",
            NEWS,
            "latest stream",
            &[
                "Every full hour · fixed start",
                "Music before it is cut to fit",
            ],
            vec![("News".to_owned(), secs(5 * 60))],
        ),
        tab(
            "Shows",
            SHOWS,
            "by date",
            &[
                "16:00 daily · fixed start",
                "Takes the place of the 16:00 news",
            ],
            vec![("Jazz Hour.mp3".to_owned(), secs(58 * 60 + 40))],
        ),
        tab(
            "Beds",
            BEDS,
            "shuffle",
            &["Under talk only · loop until stopped"],
            vec![
                ("Bed 1 · Warm Pad".to_owned(), secs(120)),
                ("Bed 2 · Soft Pulse".to_owned(), secs(95)),
                ("Bed 3 · Night Drive".to_owned(), secs(140)),
            ],
        ),
    ]
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
        let rows = self.rows_from(&top).to_vec();

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

        StationView {
            now: local,
            mode: "AUTO".to_owned(),
            decks,
            rows,
            tabs: tabs(),
            carts: CARTS.iter().map(|c| (*c).to_owned()).collect(),
            warnings: 0,
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
