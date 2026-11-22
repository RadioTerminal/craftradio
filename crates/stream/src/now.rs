//! What plays now, as data channel messages for WebRTC listeners.
//!
//! A change gives one `now` message with the title and artist, and one `history` message
//! with what played before. A cover follows in `cover` messages of [`PART`] bytes each,
//! because browsers refuse large messages.

use base64::Engine;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

/// Raw cover bytes per message. In base64 one part stays far below 256 KiB.
const PART: usize = 48 * 1024;
/// The station sends no cover that is larger than this.
const MAX_COVER: usize = 2 * 1024 * 1024;

/// The messages for the item on air, with a revision that grows on each change.
#[derive(Default)]
pub(crate) struct Now {
    pub(crate) rev: u64,
    pub(crate) title: String,
    pub(crate) messages: Arc<Vec<String>>,
}

pub(crate) type SharedNow = Arc<Mutex<Now>>;

/// An item that played before the one on air, as listeners saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Played {
    pub title: String,
    pub artist: String,
    /// When it started.
    pub at: jiff::Timestamp,
}

/// The `history` message: the items that played before, newest first.
#[doc(hidden)]
pub fn history_message(history: &[Played]) -> String {
    let items: Vec<serde_json::Value> = history
        .iter()
        .map(|p| json!({ "title": p.title, "artist": p.artist, "at": p.at.as_millisecond() }))
        .collect();
    json!({ "type": "history", "items": items }).to_string()
}

/// Sets what plays now and what played before. The cover follows when a thread has read it.
pub(crate) fn set(
    now: &SharedNow,
    title: String,
    artist: String,
    file: Option<PathBuf>,
    history: &[Played],
) {
    let history = history_message(history);
    let rev = {
        let Ok(mut n) = now.lock() else { return };
        n.rev += 1;
        n.title.clone_from(&title);
        n.messages = Arc::new(vec![meta(&title, &artist, None), history.clone()]);
        n.rev
    };
    let Some(file) = file else { return };
    let now = now.clone();
    let _ = thread::Builder::new()
        .name("webrtc-cover".into())
        .spawn(move || {
            let Some((mime, data)) = read_cover(&file) else {
                return;
            };
            let Ok(mut n) = now.lock() else { return };
            // A newer item replaced this one while the thread read the file.
            if n.rev != rev {
                return;
            }
            n.rev += 1;
            let mut messages = cover_messages(&title, &artist, n.rev, &mime, &data);
            messages.insert(1, history);
            n.messages = Arc::new(messages);
        });
}

fn meta(title: &str, artist: &str, cover: Option<serde_json::Value>) -> String {
    json!({ "type": "now", "title": title, "artist": artist, "cover": cover }).to_string()
}

/// The `now` message and the `cover` parts for one cover.
#[doc(hidden)]
pub fn cover_messages(title: &str, artist: &str, id: u64, mime: &str, data: &[u8]) -> Vec<String> {
    let parts = data.len().div_ceil(PART);
    let mut out = vec![meta(
        title,
        artist,
        Some(json!({ "id": id, "mime": mime, "parts": parts })),
    )];
    let b64 = base64::engine::general_purpose::STANDARD;
    for (i, chunk) in data.chunks(PART).enumerate() {
        out.push(
            json!({ "type": "cover", "id": id, "part": i, "data": b64.encode(chunk) }).to_string(),
        );
    }
    out
}

/// The front cover of an audio file, or its first picture.
pub(crate) fn read_cover(path: &Path) -> Option<(String, Vec<u8>)> {
    use lofty::file::TaggedFileExt;
    use lofty::picture::PictureType;
    let file = lofty::read_from_path(path).ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pictures = tag.pictures();
    let picture = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())?;
    let data = picture.data();
    if data.is_empty() || data.len() > MAX_COVER {
        return None;
    }
    let mime = picture
        .mime_type()
        .map_or("image/jpeg", |m| m.as_str())
        .to_owned();
    // Only image types that a browser shows go out.
    if !matches!(
        mime.as_str(),
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    ) {
        return None;
    }
    Some((mime, data.to_vec()))
}
