use crate::ItemId;
use jiff::civil;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn yes() -> bool {
    true
}

/// Where an item's audio comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    File {
        path: String,
    },
    LiveInput {
        device: String,
        channels: [u16; 2],
    },
    /// An internet stream. With `keep_alive` (the default), a stream that drops on air
    /// keeps its place until its planned end, and playout reconnects.
    Stream {
        url: String,
        #[serde(default = "yes")]
        keep_alive: bool,
    },
}

/// Cue points within a file, as offsets from its start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    pub cue_in: Duration,
    pub segue: Duration,
    pub cue_out: Duration,
    pub intro_end: Option<Duration>,
    pub fade_in: Duration,
    pub fade_out: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    #[serde(default)]
    pub meta: ItemMeta,
    pub title: String,
    pub artist: Option<String>,
    pub source: Source,
    pub timing: Option<Timing>,
    pub planned_len: Option<Duration>,
    #[serde(default = "yes")]
    pub movable: bool,
    #[serde(default = "yes")]
    pub trimmable: bool,
    pub locked: bool,
    pub valid_from: Option<civil::Date>,
    pub valid_to: Option<civil::Date>,
    pub max_per_day: Option<u32>,
}

/// File tags and stream title settings. The year is text to preserve tag values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub genre: String,
    pub comment: String,
    pub publish: PublishMode,
    pub custom: String,
}

/// Selects the text sent to the stream output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishMode {
    #[default]
    FromTags,
    Custom,
    KeepPrevious,
    Hide,
}

/// Builds the stream title. Unknown template fields stay unchanged.
pub fn stream_title(item: &Item, previous: &str, station_name: &str) -> String {
    let m = &item.meta;
    match m.publish {
        PublishMode::FromTags if m.artist.is_empty() => m.title.clone(),
        PublishMode::FromTags => format!("{} – {}", m.artist, m.title),
        PublishMode::Custom => {
            let mut output = String::new();
            let mut rest = m.custom.as_str();
            while let Some(start) = rest.find('{') {
                output.push_str(&rest[..start]);
                rest = &rest[start..];
                let Some(end) = rest.find('}') else { break };
                output.push_str(match &rest[1..end] {
                    "artist" => &m.artist,
                    "title" => &m.title,
                    "album" => &m.album,
                    "year" => &m.year,
                    _ => &rest[..=end],
                });
                rest = &rest[end + 1..];
            }
            output.push_str(rest);
            output
        }
        PublishMode::KeepPrevious => previous.to_owned(),
        PublishMode::Hide => station_name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_title_modes() {
        let mut item = Item {
            id: ItemId(1),
            title: "Echo".into(),
            artist: Some("Cloud".into()),
            meta: ItemMeta::default(),
            source: Source::File {
                path: "echo.wav".into(),
            },
            timing: None,
            planned_len: None,
            movable: true,
            trimmable: true,
            locked: false,
            valid_from: None,
            valid_to: None,
            max_per_day: None,
        };
        item.meta = ItemMeta {
            title: "Echo".into(),
            artist: "Cloud".into(),
            album: "Dawn".into(),
            year: "2026".into(),
            ..ItemMeta::default()
        };
        assert_eq!(stream_title(&item, "old", "station"), "Cloud – Echo");
        item.meta.artist.clear();
        assert_eq!(stream_title(&item, "old", "station"), "Echo");
        item.meta.publish = PublishMode::Custom;
        item.meta.custom = "{title}/{album}/{year}/{artist}".into();
        assert_eq!(stream_title(&item, "old", "station"), "Echo/Dawn/2026/");
        item.meta.publish = PublishMode::KeepPrevious;
        assert_eq!(stream_title(&item, "old", "station"), "old");
        item.meta.publish = PublishMode::Hide;
        assert_eq!(stream_title(&item, "old", "station"), "station");
    }
    #[test]
    fn old_item_and_playlist_data_get_defaults() {
        let item: Item = toml::from_str(
            r#"
            id = 1
            title = "Echo"
            locked = false
            [source.File]
            path = "echo.wav"
        "#,
        )
        .unwrap();
        assert_eq!(item.meta.publish, PublishMode::FromTags);
        let stream: Item = toml::from_str(
            r#"
            id = 2
            title = "Relay"
            locked = false
            [source.Stream]
            url = "http://relay.invalid/live"
        "#,
        )
        .unwrap();
        assert!(matches!(
            stream.source,
            Source::Stream {
                keep_alive: true,
                ..
            }
        ));
        assert!(item.meta.title.is_empty());
        let playlist: crate::Playlist = toml::from_str(
            r#"
            id = 1
            name = "Cloud"
            color = [100, 100, 100]
            is_main = true
            is_fill_pool = false
            priority = 0
            pick = "InOrder"
            items = []
            rules = []
            constraints = []
        "#,
        )
        .unwrap();
        assert!(playlist.source.is_none());
        assert_eq!(playlist.on_interrupt, crate::OnInterrupt::Resume);
    }
}
