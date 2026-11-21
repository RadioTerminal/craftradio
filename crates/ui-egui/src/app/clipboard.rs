//! The internal clipboard and paste.

use super::*;

/// What a cut removes after a successful paste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CutSource {
    Rows(Vec<u64>),
    PlaylistItem { playlist: String, index: usize },
}

/// The internal clipboard: library item ids that all panes share.
#[derive(Default)]
pub(crate) struct Clipboard {
    pub items: Vec<u64>,
    pub cut: bool,
    /// What the paste removes when `cut` is true.
    pub source: Option<CutSource>,
}

impl RadioApp {
    /// Put library items into the main timeline in their order. It returns the count placed.
    pub(super) fn insert_items(&mut self, items: &[u64], at: Place) -> Result<usize, String> {
        // Inserting in reverse at one place keeps the order of the items.
        let ordered: Vec<u64> = match at {
            Place::End => items.to_vec(),
            _ => items.iter().rev().copied().collect(),
        };
        let mut placed = 0;
        for item in ordered {
            placed += self.source.edit(Edit::Insert { item, at })?;
        }
        Ok(placed)
    }

    /// Put each item on a playlist. It returns the count placed, or the first error.
    fn add_to_playlist(&mut self, items: &[u64], name: &str) -> Result<usize, String> {
        let mut placed = 0;
        for item in items {
            placed += self
                .source
                .drop_item(*item, DropTarget::Playlist(name.to_owned()))?;
        }
        Ok(placed)
    }

    pub(super) fn paste(&mut self, ctx: &egui::Context, to: PasteTo) {
        if self.clip.items.is_empty() {
            self.toast(ctx, "preview — the clipboard is empty".to_owned());
            return;
        }
        let items = self.clip.items.clone();
        let (place, result) = match to {
            PasteTo::Main(at) => (
                self.view.main_playlist.clone(),
                self.insert_items(&items, at),
            ),
            PasteTo::Playlist(name) => {
                let r = self.add_to_playlist(&items, &name);
                (name, r)
            }
            PasteTo::Cart(cart) => (
                format!("cart {}", cart + 1),
                self.source.edit(Edit::AssignCart {
                    cart,
                    item: items.first().copied(),
                }),
            ),
        };
        let mut text = match &result {
            Ok(n) => format!("preview — pasted {n} items into {place} (not saved)"),
            Err(e) => e.clone(),
        };
        // A cut removes its source only after the paste worked.
        if result.is_ok() && self.clip.cut {
            let removed = match self.clip.source.take() {
                Some(CutSource::Rows(rows)) => self.source.edit(Edit::Remove(rows)),
                Some(CutSource::PlaylistItem { playlist, index }) => self
                    .source
                    .edit(Edit::RemovePlaylistItem { playlist, index }),
                None => Ok(0),
            };
            self.clip.cut = false;
            if let Err(e) = removed {
                text = format!("{text} · the cut source stays: {e}");
            }
        }
        self.toast(ctx, text);
    }
}
