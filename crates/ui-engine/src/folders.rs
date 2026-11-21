//! Library files that no playlist holds yet, and the folders of the library.

use radio_model::Item;
use radio_ui_egui::view::{ItemMeta, LibraryItem};

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
}

impl EngineSource {
    /// Adds a file to the library, or finds it there. It returns the library id.
    pub(crate) fn add_file(&mut self, item: Item, category: &str) -> u64 {
        let path = crate::source_text(&item.source);
        if let Some(l) = self.library.iter().find(|l| l.path == path) {
            return l.id;
        }
        if let Some(f) = self.files.iter().find(|f| f.view.path == path) {
            return f.id;
        }
        let id = FILE_IDS + self.files.len() as u64;
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
        self.files.push(LibFile { id, item, view });
        id
    }

    /// Sets the tags of a library file that no playlist holds.
    pub(crate) fn set_file_meta(&mut self, id: u64, meta: &ItemMeta) {
        if let Some(f) = self.files.iter_mut().find(|f| f.id == id) {
            f.item.meta = crate::map::model_meta(meta);
            f.view.meta = meta.clone();
        }
    }

    pub(crate) fn folder_playlist(
        &mut self,
        _folder: radio_ui_egui::view::FolderSource,
    ) -> Result<String, String> {
        Err("Folders are not in this build yet".into())
    }

    pub(crate) fn add_folder(&mut self, _path: &str) -> Result<usize, String> {
        Err("Folders are not in this build yet".into())
    }

    pub(crate) fn remove_folder(&mut self, _path: &str) -> Result<usize, String> {
        Err("Folders are not in this build yet".into())
    }

    pub(crate) fn rescan(&mut self, _path: &str) -> Result<usize, String> {
        Err("Folders are not in this build yet".into())
    }
}
