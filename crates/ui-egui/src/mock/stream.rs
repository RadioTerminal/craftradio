//! The stream title of the mock. The engine will call `radio_model::stream_title` here.

use crate::view::{ItemMeta, PublishMode};

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
