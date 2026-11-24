//! The public stats document the listener page reads for the current title, in the shape
//! of Icecast's `/status-json.xsl`.

use crate::framing::Format;
use crate::mount::MountStats;
use serde_json::{Value, json};

/// One mount for the stats: where it is served, below the public URL, as `<slug>/<mount>`,
/// and what it is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsEntry {
    pub path: String,
    pub stats: MountStats,
}

/// `{"icestats":{"source":[…]}}` with one entry for each live mount, in the order given.
/// `source` is always an array. Only what a listener may see is listed: the stream's URL,
/// name, type, listener count, title and artist, and its bitrate when it is known.
pub fn status_json(public_url: &str, entries: &[StatsEntry]) -> String {
    let base = public_url.trim_end_matches('/');
    let sources: Vec<Value> = entries
        .iter()
        .filter(|entry| entry.stats.live)
        .map(|StatsEntry { path, stats }| {
            let mut source = json!({
                "listenurl": format!("{base}/{path}"),
                "server_name": stats.name,
                "server_type": stats.format.map(Format::content_type).unwrap_or_default(),
                "listeners": stats.listeners,
                "title": stats.title,
                "artist": stats.artist,
            });
            if let (Some(kbps), Some(fields)) = (stats.kbps, source.as_object_mut()) {
                fields.insert("bitrate".into(), kbps.into());
            }
            source
        })
        .collect();
    json!({ "icestats": { "source": sources } }).to_string()
}
