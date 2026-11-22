//! The folders of the library, the files that no playlist holds yet, and watched folders.
//!
//! A worker thread lists and probes folders, so a large folder never stops the UI.

use std::path::Path;

use radio_engine::Command;
use radio_library::Listing;
use radio_model::{Item, PlaylistId};
use radio_ui_egui::view::{FolderSource, FolderView, ItemMeta, LibraryItem};

use crate::EngineSource;
use crate::map::{effective_meta, item_len, view_meta};

/// Library ids of files that no playlist holds start here, above every item id.
const FILE_IDS: u64 = 1 << 41;

/// A library file that no playlist holds yet.
#[derive(Clone, Debug)]
pub(crate) struct LibFile {
    pub(crate) id: u64,
    pub(crate) item: Item,
    pub(crate) view: LibraryItem,
    /// The library folder that the file comes from.
    pub(crate) folder: Option<String>,
}

/// A folder of the library and its last listing.
#[derive(Clone, Debug, Default)]
pub(crate) struct Folder {
    pub(crate) path: String,
    pub(crate) listing: Listing,
    pub(crate) reading: bool,
}

/// The name of a folder: its last path part.
fn name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_owned()
}

impl EngineSource {
    /// Adds a file to the library, or finds it there. It returns the library id.
    pub(crate) fn add_file(&mut self, item: Item, category: &str) -> u64 {
        self.add_file_from(item, category, None)
    }

    fn add_file_from(&mut self, item: Item, category: &str, folder: Option<&str>) -> u64 {
        let path = crate::source_text(&item.source);
        if let Some(l) = self.library.iter().find(|l| l.path == path) {
            return l.id;
        }
        if let Some(f) = self.files.iter().find(|f| f.view.path == path) {
            return f.id;
        }
        self.next_file += 1;
        let id = FILE_IDS + self.next_file;
        let meta = effective_meta(&item, None);
        let view = LibraryItem {
            id,
            title: item.title.clone(),
            artist: item.artist.clone(),
            len: item_len(&item),
            category: category.to_owned(),
            path,
            meta: view_meta(&item, &meta, None),
        };
        self.files.push(LibFile {
            id,
            item,
            view,
            folder: folder.map(str::to_owned),
        });
        id
    }

    /// Sets the tags of a library file that no playlist holds.
    pub(crate) fn set_file_meta(&mut self, id: u64, meta: &ItemMeta) {
        if let Some(f) = self.files.iter_mut().find(|f| f.id == id) {
            f.item.meta = crate::map::model_meta(meta);
            f.view.meta = meta.clone();
        }
    }

    /// Asks the worker to read a folder.
    fn queue_scan(&mut self, path: &str) -> Result<(), String> {
        let worker = self
            .worker
            .as_ref()
            .ok_or("The folder reader could not start")?;
        worker
            .jobs
            .send(crate::bg::Job::Scan {
                path: path.to_owned(),
                limits: self.limits,
            })
            .map_err(|_| "The folder reader has stopped".to_owned())?;
        match self.folders.iter_mut().find(|f| f.path == path) {
            Some(f) => f.reading = true,
            None => self.folders.push(Folder {
                path: path.to_owned(),
                listing: Listing::default(),
                reading: true,
            }),
        }
        Ok(())
    }

    /// Takes the results of the background lane: folder reads, probes, keychain answers and
    /// the as-played log.
    pub(crate) fn take_done(&mut self, now: jiff::Timestamp) {
        let done: Vec<crate::bg::Done> = match &self.worker {
            Some(w) => w.done.try_iter().collect(),
            None => Vec::new(),
        };
        for d in done {
            match d {
                crate::bg::Done::Reconciled { commands, lines } => {
                    let mut failed = false;
                    let mut added = Vec::new();
                    let state = self.state.clone();
                    let commands: Vec<_> = commands
                        .into_iter()
                        .filter_map(|c| {
                            crate::folder_sync::settle(&state, c, &mut added, &mut |items| {
                                self.copies(items)
                            })
                        })
                        .collect();
                    for command in commands {
                        if let Err(e) = self.run_edit(command) {
                            self.watch_errors.push(e);
                            failed = true;
                        }
                    }
                    if !failed {
                        self.notices.extend(lines);
                    }
                }
                crate::bg::Done::Scan {
                    path,
                    listing,
                    items,
                    unreadable,
                } => self.scanned(path, listing, items, unreadable),
                crate::bg::Done::Probed {
                    req,
                    items,
                    skipped,
                    errors,
                } => self.probed(req, items, skipped, &errors),
                crate::bg::Done::Secret { key, wrote, result } => {
                    self.secret_answer(key, wrote, result);
                }
                crate::bg::Done::History(rows) => match rows {
                    Ok(rows) => {
                        self.history = rows.iter().map(|r| self.history_row(r, now)).collect();
                    }
                    Err(e) => self
                        .notices
                        .push(format!("The as-played log did not read: {e}")),
                },
                crate::bg::Done::Cover { path, bytes } => {
                    self.covers.insert(path, bytes);
                }
                crate::bg::Done::Exported {
                    path,
                    count,
                    result,
                } => self.notices.push(match result {
                    Ok(()) => format!("Wrote {count} alarms to {path}"),
                    Err(e) => format!("The alarms were not written to {path}: {e}"),
                }),
            }
        }
    }

