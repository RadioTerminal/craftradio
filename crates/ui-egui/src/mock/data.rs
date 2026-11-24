//! Invented names, colours and sizes for the mock.

use super::*;

/// Playlist colours come from the theme palette, away from the colours that mean something.
const fn palette(i: usize) -> [u8; 3] {
    crate::theme::PALETTE[i].1
}
pub(super) const MUSIC: [u8; 3] = palette(0);
pub(super) const JINGLES: [u8; 3] = palette(1);
pub(super) const ADS: [u8; 3] = palette(2);
pub(super) const NEWS: [u8; 3] = palette(3);
pub(super) const SHOWS: [u8; 3] = palette(4);
pub(super) const BEDS: [u8; 3] = palette(5);
pub(super) const ONE_OFF: [u8; 3] = palette(6);
/// The name of the playlist of one-off rows.
pub(super) const ONE_OFF_NAME: &str = "One-off";
/// How early a soft one-off of the preview starts, in seconds.
pub(super) const SOFT_EARLY_S: f32 = -0.4;
/// Colours for playlists made from folders, in turn.
pub(super) const FOLDER_COLOURS: [[u8; 3]; 3] = [palette(6), palette(7), palette(5)];
/// The station name. A hidden stream title shows it.
pub(super) const STATION: &str = "Radio Preview";

/// How the stream title of an item from `playlist` is made at first.
pub(super) fn publish_for(playlist: &str) -> PublishMode {
    match playlist {
        "Jingles" | "Ads" | "Beds" => PublishMode::KeepPrevious,
        _ => PublishMode::FromTags,
    }
}

/// The name of the built-in sample music once a folder takes its place.
pub(super) const SAMPLE: &str = "Music (sample)";

pub(super) const HOURS: i64 = 6;
/// The as-played log covers this many hours before now.
pub(super) const PAST_HOURS: i64 = 3;
/// The level of a silent channel, in dBFS.
pub(super) const SILENCE_DB: f32 = -90.0;
/// Manual rows take ids from this base, so they never meet ids made from start times.
pub(super) const MANUAL_ID: u64 = 1 << 62;
/// The first id of a playlist item. The ids stay apart from the row ids and the library ids.
pub(super) const TAB_ITEM_ID: u64 = 1 << 61;
pub(super) const TRACKS: usize = 200;
pub(super) const TRACKS_PER_JINGLE: usize = 3;
pub(super) const ADS_PER_BREAK: usize = 3;
pub(super) const AD_MINUTES: [i8; 2] = [20, 50];
pub(super) const SHOW_HOUR: i8 = 16;

pub(super) const ARTISTS: [&str; 20] = [
    "Mira Solenne",
    "Copper Lantern",
    "Juniper Vale",
    "The Quiet Engines",
    "Kolibri Static",
    "Marlo Fenwick",
    "Silver Tram",
    "Lumen Drift",
    "Ottilie Rask",
    "Nightjar Avenue",
    "Pela Morrow",
    "The Linden Hours",
    "Corvo Bay",
    "Ines Valtier",
    "Glass Orchard",
    "Tavi Ondrej",
    "Brindle & Moss",
    "Saffron Relay",
    "Halvard Pine",
    "Velka Strand",
];
pub(super) const NOUNS: [&str; 10] = [
    "Lanterns",
    "Signals",
    "Tides",
    "Gardens",
    "Satellites",
    "Echoes",
    "Harbours",
    "Embers",
    "Orchards",
    "Meridians",
];
pub(super) const PLACES: [&str; 10] = [
    "Kelmora", "Tavissa", "Brendhal", "Ulvenik", "Sarrow", "Miravel", "Quenby", "Hollin", "Draska",
    "Vintmoor",
];
pub(super) const DEVICES: [&str; 4] = [
    "Built-in Output",
    "Built-in Microphone",
    "USB Audio Interface",
    "Headphones (USB)",
];
pub(super) const AD_NAMES: [&str; 6] = [
    "Kesten Bakery",
    "Lumo Bikes",
    "Orrin Hardware",
    "Pavla Opticians",
    "Riverside Market",
    "Tamsin Travel",
];
/// Carts on one page of the cartwall.
pub(super) const CARTS_PER_PAGE: usize = 16;
/// A new cart: ducking at -12 dB, one-shot, no colour.
pub(super) const DEFAULT_SLOT: CartSlot = CartSlot {
    duck_db: -12.0,
    mode: crate::view::CartMode::OneShot,
    colour: None,
};
pub(super) const ICECAST_PORT: u16 = 8000;
/// The stream title before a row that is not in the timeline.
pub(super) const UNKNOWN_PREVIOUS: &str = "(the stream title before it)";
/// The part name of the cartwall settings.
pub(super) const CARTWALL_PART: &str = "cartwall";
pub(super) const ICECAST_KBPS: u32 = 128;
/// The cartwall part settings for the grid.
pub(super) const CART_COLS_KEY: &str = "cols";
pub(super) const CART_ROWS_KEY: &str = "rows";

pub(super) const CARTS: [&str; 32] = [
    "Station ID 1",
    "Station ID 2",
    "Station ID 3",
    "Sweep",
    "Horn",
    "Laugh",
    "Applause",
    "Bed 1",
    "Bed 2",
    "Drum Roll",
    "Rimshot",
    "Cheer",
    "Countdown",
    "Stinger",
    "Whoosh",
    "Bell",
    "Weather Intro",
    "Traffic Intro",
    "Sports Intro",
    "News Intro",
    "Top of Hour",
    "Promo 1",
    "Promo 2",
    "Promo 3",
    "Phone Ring",
    "Crowd",
    "Siren",
    "Clock Tick",
    "Bass Drop",
    "Reverse Swell",
    "Outro",
    "Silence 5s",
];

pub(super) fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}
