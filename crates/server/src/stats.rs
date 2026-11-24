//! The public stats document the listener page reads for the current title, in the shape
//! of Icecast's `/status-json.xsl`.

use crate::framing::Format;
use crate::mount::MountStats;
use serde_json::{Value, json};

/// `{"icestats":{"source":[…]}}` with one entry for each live mount, given as its slug and
/// its stats. `source` is always an array. Only what a listener may see is listed: the
/// stream's URL, name, type, listener count, title and artist.
pub fn status_json(public_url: &str, mounts: &[(String, MountStats)]) -> String {
    let base = public_url.trim_end_matches('/');
    let sources: Vec<Value> = mounts
        .iter()
        .filter(|(_, stats)| stats.live)
        .map(|(slug, stats)| {
            json!({
                "listenurl": format!("{base}/{slug}/stream"),
                "server_name": stats.name,
                "server_type": stats.format.map(Format::content_type).unwrap_or_default(),
                "listeners": stats.listeners,
                "title": stats.title,
                "artist": stats.artist,
            })
        })
        .collect();
    json!({ "icestats": { "source": sources } }).to_string()
}
