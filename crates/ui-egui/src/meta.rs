//! The stream title: the exact text that Icecast gets for an item.

use crate::view::{ItemMeta, PublishMode, Row};

/// The placeholders that a custom stream title can use.
pub const PLACEHOLDERS: [&str; 6] = [
    "{artist}",
    "{title}",
    "{album}",
    "{year}",
    "{genre}",
    "{comment}",
];

/// The text that Icecast gets for `meta`.
/// `previous` is the text of the item before. `station` is the station name.
pub fn stream_title(meta: &ItemMeta, previous: &str, station: &str) -> String {
    match meta.publish {
        PublishMode::FromTags => {
            if meta.artist.is_empty() {
                meta.title.clone()
            } else {
                format!("{} – {}", meta.artist, meta.title)
            }
        }
        PublishMode::Custom => {
            let values = [
                &meta.artist,
                &meta.title,
                &meta.album,
                &meta.year,
                &meta.genre,
                &meta.comment,
            ];
            PLACEHOLDERS
                .iter()
                .zip(values)
                .fold(meta.custom.clone(), |text, (p, v)| text.replace(p, v))
        }
        PublishMode::KeepPrevious => previous.to_owned(),
        PublishMode::Hide => station.to_owned(),
    }
}

/// The stream title before the row `stop`, or after all rows when `stop` is `None`.
/// The rows must be in time order.
pub fn streaming_before<'a>(
    rows: impl IntoIterator<Item = &'a Row>,
    stop: Option<u64>,
    station: &str,
) -> String {
    let mut current = station.to_owned();
    for r in rows {
        if Some(r.id) == stop {
            break;
        }
        current = stream_title(&r.meta, &current, station);
    }
    current
}

/// The tags of an item from a display title such as "Artist – Title".
pub fn from_title(title: &str, publish: PublishMode) -> ItemMeta {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_stream_title_fills_its_placeholders() {
        let meta = ItemMeta {
            title: "Lanterns of Kelmora".to_owned(),
            artist: "Mira Solenne".to_owned(),
            year: "2024".to_owned(),
            publish: PublishMode::Custom,
            custom: "{artist} – {title} ({year}) on air".to_owned(),
            ..ItemMeta::default()
        };
        let text = stream_title(&meta, "before", "Station");
        assert_eq!(text, "Mira Solenne – Lanterns of Kelmora (2024) on air");
        assert!(
            !text.contains('{'),
            "no placeholder stays in the stream title"
        );
    }
}
