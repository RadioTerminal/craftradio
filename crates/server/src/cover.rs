//! The cover of what a station plays now: set by the station's app with its key, kept in
//! memory only, and served to the listener page at `/<slug>/cover`.

use ring::digest::{SHA256, digest};

/// The largest cover `PUT /api/station/cover` takes.
pub const MAX_COVER: usize = 1 << 20;

/// One cover: its bytes, the type they were sent as, and the tag that names them.
#[derive(Debug, PartialEq, Eq)]
pub struct Cover {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    /// A strong `ETag`, quoted: a hash of the bytes and of the count of covers the station
    /// was given, so each cover set is new to the page, even with the bytes of the last
    /// (the next song of the same album).
    pub etag: String,
}

impl Cover {
    /// The cover `bytes` of type `mime`, the station's `set`-th.
    pub fn new(mime: &'static str, bytes: Vec<u8>, set: u64) -> Cover {
        let mut tagged = set.to_be_bytes().to_vec();
        tagged.extend_from_slice(&bytes);
        let mut etag = String::with_capacity(34);
        etag.push('"');
        for byte in digest(&SHA256, &tagged).as_ref().iter().take(16) {
            etag.push_str(&format!("{byte:02x}"));
        }
        etag.push('"');
        Cover { mime, bytes, etag }
    }

    /// True when an `If-None-Match` value names this cover (or is `*`).
    pub fn matches(&self, if_none_match: &str) -> bool {
        if_none_match
            .split(',')
            .map(str::trim)
            .any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == self.etag.as_str())
    }
}

/// The image type of a `Content-Type` value, as the server's own string: JPEG, PNG or WebP.
/// Parameters after `;` and the case are ignored.
pub fn image_type(content_type: &str) -> Option<&'static str> {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "image/jpeg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}
