//! Shared folder reconciliation for startup and watched changes.
use radio_engine::Command;
use radio_model::{FolderSource, Source};
use std::path::Path;

pub fn reconcile(
    state: &radio_engine::State,
    mut apply: impl FnMut(Command) -> Result<(), String>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let playlists: Vec<(radio_model::PlaylistId, String, FolderSource)> = state
        .station
        .playlists
        .iter()
        .filter_map(|p| {
            let src = p.source.clone().or_else(|| {
                state
                    .source(p.id)
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
            })?;
            src.watch.then(|| (p.id, p.name.clone(), src))
        })
        .collect();
    let mut next = state
        .station
        .playlists
        .iter()
        .flat_map(|p| p.items.iter().map(|i| i.id.0))
        .max()
        .unwrap_or(0)
        + 1;
    for (playlist, name, src) in playlists {
        let list = radio_library::list_audio(
            Path::new(&src.path),
            src.recursive,
            radio_library::Limits::DEFAULT,
        );
        let healthy = Path::new(&src.path).is_dir() && list.errors.is_empty();

        let Some(p) = state.station.playlists.iter().find(|p| p.id == playlist) else {
            continue;
        };
        let files: Vec<(radio_model::ItemId, String)> = p
            .items
            .iter()
            .filter_map(|i| match &i.source {
                Source::File { path } => Some((i.id, path.clone())),
                _ => None,
            })
            .collect();
        let (present, gone): (Vec<_>, Vec<_>) = files
            .iter()
            .cloned()
            .partition(|(_, f)| Path::new(f).is_file());
        let mut cmds = Vec::new();
        // Files that came back.
        let back: Vec<_> = present
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| state.missing.contains(id))
            .collect();
        if !back.is_empty() {
            cmds.push(Command::ItemMarkMissing {
                items: back.clone(),
                missing: false,
            });
        }
        // A hash for each file that has none yet, for a later relink.
        for (id, f) in &present {
            if !state.hashes.contains_key(id)
                && let Ok((hash, size)) = radio_library::content_hash(f)
            {
                cmds.push(Command::ItemSetHash {
                    item: *id,
                    hash: hex(&hash),
                    size,
                });
            }
        }
        // Gone files that moved inside the folder.
        let wanted: Vec<(radio_model::ItemId, [u8; 32], u64)> = gone
            .iter()
            .filter_map(|(id, _)| {
                let (h, size) = state.hashes.get(id)?;
                Some((*id, unhex(h)?, *size))
            })
            .collect();
        let relinked = if healthy && !wanted.is_empty() {
            radio_library::relink(&wanted, std::slice::from_ref(&src))
        } else {
            Vec::new()
        };
        for (item, path) in &relinked {
            cmds.push(Command::ItemRelink {
                item: *item,
                path: path.clone(),
            });
        }
        let still_gone: Vec<radio_model::ItemId> = gone
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !relinked.iter().any(|(r, _)| r == id))
            .collect();
        let newly_missing: Vec<_> = still_gone
            .iter()
            .copied()
            .filter(|id| !state.missing.contains(id))
            .collect();
        if !healthy && !newly_missing.is_empty() {
            cmds.push(Command::ItemMarkMissing {
                items: newly_missing.clone(),
                missing: true,
            });
        }
        if healthy && !still_gone.is_empty() {
            cmds.push(Command::PlaylistRemoveItems {
                playlist,
                items: still_gone.clone(),
            });
        }
        // New files in the folder.
        let known: Vec<&str> = files
            .iter()
            .map(|(_, f)| f.as_str())
            .chain(relinked.iter().map(|(_, f)| f.as_str()))
            .collect();
        let mut items = Vec::new();
        for f in list.files.iter().filter(|f| !known.contains(&f.as_str())) {
            if let Some(mut item) = crate::file_item(f) {
                item.id = radio_model::ItemId(next);
                next += 1;
                items.push(item);
            }
        }
        let added = items.len();
        if added > 0 {
            cmds.push(Command::PlaylistAddItems {
                playlist,
                items,
                at: None,
                lock: false,
            });
        }
        if cmds.is_empty() {
            continue;
        }
        if let Err(e) = apply(Command::StationSystem { commands: cmds }) {
            lines.push(format!("{name}: cannot catch up with the folder: {e}"));
            continue;
        }
        let mut parts = Vec::new();
        if added > 0 {
            parts.push(format!("{added} new files"));
        }
        if !relinked.is_empty() {
            parts.push(format!("{} relinked", relinked.len()));
        }
        if !back.is_empty() {
            parts.push(format!("{} found again", back.len()));
        }
        if healthy && !still_gone.is_empty() {
            parts.push(format!("{} gone", still_gone.len()));
        }
        if !healthy && !still_gone.is_empty() {
            parts.push(format!(
                "{} missing (the folder does not read)",
                still_gone.len()
            ));
        }
        if !parts.is_empty() {
            lines.push(format!("{name}: {}", parts.join(", ")));
        }
    }
    lines
}

