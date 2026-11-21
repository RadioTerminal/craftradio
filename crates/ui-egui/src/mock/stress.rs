//! A stress data set for visual checks: long text in many scripts, many carts and rows.
//! We invented every name.

use super::*;

/// The hours of timeline that give about 3000 rows.
const STRESS_HOURS: i64 = 210;
/// The number of carts, so cart numbers have four digits.
const STRESS_CARTS: usize = 1200;
const STRESS_WARNINGS: usize = 99;
/// Twelve hours, the length of a long show.
const TWELVE_HOURS: Duration = Duration::from_secs(12 * 3600);

/// Long titles in several scripts. They replace every third title.
pub const STRESS_TITLES: [&str; 6] = [
    "The Extraordinarily Long-Named Midnight Orchestra of the Southern Valleys – A Song Whose Title Never Ends",
    "夜明けの電車と遠い海の物語 – 静かな街の長い長い夜のための歌と、もう一つの長い副題",
    "Η Ορχήστρα των Βορείων Ανέμων – Το Τραγούδι της Μακριάς Νύχτας και του Πρωινού",
    "فرقة الرياح الشمالية الطويلة جدا – أغنية الليل الطويل والصباح البعيد",
    "🎧 Night Owls 🌙✨ – Dancing 💃 Until 🌅 Dawn 🎶🎶🎶 (Extended Sunrise Mix)",
    "Ünïcödé Çhäracters Ëverywhere – Søren Ærø Œuvre Ñoño Łódź Ğüneş (Live At The Very Long Hall)",
];

/// A long artist name for the library and the decks.
const STRESS_ARTIST: &str =
    "The Extraordinarily Long-Named Midnight Orchestra of the Southern Valleys";
/// A suffix that makes playlist names very long.
const LONG_NAME: &str = " — Late Night Weekend Programming For Insomniacs";

/// An internet address of 200 characters.
pub fn stress_url() -> String {
    let mut url = "https://stream.example.org/".to_owned();
    while url.len() < 196 {
        url.push_str("very-long-path-");
    }
    url.truncate(196);
    url.push_str(".mp3");
    url
}

/// The mock with the stress data on top.
pub struct StressSource {
    inner: MockSource,
}

impl StressSource {
    pub fn new(tz: TimeZone) -> Self {
        Self {
            inner: MockSource::with_hours(tz, STRESS_HOURS),
        }
    }
}

/// Puts the stress data into a snapshot.
pub fn stress(v: &mut StationView) {
    let title = |i: usize| STRESS_TITLES[i % STRESS_TITLES.len()].to_owned();
    let mut renames: Vec<(String, String)> = Vec::new();
    for tab in &mut v.tabs {
        let new = format!("{}{LONG_NAME}", tab.name);
        renames.push((tab.name.clone(), new.clone()));
        tab.name = new;
        for (k, item) in tab.items.iter_mut().enumerate() {
            if k % 2 == 0 {
                item.0 = title(k);
            }
        }
    }
    let main = format!("{}{LONG_NAME}", v.main.name);
    renames.push((v.main.name.clone(), main.clone()));
    v.main.name.clone_from(&main);
    v.main_playlist = main;
    let rename = |name: &mut String| {
        if let Some((_, new)) = renames.iter().find(|(old, _)| old == name) {
            name.clone_from(new);
        }
    };
    for (i, r) in v.rows.iter_mut().chain(v.history.iter_mut()).enumerate() {
        rename(&mut r.playlist);
        if i % 3 == 0 {
            r.title = title(i / 3);
            r.meta.title = r.title.clone();
            r.meta.artist = STRESS_ARTIST.to_owned();
        }
    }
    // A twelve-hour show and a stream with a long address, early in the timeline.
    if let Some(r) = v.rows.get_mut(4) {
        r.len = TWELVE_HOURS;
        r.title = "The Twelve Hour Marathon Show With Guests From Everywhere".to_owned();
    }
    if let Some(r) = v.rows.get_mut(5) {
        r.title = stress_url();
        r.meta.keep_alive = Some(true);
        r.stream = Some(crate::view::StreamState::Reconnecting {
            attempt: 1234,
            next_in_s: 9999,
        });
    }
    for (i, d) in v.decks.iter_mut().enumerate() {
        d.title = title(i);
    }
    v.decks[0].len = TWELVE_HOURS;
    v.decks[0].elapsed = TWELVE_HOURS - Duration::from_secs(1);
    for (i, l) in v.library.iter_mut().enumerate() {
        if i % 3 == 0 {
            l.title = title(i / 3);
            l.artist = Some(STRESS_ARTIST.to_owned());
            l.category = format!("{}{LONG_NAME}", l.category);
        }
    }
    if let Some(l) = v.library.get_mut(1) {
        l.path = stress_url();
        l.len = TWELVE_HOURS;
    }
    v.carts = (0..STRESS_CARTS)
        .map(|i| format!("Cart {} · {}", i + 1000, title(i)))
        .collect();
    let per = v.cartwall.per_page.max(1);
    let pages = STRESS_CARTS.div_ceil(per);
    v.cartwall.pages = (1..=pages)
        .map(|p| format!("Page {p}{LONG_NAME}"))
        .collect();
    v.cartwall.slots = vec![
        CartSlot {
            duck_db: -12.0,
            mode: crate::view::CartMode::Restart,
            colour: Some(2),
        };
        STRESS_CARTS
    ];
    v.warnings = STRESS_WARNINGS;
    v.station = format!("Radio{LONG_NAME}");
    let files: Vec<LibraryItem> = v.library.iter().take(40).cloned().collect();
    v.folders = vec![
        FolderView {
            path: "/Volumes/A Very Long Drive Name/Music/Archive/2026/Late Night Weekend Programming"
                .to_owned(),
            files,
            truncated: true,
            errors: Vec::new(),
        },
        FolderView {
            path: "/Volumes/Missing Drive/Music".to_owned(),
            files: Vec::new(),
            truncated: false,
            errors: vec![
                "cannot read /Volumes/Missing Drive/Music: No such file or directory (os error 2)"
                    .to_owned(),
                "cannot read /Volumes/Missing Drive/Music/Sub folder with a long name: Permission denied (os error 13)"
                    .to_owned(),
            ],
        },
    ];
}

impl ViewSource for StressSource {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let mut v = self.inner.snapshot(now);
        stress(&mut v);
        v
    }

    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        self.inner.edit(edit)
    }

    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
}
