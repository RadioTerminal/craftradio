use jiff::Timestamp;
use radio_model::{ItemId, PlaylistId};

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
}

impl PlayHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, p: Played) {
        let index = self.entries.partition_point(|entry| entry.at <= p.at);
        self.entries.insert(index, p);
    }

    pub fn last_played(&self, item: ItemId) -> Option<Timestamp> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.item == item)
            .map(|entry| entry.at)
    }

    pub fn count_since(&self, pred: impl Fn(&Played) -> bool, since: Timestamp) -> usize {
        self.iter_since(since).filter(|entry| pred(entry)).count()
    }

    pub fn iter_since(&self, since: Timestamp) -> impl Iterator<Item = &Played> {
        let index = self.entries.partition_point(|entry| entry.at < since);
        self.entries[index..].iter()
    }
}