/// Removes the new items that the playlist holds now. It gives new ids to the other items.
///
/// The background lane reconciles an old snapshot. `added` holds the files of this round.
pub fn settle(
    state: &radio_engine::State,
    cmd: Command,
    added: &mut Vec<(radio_model::PlaylistId, String)>,
    fresh: &mut impl FnMut(Vec<radio_model::Item>) -> Vec<radio_model::Item>,
) -> Option<Command> {
    match cmd {
        Command::StationSystem { commands } => {
            let commands: Vec<Command> = commands
                .into_iter()
                .filter_map(|c| settle(state, c, added, fresh))
                .collect();
            (!commands.is_empty()).then_some(Command::StationSystem { commands })
        }
        Command::PlaylistAddItems {
            playlist,
            items,
            at,
            lock,
        } => {
            let held = |path: &str| {
                state
                    .station
                    .playlists
                    .iter()
                    .filter(|p| p.id == playlist)
                    .flat_map(|p| p.items.iter())
                    .any(|i| matches!(&i.source, Source::File { path: p } if p == path))
            };
            let mut kept = Vec::new();
            for item in items {
                if let Source::File { path } = &item.source {
                    let key = (playlist, path.clone());
                    if held(path) || added.contains(&key) {
                        continue;
                    }
                    added.push(key);
                }
                kept.push(item);
            }
            (!kept.is_empty()).then(|| Command::PlaylistAddItems {
                playlist,
                items: fresh(kept),
                at,
                lock,
            })
        }
        other => Some(other),
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if text.len() != 64 {
        return None;
    }
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

use crate::EngineSource;
use crate::folders::push_bounded;
use radio_library::Watcher;
use radio_model::{FolderSource as MFolder, PlaylistId};
use std::collections::HashMap;

impl EngineSource {
    /// Starts and stops the watches of the folder playlists that have `watch` on.
    pub(crate) fn sync_watches(&mut self) {
        let mut wanted: HashMap<PlaylistId, MFolder> = HashMap::new();
        for p in &self.state.station.playlists {
            let src = p.source.clone().or_else(|| {
                self.state
                    .source(p.id)
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
            });
            if let Some(src) = src.filter(|s: &MFolder| s.watch) {
                wanted.insert(p.id, src);
            }
        }
        self.watches
            .retain(|id, (src, _)| wanted.get(id).is_some_and(|w| w == src));
        for (id, src) in wanted {
            if self.watches.contains_key(&id) {
                continue;
            }
            match Watcher::new(&src) {
                Ok(w) => {
                    self.watches.insert(id, (src, w));
                }
                Err(e) => push_bounded(
                    &mut self.watch_errors,
                    format!("cannot watch {}: {e}", src.path),
                ),
            }
        }
    }

    /// Applies the settled changes of watched folders to their playlists.
    pub(crate) fn poll_watches(&mut self) {
        let mut changed = false;
        for (_, w) in self.watches.values_mut() {
            match w.events() {
                Ok(mut events) => changed |= events.next().is_some(),
                Err(e) => push_bounded(&mut self.watch_errors, e.to_string()),
            }
        }
        if changed && let Some(worker) = &self.worker {
            let _ = worker
                .jobs
                .send(crate::bg::Job::Reconcile(self.state.clone()));
        }
    }
}
