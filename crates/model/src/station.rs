use crate::{Item, ItemId, ModelError, Playlist, PlaylistId, Source};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    /// IANA time zone name.
    pub tz: String,
    pub playlists: Vec<Playlist>,
}

impl Station {
    /// Checks structural invariants. Order: time zone, main playlist, duplicate ids, items.
    pub fn validate(&self) -> Result<(), ModelError> {
        if jiff::tz::TimeZone::get(&self.tz).is_err() {
            return Err(ModelError::UnknownTimeZone(self.tz.clone()));
        }
        match self.playlists.iter().filter(|p| p.is_main).count() {
            0 => return Err(ModelError::NoMainPlaylist),
            1 => {}
            _ => return Err(ModelError::MultipleMainPlaylists),
        }
        let mut playlist_ids = BTreeSet::<PlaylistId>::new();
        let mut item_ids = BTreeSet::<ItemId>::new();
        for p in &self.playlists {
            if !playlist_ids.insert(p.id) {
                return Err(ModelError::DuplicateId(p.id.0));
            }
            for item in &p.items {
                if !item_ids.insert(item.id) {
                    return Err(ModelError::DuplicateId(item.id.0));
                }
            }
        }
        self.playlists
            .iter()
            .flat_map(|p| &p.items)
            .try_for_each(validate_item)
    }

    /// The main playlist. Panics if there is none; call [`Station::validate`] first.
    pub fn main(&self) -> &Playlist {
        self.playlists
            .iter()
            .find(|p| p.is_main)
            .expect("station has a main playlist (call validate first)")
    }
}

fn validate_item(item: &Item) -> Result<(), ModelError> {
    if let Some(t) = &item.timing {
        if t.segue < t.cue_in {
            return Err(ModelError::BadTiming {
                item: item.id,
                reason: "segue before cue_in",
            });
        }
        if t.cue_out < t.segue {
            return Err(ModelError::BadTiming {
                item: item.id,
                reason: "cue_out before segue",
            });
        }
    }
    let needs_len = match item.source {
        Source::File { .. } => item.timing.is_none(),
        Source::LiveInput { .. } | Source::Stream { .. } => true,
    };
    if needs_len && item.planned_len.is_none() {
        return Err(ModelError::MissingPlannedLen(item.id));
    }
    Ok(())
}
