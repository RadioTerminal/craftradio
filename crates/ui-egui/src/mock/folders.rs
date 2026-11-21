//! Read the audio files of a folder. Only the mock source reads the disk; the panels never do.
//!
//! The scan reads only names. It does not decode files, so it does not know their lengths.
//! It skips linked folders, goes at most `max_depth` folders deep, and stops at the limits.
//! It reads each folder in name order, so an early stop always gives the same files.

use std::path::Path;

/// The file extensions that count as audio.
pub(super) const AUDIO: [&str; 7] = ["mp3", "flac", "wav", "ogg", "m4a", "opus", "aac"];

/// How far a scan goes.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    pub max_files: usize,
    pub max_entries: usize,
    pub max_depth: usize,
}

impl Limits {
    pub(super) const DEFAULT: Self = Self {
        max_files: 2000,
        max_entries: 20_000,
        max_depth: 8,
    };
}

/// The result of a scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Scan {
    /// Full paths of the audio files, sorted.
    pub files: Vec<String>,
    /// True when a limit stopped the scan.
    pub truncated: bool,
    /// Problems that did not stop the scan.
    pub errors: Vec<String>,
}

/// True when the path has an audio extension.
pub(super) fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// Scan `dir` for audio files. With `recursive`, subfolders count too.
pub(super) fn scan(dir: &Path, recursive: bool, limits: Limits) -> Scan {
    let mut out = Scan::default();
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
    out: &mut Scan,
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
        } else if !kind.is_symlink() && is_audio(&path) {
            out.files.push(path.display().to_string());
        }
    }
}

/// The last part of a path: the file or folder name.
pub(super) fn name_of(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// A file name without its extension.
pub(super) fn stem_of(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}
