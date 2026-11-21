//! A mock view source for the UI preview. It makes a fake timeline. It plays no audio.
//!
//! We invented all artists and titles.

mod data;
mod decks;
mod source;
mod tabs;
mod timeline;

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

use data::*;
use decks::*;
use tabs::*;
use timeline::*;

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
