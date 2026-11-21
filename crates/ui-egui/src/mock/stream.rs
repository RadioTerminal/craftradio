//! The stream title of the mock. The engine will call `radio_model::stream_title` here.

use jiff::Timestamp;

use crate::view::{ItemMeta, PublishMode, StreamState};

/// The mock drops each stream for part of every minute: from this second ...
const DROP_FROM_S: i64 = 20;
/// ... for this many seconds.
const DROP_FOR_S: i64 = 12;
/// The time between two tries to connect again, in seconds.
const RETRY_EVERY_S: i64 = 4;
const MINUTE_S: i64 = 60;

/// The simulated state of a stream at `now`. No stream plays in the preview.
pub(super) fn simulate(keep_alive: bool, now: Timestamp) -> StreamState {
    let s = now.as_second().rem_euclid(MINUTE_S) - DROP_FROM_S;
    if !(0..DROP_FOR_S).contains(&s) {
        return StreamState::Connected;
    }
    if !keep_alive {
        return StreamState::Down;
    }
    StreamState::Reconnecting {
        attempt: u32::try_from(1 + s / RETRY_EVERY_S).unwrap_or(1),
        next_in_s: u32::try_from(RETRY_EVERY_S - s % RETRY_EVERY_S).unwrap_or(0),
    }
}

/// Fill the placeholders of `template` from `meta` in one pass over the template.
// The scan never reads a value again, so a value that holds a placeholder stays as it is.
pub(super) fn render(template: &str, meta: &ItemMeta) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let Some(close) = after.find('}') else {
            out.push_str(after);
            return out;
        };
        let value = match &after[1..close] {
            "artist" => Some(&meta.artist),
            "title" => Some(&meta.title),
            "album" => Some(&meta.album),
            "year" => Some(&meta.year),
            "genre" => Some(&meta.genre),
            "comment" => Some(&meta.comment),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(v),
            None => out.push_str(&after[..=close]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The text that Icecast gets for `meta`, after an item whose text was `previous`.
pub(super) fn title_for(meta: &ItemMeta, previous: &str, station: &str) -> String {
    match meta.publish {
        PublishMode::FromTags if meta.artist.is_empty() => meta.title.clone(),
        PublishMode::FromTags => format!("{} – {}", meta.artist, meta.title),
        PublishMode::Custom => render(&meta.custom, meta),
        PublishMode::KeepPrevious => previous.to_owned(),
        PublishMode::Hide => station.to_owned(),
    }
}

/// The tags of an item from a display title such as "Artist – Title".
pub(super) fn from_title(title: &str, publish: PublishMode) -> ItemMeta {
    let (artist, name) = match title.split_once(" – ") {
        Some((a, t)) => (a.to_owned(), t.to_owned()),
        None => (String::new(), title.to_owned()),
    };
    ItemMeta {
        title: name,
        artist,
        publish,
        ..ItemMeta::default()
    }
}
