//! A bounded listing of the audio files of a folder, and a quick probe of one file.
//!
//! The listing reads only names, in name order, so an early stop always gives the same files.
//! It skips linked folders and stops at the limits.

use crate::{LibraryError, audio_path, media, metadata};
use lofty::file::AudioFile;
use radio_model::ItemMeta;
use std::path::Path;
use std::time::Duration;

/// How far a listing goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_files: usize,
    pub max_entries: usize,
    pub max_depth: usize,
}

impl Limits {
    /// The limits of the UI: 2000 files, 20 000 entries, 8 folders deep.
    pub const DEFAULT: Self = Self {
        max_files: 2000,
        max_entries: 20_000,
        max_depth: 8,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The result of a listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Full paths of the audio files, sorted.
    pub files: Vec<String>,
    /// True when a limit stopped the listing.
    pub truncated: bool,
    /// Problems that did not stop the listing.
    pub errors: Vec<String>,
}

/// Lists the audio files of `dir`. With `recursive`, subfolders count too.
pub fn list_audio(dir: &Path, recursive: bool, limits: Limits) -> Listing {
    let mut out = Listing::default();
    let mut entries = 0;
    walk(dir, recursive, 0, limits, &mut entries, &mut out);
    out.files.sort();
    out.files.truncate(limits.max_files);
    out
}

fn walk(
    dir: &Path,
    recursive: bool,
    depth: usize,
    limits: Limits,
    entries: &mut usize,
    out: &mut Listing,
) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            out.errors
                .push(format!("cannot read {}: {e}", dir.display()));
            return;
        }
    };
    let mut list = Vec::new();
    for entry in read {
        match entry {
            Ok(e) => list.push(e),
            Err(e) => out
                .errors
                .push(format!("cannot read in {}: {e}", dir.display())),
        }
    }
    list.sort_by_key(std::fs::DirEntry::file_name);
    for entry in list {
        if out.files.len() >= limits.max_files || *entries >= limits.max_entries {
            out.truncated = true;
            return;
        }
        *entries += 1;
        let path = entry.path();
        // `file_type` does not follow links, so a linked folder is not a folder here.
        let kind = match entry.file_type() {
            Ok(k) => k,
            Err(e) => {
                out.errors
                    .push(format!("cannot read {}: {e}", path.display()));
                continue;
            }
        };
        if kind.is_dir() {
            if recursive && depth + 1 < limits.max_depth {
                walk(&path, recursive, depth + 1, limits, entries, out);
            }
        } else if kind.is_file() && audio_path(&path) {
            out.files.push(path.display().to_string());
        }
    }
}

/// The length and the tags of a file, from its header only. It decodes no audio.
pub fn probe(path: impl AsRef<Path>) -> Result<(Duration, ItemMeta), LibraryError> {
    let path = path.as_ref();
    let file = lofty::read_from_path(path).map_err(media)?;
    let len = file.properties().duration();
    if len.is_zero() {
        return Err(media("the file has no length in its header"));
    }
    Ok((len, metadata(path)?))
}
