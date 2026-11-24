//! What plays now, as data channel messages for WebRTC listeners.
//!
//! A change gives one `now` message with the title and artist, and one `history` message
//! with what played before. Small `thumb` pictures of the history covers follow, once per
//! listener each. The cover on air follows in `cover` messages of [`PART`] bytes each,
//! because browsers refuse large messages.

use crate::brand::{PictureError, PictureErrors, load_picture, record};
use base64::Engine;
use serde_json::json;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

/// Raw cover bytes per message. In base64 one part stays far below 256 KiB.
pub(crate) const PART: usize = 48 * 1024;
/// The station sends no cover that is larger than this.
const MAX_COVER: usize = 2 * 1024 * 1024;
/// The edge of a history thumbnail, in pixels.
const THUMB_PX: u32 = 96;

/// One data channel message. A message with a key goes to each listener only once.
#[derive(Debug, Clone)]
pub(crate) struct Message {
    pub(crate) key: Option<String>,
    pub(crate) text: String,
}

impl Message {
    fn every_time(text: String) -> Self {
        Self { key: None, text }
    }
}

/// A history thumbnail: its key, and the JPEG in base64.
#[derive(Debug, Clone)]
struct Thumb {
    key: String,
    data: String,
}

/// The messages for the item on air, with a revision that grows on each change.
#[derive(Default)]
pub(crate) struct Now {
    pub(crate) rev: u64,
    pub(crate) title: String,
    pub(crate) messages: Arc<Vec<Message>>,
    /// Thumbnails by file. `None` for a file without a cover.
    thumbs: HashMap<PathBuf, Option<Thumb>>,
}

pub(crate) type SharedNow = Arc<Mutex<Now>>;

/// An item that played before the one on air, as listeners saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Played {
    pub title: String,
    pub artist: String,
    /// When it started.
    pub at: jiff::Timestamp,
    /// The audio file, for its cover. `None` for an item that shows no cover.
    pub file: Option<PathBuf>,
}

/// The `history` message: the items that played before, newest first.
#[doc(hidden)]
pub fn history_message(history: &[Played]) -> String {
    history_with_thumbs(history, &HashMap::new())
}

fn history_with_thumbs(history: &[Played], thumbs: &HashMap<PathBuf, Option<Thumb>>) -> String {
    let items: Vec<serde_json::Value> = history
        .iter()
        .map(|p| {
            let thumb = p
                .file
                .as_ref()
                .and_then(|f| thumbs.get(f))
                .and_then(|t| t.as_ref())
                .map(|t| t.key.clone());
            json!({ "title": p.title, "artist": p.artist, "at": p.at.as_millisecond(), "thumb": thumb })
        })
        .collect();
    json!({ "type": "history", "items": items }).to_string()
}

/// The messages for one state: now, history, the thumbnails it uses, then the cover parts.
fn compose(
    title: &str,
    artist: &str,
    history: &[Played],
    thumbs: &HashMap<PathBuf, Option<Thumb>>,
    cover: Option<(u64, &str, &[u8])>,
) -> Vec<Message> {
    let mut cover_parts = match cover {
        Some((id, mime, data)) => cover_messages(title, artist, id, mime, data),
        None => vec![meta(title, artist, None)],
    };
    let now = cover_parts.remove(0);
    let mut out = vec![
        Message::every_time(now),
        Message::every_time(history_with_thumbs(history, thumbs)),
    ];
    let used = history
        .iter()
        .filter_map(|p| thumbs.get(p.file.as_ref()?)?.as_ref());
    for t in used {
        out.push(Message {
            key: Some(t.key.clone()),
            text: json!({ "type": "thumb", "key": t.key, "data": t.data }).to_string(),
        });
    }
    out.extend(cover_parts.into_iter().map(Message::every_time));
    out
}

/// Sets what plays now and what played before. A thread reads the cover and the
/// history thumbnails, and the messages grow when it has them.
pub(crate) fn set(
    now: &SharedNow,
    title: String,
    artist: String,
    file: Option<PathBuf>,
    history: &[Played],
    errors: &PictureErrors,
) {
    let history = history.to_vec();
    let (rev, missing) = {
        let Ok(mut n) = now.lock() else { return };
        n.rev += 1;
        n.title.clone_from(&title);
        // Thumbnails stay only for the files that the history still lists.
        n.thumbs
            .retain(|f, _| history.iter().any(|p| p.file.as_ref() == Some(f)));
        n.messages = Arc::new(compose(&title, &artist, &history, &n.thumbs, None));
        let missing: Vec<PathBuf> = history
            .iter()
            .filter_map(|p| p.file.clone())
            .filter(|f| !n.thumbs.contains_key(f))
            .collect();
        (n.rev, missing)
    };
    if file.is_none() && missing.is_empty() {
        return;
    }
    let now = now.clone();
    let errors = errors.clone();
    let _ = thread::Builder::new()
        .name("webrtc-cover".into())
        .spawn(move || {
            let cover = file.as_deref().and_then(|f| match read_cover(f) {
                Ok(cover) => cover,
                Err(reason) => {
                    record(
                        &errors,
                        PictureError {
                            path: f.to_path_buf(),
                            reason,
                        },
                    );
                    None
                }
            });
            let made: Vec<(PathBuf, Option<Thumb>)> = missing
                .into_iter()
                .map(|f| (f.clone(), thumb(&f)))
                .collect();
            let Ok(mut n) = now.lock() else { return };
            n.thumbs.extend(made);
            // A newer item replaced this one while the thread read the files.
            if n.rev != rev {
                return;
            }
            n.rev += 1;
            let id = n.rev;
            let cover = cover
                .as_ref()
                .map(|(mime, data)| (id, mime.as_str(), data.as_slice()));
            n.messages = Arc::new(compose(&title, &artist, &history, &n.thumbs, cover));
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

/// A small JPEG of the cover of `path`, for the history. `None` when it has no cover.
fn thumb(path: &Path) -> Option<Thumb> {
    // The cover thread reports a picture file that fails, so this stays quiet.
    let (_, data) = read_cover(path).ok()??;
    let image = image::load_from_memory(&data).ok()?;
    let small = image.thumbnail(THUMB_PX, THUMB_PX).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
        .encode_image(&small)
        .ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    Some(Thumb {
        key: format!("{:016x}", hasher.finish()),
        data: base64::engine::general_purpose::STANDARD.encode(jpeg),
    })
}

/// The cover of `path`. A PNG, JPEG or WebP file is its own cover. An audio file gives its
/// front cover, or its first picture, and `Ok(None)` when it has none. An image file that
/// cannot be used gives the reason.
pub(crate) fn read_cover(path: &Path) -> Result<Option<(String, Vec<u8>)>, String> {
    let is_image = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["png", "jpg", "jpeg", "webp"]
            .iter()
            .any(|i| e.eq_ignore_ascii_case(i))
    });
    if is_image {
        return load_picture(path).map(|p| Some((p.mime, p.data)));
    }
    Ok(read_tag_cover(path))
}

/// The front cover of an audio file, or its first picture.
fn read_tag_cover(path: &Path) -> Option<(String, Vec<u8>)> {
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