    fn scanned(&mut self, path: String, listing: Listing, items: Vec<Item>, unreadable: usize) {
        // A folder that left the library meanwhile keeps no files.
        let Some(i) = self.folders.iter().position(|f| f.path == path) else {
            return;
        };
        self.files
            .retain(|f| f.folder.as_deref() != Some(path.as_str()));
        let name = name_of(&path);
        for item in items {
            self.add_file_from(item, &name, Some(&path));
        }
        let mut listing = listing;
        if unreadable > 0 {
            listing
                .errors
                .push(format!("{unreadable} files have no readable audio header"));
        }
        self.folders[i].listing = listing;
        self.folders[i].reading = false;
    }

    /// Reads the folders that the engine keeps and this source has not read yet.
    pub(crate) fn sync_folders(&mut self) {
        let wanted: Vec<String> = self.state.folders.iter().cloned().collect();
        self.folders.retain(|f| wanted.contains(&f.path));
        for path in wanted {
            if !self.folders.iter().any(|f| f.path == path) {
                let _ = self.queue_scan(&path);
            }
        }
    }

    /// The folders of the library, for the snapshot.
    pub(crate) fn folder_views(&self) -> Vec<FolderView> {
        self.folders
            .iter()
            .map(|f| {
                let mut errors = f.listing.errors.clone();
                if f.reading {
                    errors.push("Reading the folder…".to_owned());
                }
                FolderView {
                    path: f.path.clone(),
                    files: self
                        .files
                        .iter()
                        .filter(|x| x.folder.as_deref() == Some(f.path.as_str()))
                        .map(|x| x.view.clone())
                        .collect(),
                    truncated: f.listing.truncated,
                    errors,
                }
            })
            .collect()
    }

    pub(crate) fn add_folder(&mut self, path: &str) -> Result<usize, String> {
        let path = path.trim().to_owned();
        if path.is_empty() {
            return Err("The folder path is empty".into());
        }
        if !Path::new(&path).is_dir() {
            return Err(format!("{path} is not a folder"));
        }
        self.run_edit(Command::LibraryAddFolder { path: path.clone() })?;
        self.queue_scan(&path)?;
        Ok(0)
    }

    pub(crate) fn remove_folder(&mut self, path: &str) -> Result<usize, String> {
        self.run_edit(Command::LibraryRemoveFolder {
            path: path.to_owned(),
        })?;
        self.folders.retain(|f| f.path != path);
        self.files.retain(|f| f.folder.as_deref() != Some(path));
        Ok(1)
    }

    pub(crate) fn rescan(&mut self, path: &str) -> Result<usize, String> {
        if !self.folders.iter().any(|f| f.path == path) {
            return Err(format!("{path} is not in the Library"));
        }
        self.queue_scan(path)?;
        Ok(0)
    }

    /// Makes a playlist from the audio files of a folder, on the background lane. It returns
    /// the playlist name at once; a notice reports the result.
    pub(crate) fn folder_playlist(&mut self, folder: FolderSource) -> Result<String, String> {
        if !Path::new(&folder.path).is_dir() {
            return Err(format!("{} is not a folder", folder.path));
        }
        let name = self.unique_name(&name_of(&folder.path));
        let max = self.state.station.playlists.iter().map(|p| p.id.0).max();
        let id = PlaylistId(max.unwrap_or(0) + 1);
        let source = serde_json::json!({
            "path": folder.path,
            "recursive": folder.recursive,
            "watch": folder.watch,
        });
        self.queue_probe(
            vec![folder.path.clone()],
            folder.recursive,
            crate::edit::ProbeAfter::NewPlaylist {
                id,
                name: name.clone(),
                source,
            },
        )?;
        Ok(name)
    }
}

/// Keeps at most this many watch problems, the newest ones.
const WATCH_ERRORS: usize = 100;

pub(crate) fn push_bounded(list: &mut Vec<String>, e: String) {
    list.push(e);
    if list.len() > WATCH_ERRORS {
        let extra = list.len() - WATCH_ERRORS;
        list.drain(..extra);
    }
}
