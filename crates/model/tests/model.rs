use jiff::civil;
use radio_model::{
    Constraint, FixMode, Item, ItemId, ModelError, PickOrder, Placement, Repeat, Rule, Source,
    Station, Trigger,
};
use radio_testkit::{jingle, live, playlist, station, track};
use std::time::Duration;

fn valid_station() -> Station {
    let mut main = playlist(
        1,
        "Main",
        vec![track(1, "Song", "Artist", 180), jingle(2, 10)],
    );
    main.is_main = true;
    station("Europe/Ljubljana", vec![main])
}

#[test]
fn station_roundtrips_through_toml() {
    let mut st = valid_station();
    let mut ads = playlist(2, "Ads", vec![live(3, 60), track(4, "Ad", "Brand", 30)]);
    ads.is_fill_pool = true;
    ads.pick = PickOrder::Weighted {
        weights: vec![(ItemId(3), 2), (ItemId(4), 5)],
    };
    ads.constraints = vec![
        Constraint::NotAdjacentToSelf,
        Constraint::MinGapSameItem(Duration::from_secs(600)),
        Constraint::Daypart {
            from: civil::time(6, 0, 0, 0),
            to: civil::time(22, 30, 0, 0),
        },
        Constraint::MaxPerHour(4),
    ];
    ads.rules = vec![
        Rule {
            trigger: Trigger::EveryNTracks(3),
            placement: Placement::BetweenTracks,
            take: 1,
            constraints: vec![Constraint::NotAdjacentTo(radio_model::PlaylistId(1))],
        },
        Rule {
            trigger: Trigger::At {
                at: civil::date(2026, 11, 21).at(12, 0, 0, 0),
                repeat: Repeat::Daily,
            },
            placement: Placement::Fixed {
                mode: FixMode::Hard,
                tolerance: Duration::from_secs(5),
            },
            take: 2,
            constraints: vec![],
        },
        Rule {
            trigger: Trigger::AtMinutes(vec![0, 30]),
            placement: Placement::BetweenTracks,
            take: 1,
            constraints: vec![],
        },
    ];
    let mut dated = track(5, "Seasonal", "X", 200);
    dated.valid_from = Some(civil::date(2026, 12, 1));
    dated.valid_to = Some(civil::date(2026, 12, 31));
    dated.max_per_day = Some(3);
    ads.items.push(dated);
    st.playlists.push(ads);
    let text = toml::to_string(&st).unwrap();
    let back: Station = toml::from_str(&text).unwrap();
    assert_eq!(st, back);
    assert!(matches!(
        back.playlists[1].items[0].source,
        Source::LiveInput { .. }
    ));
}

#[test]
fn serde_defaults_apply() {
    let text = r#"
tz = "UTC"
[[playlists]]
id = 1
name = "Main"
color = [1, 2, 3]
is_main = true
is_fill_pool = false
priority = 0
pick = "InOrder"
rules = []
constraints = []
[[playlists.items]]
id = 1
title = "T"
planned_len = { secs = 10, nanos = 0 }
locked = false
[playlists.items.source.File]
path = "a.mp3"
"#;
    let st: Station = toml::from_str(text).unwrap();
    let p = &st.playlists[0];
    assert!(p.enabled);
    assert!(p.items[0].movable && p.items[0].trimmable);
    st.validate().unwrap();
    let _: &Item = &p.items[0];
}

#[test]
fn validate_accepts_valid() {
    valid_station().validate().unwrap();
    assert_eq!(valid_station().main().id.0, 1);
}

#[test]
fn validate_rejects_no_main() {
    let mut st = valid_station();
    st.playlists[0].is_main = false;
    assert_eq!(st.validate(), Err(ModelError::NoMainPlaylist));
}

#[test]
fn validate_rejects_two_mains() {
    let mut st = valid_station();
    let mut other = playlist(2, "Other", vec![]);
    other.is_main = true;
    st.playlists.push(other);
    assert_eq!(st.validate(), Err(ModelError::MultipleMainPlaylists));
}

#[test]
fn validate_rejects_segue_before_cue_in() {
    let mut st = valid_station();
    let mut bad = track(7, "Bad", "A", 100);
    let t = bad.timing.as_mut().unwrap();
    t.cue_in = Duration::from_secs(50);
    t.segue = Duration::from_secs(40);
    st.playlists[0].items.push(bad);
    assert!(matches!(
        st.validate(),
        Err(ModelError::BadTiming {
            item: ItemId(7),
            ..
        })
    ));
}

#[test]
fn validate_rejects_cue_out_before_segue() {
    let mut st = valid_station();
    let mut bad = track(8, "Bad", "A", 100);
    let t = bad.timing.as_mut().unwrap();
    t.segue = Duration::from_secs(99);
    t.cue_out = Duration::from_secs(90);
    st.playlists[0].items.push(bad);
    assert!(matches!(
        st.validate(),
        Err(ModelError::BadTiming {
            item: ItemId(8),
            ..
        })
    ));
}

#[test]
fn validate_requires_planned_len_for_live_and_stream() {
    let mut st = valid_station();
    let mut l = live(9, 60);
    l.planned_len = None;
    st.playlists[0].items.push(l);
    assert_eq!(st.validate(), Err(ModelError::MissingPlannedLen(ItemId(9))));

    let mut st = valid_station();
    let mut s = track(10, "S", "A", 60);
    s.source = Source::Stream {
        url: "http://x/y".into(),
        keep_alive: true,
    };
    s.timing = None;
    s.planned_len = None;
    st.playlists[0].items.push(s);
    assert_eq!(
        st.validate(),
        Err(ModelError::MissingPlannedLen(ItemId(10)))
    );
}

#[test]
fn file_without_timing_needs_planned_len() {
    let mut st = valid_station();
    let mut f = track(11, "F", "A", 60);
    f.timing = None;
    st.playlists[0].items.push(f.clone());
    assert_eq!(
        st.validate(),
        Err(ModelError::MissingPlannedLen(ItemId(11)))
    );
    st.playlists[0].items.last_mut().unwrap().planned_len = Some(Duration::from_secs(60));
    st.validate().unwrap();
}

#[test]
fn validate_rejects_unknown_tz() {
    let mut st = valid_station();
    st.tz = "Mars/Base".into();
    assert_eq!(
        st.validate(),
        Err(ModelError::UnknownTimeZone("Mars/Base".into()))
    );
}

#[test]
fn validate_rejects_duplicate_item_ids() {
    let mut st = valid_station();
    let mut other = playlist(2, "Other", vec![track(1, "Dup", "A", 60)]);
    other.is_main = false;
    st.playlists.push(other);
    assert_eq!(st.validate(), Err(ModelError::DuplicateId(1)));
}

#[test]
fn an_item_saved_before_pictures_loads_with_none() {
    let meta: radio_model::ItemMeta =
        toml::from_str("title = \"Echo\"\nartist = \"Cloud\"").unwrap();
    assert!(meta.picture.is_empty());
    assert_eq!(meta.title, "Echo");
}
