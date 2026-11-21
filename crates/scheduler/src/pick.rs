use radio_model::{Item, ItemId, PickOrder, Playlist};
use rand_chacha::{
    ChaCha8Rng,
    rand_core::{RngCore, SeedableRng},
};

use crate::PlayHistory;

/// State for one playlist. Recreate the picker when its items or pick order change.
#[derive(Clone)]
pub struct Picker {
    cursor: usize,
    permutation: Vec<usize>,
    rng: ChaCha8Rng,
    scores: Vec<i128>,
}

impl Picker {
    pub fn new(pl: &Playlist) -> Self {
        let seed = match pl.pick {
            PickOrder::Shuffle { seed } => seed,
            _ => 0,
        };
        Self {
            cursor: 0,
            permutation: Vec::new(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            scores: vec![0; pl.items.len()],
        }
    }

    /// Continues the order of the playlist after `last`, its most recent item. The next
    /// `InOrder` or `Shuffle` pick is the item after `last`. Other orders already follow
    /// the history, and this call does not change them. An unknown `last` changes nothing.
    pub fn resume_after(&mut self, pl: &Playlist, last: ItemId) {
        let len = pl.items.len();
        let Some(index) = pl.items.iter().position(|i| i.id == last) else {
            return;
        };
        match pl.pick {
            PickOrder::InOrder => self.cursor = (index + 1) % len,
            PickOrder::Shuffle { .. } => {
                if self.permutation.len() != len {
                    self.shuffle(len);
                }
                if let Some(at) = self.permutation.iter().position(|&i| i == index) {
                    self.cursor = at + 1;
                }
            }
            PickOrder::Weighted { .. } | PickOrder::LeastRecentlyPlayed => {}
        }
    }

    fn shuffle(&mut self, len: usize) {
        self.permutation = (0..len).collect();
        for i in (1..len).rev() {
            let bound = (i + 1) as u64;
            let threshold = bound.wrapping_neg() % bound;
            let draw = loop {
                let draw = self.rng.next_u64();
                if draw >= threshold {
                    break draw;
                }
            };
            self.permutation.swap(i, (draw % bound) as usize);
        }
        self.cursor = 0;
    }

    pub fn next<'a>(
        &mut self,
        pl: &'a Playlist,
        hist: &PlayHistory,
        ok: impl Fn(&Item) -> bool,
    ) -> Option<&'a Item> {
        let len = pl.items.len();
        if len == 0 {
            return None;
        }
        match &pl.pick {
            PickOrder::InOrder => {
                for _ in 0..len {
                    let index = self.cursor % len;
                    self.cursor = (index + 1) % len;
                    if ok(&pl.items[index]) {
                        return Some(&pl.items[index]);
                    }
                }
                None
            }
            PickOrder::Shuffle { .. } => {
                let mut visited = vec![false; len];
                let mut remaining = len;
                while remaining > 0 {
                    if self.permutation.len() != len || self.cursor >= len {
                        self.shuffle(len);
                    }
                    let index = self.permutation[self.cursor];
                    self.cursor += 1;
                    if visited[index] {
                        continue;
                    }
                    visited[index] = true;
                    remaining -= 1;
                    if ok(&pl.items[index]) {
                        return Some(&pl.items[index]);
                    }
                }
                None
            }
            PickOrder::LeastRecentlyPlayed => pl
                .items
                .iter()
                .filter(|item| ok(item))
                .min_by_key(|item| hist.last_played(item.id)),
            PickOrder::Weighted { weights } => {
                self.scores.resize(len, 0);
                let mut total = 0i128;
                let mut best = None;
                for (index, item) in pl.items.iter().enumerate() {
                    let weight = weights
                        .iter()
                        .find(|(id, _)| *id == item.id)
                        .map_or(0, |(_, weight)| i128::from(*weight));
                    if weight == 0 || !ok(item) {
                        continue;
                    }
                    total += weight;
                    self.scores[index] += weight;
                    if best.is_none_or(|previous| self.scores[index] > self.scores[previous]) {
                        best = Some(index);
                    }
                }
                best.map(|index| {
                    self.scores[index] -= total;
                    &pl.items[index]
                })
            }
        }
    }
}
