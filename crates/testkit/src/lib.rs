//! Fixture builders for tests.

use radio_model::{Item, ItemId, PickOrder, Playlist, PlaylistId, Source, Station, Timing};
use std::time::Duration;

fn secs(s: u32) -> Duration {
    Duration::from_secs(s.into())
}

fn base_item(id: u64, title: &str, artist: Option<&str>, source: Source) -> Item {
    Item {
        id: ItemId(id),
        meta: radio_model::ItemMeta::default(),
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        source,
        timing: None,
        planned_len: None,
        movable: true,
        trimmable: true,
        locked: false,
        valid_from: None,
        valid_to: None,
        max_per_day: None,
    }
}

/// A music file: cue_in 0, segue 2s before the end, movable and trimmable.
pub fn track(id: u64, title: &str, artist: &str, secs_len: u32) -> Item {
    let mut item = base_item(
        id,
        title,
        Some(artist),
        Source::File {
            path: format!("{title}.mp3"),
        },
    );
    item.timing = Some(Timing {
        cue_in: Duration::ZERO,
        segue: secs(secs_len.saturating_sub(2)),
        cue_out: secs(secs_len),
        intro_end: None,
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
    });
    item
}

/// A jingle: plays to the end (segue == cue_out), neither movable nor trimmable.
pub fn jingle(id: u64, secs_len: u32) -> Item {
    let mut item = base_item(
        id,
        &format!("jingle-{id}"),
        None,
        Source::File {
            path: format!("jingle-{id}.wav"),
        },
    );
    item.timing = Some(Timing {
        cue_in: Duration::ZERO,
        segue: secs(secs_len),
        cue_out: secs(secs_len),
        intro_end: None,
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
    });
    item.movable = false;
    item.trimmable = false;
    item
}

/// A live input slot with a planned length.
pub fn live(id: u64, planned_secs: u32) -> Item {
    let mut item = base_item(
        id,
        &format!("live-{id}"),
        None,
        Source::LiveInput {
            device: "default".to_owned(),
            channels: [0, 1],
        },
    );
    item.planned_len = Some(secs(planned_secs));
    item.movable = false;
    item.trimmable = false;
    item
}

/// An enabled, non-main playlist with in-order picking and no rules.
pub fn playlist(id: u64, name: &str, items: Vec<Item>) -> Playlist {
    Playlist {
        id: PlaylistId(id),
        source: None,
        name: name.to_owned(),
        color: [128, 128, 128],
        enabled: true,
        is_main: false,
        is_fill_pool: false,
        role: radio_model::PlaylistRole::User,
        on_interrupt: radio_model::OnInterrupt::Resume,
        priority: 0,
        pick: PickOrder::InOrder,
        items,
        rules: Vec::new(),
        constraints: Vec::new(),
    }
}

pub fn station(tz: &str, playlists: Vec<Playlist>) -> Station {
    Station {
        tz: tz.to_owned(),
        playlists,
    }
}
