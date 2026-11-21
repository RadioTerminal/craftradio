use jiff::Timestamp;
use radio_model::{ItemId, PlaylistId};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Played {
    pub item: ItemId,
    pub playlist: PlaylistId,
    pub at: Timestamp,
    pub artist: Option<String>,
    pub title: String,
}

#[derive(Debug, Clone, Default)]
pub struct PlayHistory {
    entries: Vec<Played>,
    items: BTreeMap<ItemId, Vec<Timestamp>>,
    artists: BTreeMap<String, Vec<Timestamp>>,
    titles: BTreeMap<String, Vec<Timestamp>>,
    playlists: BTreeMap<PlaylistId, Vec<Timestamp>>,
}

impl PlayHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, p: Played) {
        fn insert(times: &mut Vec<Timestamp>, at: Timestamp) {
            let index = times.partition_point(|t| *t <= at);
            times.insert(index, at);
        }
        insert(self.items.entry(p.item).or_default(), p.at);
        insert(self.playlists.entry(p.playlist).or_default(), p.at);
        if let Some(artist) = &p.artist {
            insert(self.artists.entry(artist.clone()).or_default(), p.at);
        }
        insert(self.titles.entry(p.title.clone()).or_default(), p.at);
        let index = self.entries.partition_point(|entry| entry.at <= p.at);
        self.entries.insert(index, p);
    }

    pub fn last_played(&self, item: ItemId) -> Option<Timestamp> {
        self.items
            .get(&item)
            .and_then(|times| times.last())
            .copied()
    }

    pub(crate) fn artist_after(&self, artist: &str, cutoff: Timestamp) -> bool {
        self.artists
            .get(artist)
            .is_some_and(|times| times.last().is_some_and(|t| *t > cutoff))
    }

    pub(crate) fn title_after(&self, title: &str, cutoff: Timestamp) -> bool {
        self.titles
            .get(title)
            .is_some_and(|times| times.last().is_some_and(|t| *t > cutoff))
    }

    pub(crate) fn playlist_since(&self, playlist: PlaylistId, since: Timestamp) -> &[Timestamp] {
        let times = self.playlists.get(&playlist).map_or(&[][..], Vec::as_slice);
        &times[times.partition_point(|t| *t < since)..]
    }

    pub(crate) fn item_since(&self, item: ItemId, since: Timestamp) -> &[Timestamp] {
        let times = self.items.get(&item).map_or(&[][..], Vec::as_slice);
        &times[times.partition_point(|t| *t < since)..]
    }

    pub fn count_since(&self, pred: impl Fn(&Played) -> bool, since: Timestamp) -> usize {
        self.iter_since(since).filter(|entry| pred(entry)).count()
    }

    pub fn iter_since(&self, since: Timestamp) -> impl Iterator<Item = &Played> {
        let index = self.entries.partition_point(|entry| entry.at < since);
        self.entries[index..].iter()
    }
}

/// Borrowed history plus the small set of plays in one speculative check.
pub(crate) struct HistoryOverlay<'a> {
    pub base: &'a PlayHistory,
    pub scratch: PlayHistory,
}

pub(crate) trait HistoryRead {
    fn last_played(&self, item: ItemId) -> Option<Timestamp>;
    fn artist_after(&self, artist: &str, cutoff: Timestamp) -> bool;
    fn title_after(&self, title: &str, cutoff: Timestamp) -> bool;
    fn playlist_since(&self, playlist: PlaylistId, since: Timestamp) -> [&[Timestamp]; 2];
    fn item_since(&self, item: ItemId, since: Timestamp) -> [&[Timestamp]; 2];
}

impl HistoryRead for PlayHistory {
    fn last_played(&self, item: ItemId) -> Option<Timestamp> {
        self.last_played(item)
    }
    fn artist_after(&self, artist: &str, cutoff: Timestamp) -> bool {
        self.artist_after(artist, cutoff)
    }
    fn title_after(&self, title: &str, cutoff: Timestamp) -> bool {
        self.title_after(title, cutoff)
    }
    fn playlist_since(&self, playlist: PlaylistId, since: Timestamp) -> [&[Timestamp]; 2] {
        [self.playlist_since(playlist, since), &[]]
    }
    fn item_since(&self, item: ItemId, since: Timestamp) -> [&[Timestamp]; 2] {
        [self.item_since(item, since), &[]]
    }
}

impl HistoryRead for HistoryOverlay<'_> {
    fn last_played(&self, item: ItemId) -> Option<Timestamp> {
        self.base
            .last_played(item)
            .max(self.scratch.last_played(item))
    }
    fn artist_after(&self, artist: &str, cutoff: Timestamp) -> bool {
        self.base.artist_after(artist, cutoff) || self.scratch.artist_after(artist, cutoff)
    }
    fn title_after(&self, title: &str, cutoff: Timestamp) -> bool {
        self.base.title_after(title, cutoff) || self.scratch.title_after(title, cutoff)
    }
    fn playlist_since(&self, playlist: PlaylistId, since: Timestamp) -> [&[Timestamp]; 2] {
        [
            self.base.playlist_since(playlist, since),
            self.scratch.playlist_since(playlist, since),
        ]
    }
    fn item_since(&self, item: ItemId, since: Timestamp) -> [&[Timestamp]; 2] {
        [
            self.base.item_since(item, since),
            self.scratch.item_since(item, since),
        ]
    }
}
