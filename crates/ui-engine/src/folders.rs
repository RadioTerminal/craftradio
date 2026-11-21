//! The folders of the library, the files that no playlist holds yet, and watched folders.
//!
//! A worker thread lists and probes folders, so a large folder never stops the UI.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

use radio_engine::Command;
use radio_library::{Limits, Listing, WatchEvent, Watcher};
use radio_model::{FolderSource as MFolder, Item, ItemId, PickOrder, PlaylistId, Source};
use radio_ui_egui::view::{FolderSource, FolderView, ItemMeta, LibraryItem};

use crate::EngineSource;
use crate::edit::file_item;
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

/// A request to the folder worker.
struct Job {
    path: String,
    limits: Limits,
}

/// The answer of the folder worker.
pub(crate) struct Done {
    path: String,
    listing: Listing,
    items: Vec<Item>,
    unreadable: usize,
}

/// The folder worker: one thread that lists and probes folders in order.
pub(crate) struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl Worker {
    pub(crate) fn start() -> Option<Self> {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        std::thread::Builder::new()
            .name("library-folders".into())
            .spawn(move || {
                for job in rx {
                    let listing = radio_library::list_audio(Path::new(&job.path), true, job.limits);
                    let mut items = Vec::new();
                    let mut unreadable = 0;
                    for f in &listing.files {
                        match file_item(f) {
                            Some(i) => items.push(i),
                            None => unreadable += 1,
                        }
                    }
                    let done = Done {
                        path: job.path,
                        listing,
                        items,
                        unreadable,
                    };
                    if tx.send(done).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self { jobs, done })
    }
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
            meta: view_meta(&item, &meta),
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
            .send(Job {
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

    /// Takes the finished folder reads.
    pub(crate) fn take_scans(&mut self) {
        let done: Vec<Done> = match &self.worker {
            Some(w) => w.done.try_iter().collect(),
            None => Vec::new(),
        };
        for d in done {
            // A folder that left the library meanwhile keeps no files.
            let Some(i) = self.folders.iter().position(|f| f.path == d.path) else {
                continue;
            };
            self.files
                .retain(|f| f.folder.as_deref() != Some(d.path.as_str()));
            let name = name_of(&d.path);
            for item in d.items {
                self.add_file_from(item, &name, Some(&d.path));
            }
            let mut listing = d.listing;
            if d.unreadable > 0 {
                listing.errors.push(format!(
                    "{} files have no readable audio header",
                    d.unreadable
                ));
            }
            self.folders[i].listing = listing;
            self.folders[i].reading = false;
        }
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

    /// Makes a playlist from the audio files of a folder. It returns the playlist name.
    pub(crate) fn folder_playlist(&mut self, folder: FolderSource) -> Result<String, String> {
        let list =
            radio_library::list_audio(Path::new(&folder.path), folder.recursive, self.limits);
        let (items, _) = self.probe_paths(&list.files);
        if items.is_empty() {
            let why = list.errors.first().cloned().unwrap_or_default();
            return Err(format!("No audio files in {} {why}", folder.path));
        }
        let name = self.unique_name(&name_of(&folder.path));
        let max = self.state.station.playlists.iter().map(|p| p.id.0).max();
        let id = PlaylistId(max.unwrap_or(0) + 1);
        let first = self.fresh_ids(items.len() as u64);
        let items: Vec<Item> = items
            .into_iter()
            .enumerate()
            .map(|(k, mut i)| {
                i.id = ItemId(first + k as u64);
                i
            })
            .collect();
        let source = serde_json::json!({
            "path": folder.path,
            "recursive": folder.recursive,
            "watch": folder.watch,
        });
        self.run_batch(vec![
            Command::PlaylistCreate { name: name.clone() },
            Command::PlaylistSetPick {
                playlist: id,
                pick: PickOrder::Shuffle { seed: id.0 },
            },
            Command::PlaylistAddItems {
                playlist: id,
                items,
                at: None,
            },
            Command::PlaylistSetSource {
                playlist: id,
                source,
            },
        ])?;
        Ok(name)
    }

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
                Err(e) => self
                    .watch_errors
                    .push(format!("cannot watch {}: {e}", src.path)),
            }
        }
    }

    /// Applies the settled changes of watched folders to their playlists.
    pub(crate) fn poll_watches(&mut self) {
        let mut changes: Vec<(PlaylistId, WatchEvent)> = Vec::new();
        for (id, (_, w)) in &mut self.watches {
            match w.events() {
                Ok(events) => changes.extend(events.map(|e| (*id, e))),
                Err(e) => self.watch_errors.push(e.to_string()),
            }
        }
        for (playlist, change) in changes {
            match change {
                WatchEvent::Added(path) => {
                    let path = path.to_string_lossy().into_owned();
                    let Some(item) = file_item(&path) else {
                        continue;
                    };
                    let first = self.fresh_ids(1);
                    let mut item = item;
                    item.id = ItemId(first);
                    let _ = self.run_edit(Command::StationSystem {
                        commands: vec![Command::PlaylistAddItems {
                            playlist,
                            items: vec![item],
                            at: None,
                        }],
                    });
                }
                WatchEvent::Removed(path) => {
                    let path = path.to_string_lossy().into_owned();
                    let gone: Vec<ItemId> = self
                        .state
                        .station
                        .playlists
                        .iter()
                        .filter(|p| p.id == playlist)
                        .flat_map(|p| p.items.iter())
                        .filter(|i| matches!(&i.source, Source::File { path: p } if *p == path))
                        .map(|i| i.id)
                        .collect();
                    if !gone.is_empty() {
                        let _ = self.run_edit(Command::StationSystem {
                            commands: vec![Command::PlaylistRemoveItems {
                                playlist,
                                items: gone,
                            }],
                        });
                    }
                }
                WatchEvent::Changed(_) => {}
            }
        }
    }
}
