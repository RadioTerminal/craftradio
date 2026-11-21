//! List the audio files of a folder that a person picked.
//!
//! The preview reads only names. It does not decode files, so it does not know their lengths.

use std::path::Path;

/// The file extensions that count as audio.
pub const AUDIO: [&str; 7] = ["mp3", "flac", "wav", "ogg", "m4a", "opus", "aac"];
/// The preview lists at most this many files per folder.
pub const MAX_FILES: usize = 2000;

/// The full paths of the audio files in `dir`, sorted. With `recursive`, subfolders count too.
pub fn list_audio(dir: &Path, recursive: bool) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    walk(dir, recursive, &mut out)?;
    out.sort();
    out.truncate(MAX_FILES);
    Ok(out)
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<String>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        if out.len() >= MAX_FILES {
            break;
        }
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                walk(&path, recursive, out)?;
            }
            continue;
        }
        if is_audio(&path) {
            out.push(path.display().to_string());
        }
    }
    Ok(())
}

/// True when the path has an audio extension.
pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// Split dropped paths into audio files and a count of skipped files.
/// A dropped folder gives its audio files, without subfolders.
pub fn expand_dropped(paths: &[std::path::PathBuf]) -> (Vec<String>, usize) {
    let mut audio = Vec::new();
    let mut skipped = 0;
    for p in paths {
        if p.is_dir() {
            match list_audio(p, false) {
                Ok(files) => audio.extend(files),
                Err(_) => skipped += 1,
            }
        } else if is_audio(p) {
            audio.push(p.display().to_string());
        } else {
            skipped += 1;
        }
    }
    audio.truncate(MAX_FILES);
    (audio, skipped)
}

/// The last part of a path: the file or folder name.
pub fn name_of(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// A file name without its extension.
pub fn stem_of(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}
