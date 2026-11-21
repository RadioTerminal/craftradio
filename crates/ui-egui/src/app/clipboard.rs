//! The internal clipboard and paste.

use super::*;

/// The internal clipboard: library item ids. `rows` are the rows that a cut removes on paste.
#[derive(Default)]
pub(crate) struct Clipboard {
    pub items: Vec<u64>,
    pub cut: bool,
    pub rows: Vec<u64>,
}

impl RadioApp {
    pub(super) fn insert_items(&mut self, items: &[u64], at: Place) -> Result<String, String> {
        let mut last = Ok(String::new());
        // Inserting in reverse at one place keeps the order of the items.
        let ordered: Vec<u64> = match at {
            Place::End => items.to_vec(),
            _ => items.iter().rev().copied().collect(),
        };
        for item in ordered {
            last = self.source.edit(Edit::Insert { item, at });
            if last.is_err() {
                break;
            }
        }
        last
    }

    pub(super) fn paste(&mut self, ctx: &egui::Context, to: PasteTo) {
        if self.clip.items.is_empty() {
            self.toast(ctx, "preview — the clipboard is empty".to_owned());
            return;
        }
        let items = self.clip.items.clone();
        let result = match to {
            PasteTo::Main(at) => self.insert_items(&items, at),
            PasteTo::Playlist(name) => {
                for item in &items {
                    self.source
                        .drop_item(*item, DropTarget::Playlist(name.clone()));
                }
                Ok(format!(
                    "preview — pasted {} items into {name} (not saved)",
                    items.len()
                ))
            }
            PasteTo::Cart(cart) => self.source.edit(Edit::AssignCart {
                cart,
                item: items.first().copied(),
            }),
        };
        if result.is_ok() && self.clip.cut && !self.clip.rows.is_empty() {
            let rows = std::mem::take(&mut self.clip.rows);
            let _ = self.source.edit(Edit::Remove(rows));
            self.clip.cut = false;
        }
        self.report(ctx, result);
    }
}
