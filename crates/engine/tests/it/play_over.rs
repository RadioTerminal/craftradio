//! `item.set_markers` and `playlist.set_plays`, dispatched by id as the CLI and MCP do.

use radio_engine::{Command, Engine};
use radio_model::{Item, ItemId, MarkerSource, PlayMode, PlaylistId, Source, Timing};
use serde_json::json;
use std::time::Duration;

const MAIN: PlaylistId = PlaylistId(1);

fn song(id: u64) -> Item {
    Item {
        id: ItemId(id),
        title: "Paper Lanterns".into(),
        artist: Some("The Quiet Hours".into()),
        source: Source::File {
            path: "/music/lanterns.wav".into(),
        },
        timing: Some(Timing {
            cue_in: Duration::from_secs(1),
            segue: Duration::from_secs(170),
            cue_out: Duration::from_secs(175),
            intro_end: None,
            fade_in: Duration::ZERO,
            fade_out: Duration::ZERO,
            outro_start: None,
            markers: MarkerSource::NotAnalysed,
        }),
        planned_len: Some(Duration::from_secs(174)),
        movable: true,
        trimmable: true,
        locked: false,
        valid_from: None,
        valid_to: None,
        max_per_day: None,
        meta: Default::default(),
    }
}

fn engine() -> Engine {
    let mut e = Engine::open_in_memory().unwrap();
    e.dispatch(Command::PlaylistAddItems {
        playlist: MAIN,
        items: vec![song(7)],
        at: None,
        lock: false,
    })
    .unwrap();
    e
}

fn run(e: &mut Engine, id: &str, args: serde_json::Value) -> Result<(), String> {
    let cmd: Command = serde_json::from_value(json!({"id": id, "args": args})).unwrap();
    e.dispatch(cmd).map(|_| ()).map_err(|err| err.to_string())
}

fn secs(n: u64) -> serde_json::Value {
    json!({"secs": n, "nanos": 0})
}

fn timing(e: &Engine) -> Timing {
    e.state().station.main().items[0].timing.clone().unwrap()
}

#[test]
fn markers_set_by_hand_survive_a_detection() {
    let mut e = engine();
    run(
        &mut e,
        "item.set_markers",
        json!({"item": 7, "intro_end": secs(12), "outro_start": secs(150), "markers": "ByHand"}),
    )
    .unwrap();
    run(
        &mut e,
        "item.set_markers",
        json!({"item": 7, "intro_end": secs(20), "outro_start": null, "markers": "Detected"}),
    )
    .unwrap();
    let t = timing(&e);
    assert_eq!(t.intro_end, Some(Duration::from_secs(12)));
    assert_eq!(t.outro_start, Some(Duration::from_secs(150)));
    assert_eq!(t.markers, MarkerSource::ByHand);
}

#[test]
fn a_detection_sets_markers_and_keeps_the_cue_points() {
    let mut e = engine();
    let before = timing(&e);
    run(
        &mut e,
        "item.set_markers",
        json!({"item": 7, "intro_end": secs(9), "outro_start": secs(160), "markers": "Detected"}),
    )
    .unwrap();
    let t = timing(&e);
    assert_eq!(t.intro_end, Some(Duration::from_secs(9)));
    assert_eq!(t.markers, MarkerSource::Detected);
    assert_eq!(
        (t.cue_in, t.segue, t.cue_out),
        (before.cue_in, before.segue, before.cue_out)
    );
}

#[test]
fn markers_outside_the_song_are_refused() {
    let mut e = engine();
    for (intro, outro) in [
        (secs(0), json!(null)),
        (json!(null), secs(172)),
        (secs(160), secs(150)),
    ] {
        let err = run(
            &mut e,
            "item.set_markers",
            json!({"item": 7, "intro_end": intro, "outro_start": outro, "markers": "ByHand"}),
        );
        assert!(err.is_err(), "{intro} / {outro} was taken");
    }
    assert_eq!(timing(&e).markers, MarkerSource::NotAnalysed);
}

#[test]
fn set_plays_changes_the_playlist_and_refuses_a_boost() {
    let mut e = engine();
    run(
        &mut e,
        "playlist.set_plays",
        json!({"playlist": 1, "plays": {"Over": {"duck_db": -9}}}),
    )
    .unwrap();
    assert_eq!(
        e.state().station.main().plays,
        PlayMode::Over { duck_db: -9 }
    );
    for db in [3, -41] {
        let err = run(
            &mut e,
            "playlist.set_plays",
            json!({"playlist": 1, "plays": {"Over": {"duck_db": db}}}),
        );
        assert!(err.is_err(), "{db} dB was taken");
    }
    run(
        &mut e,
        "playlist.set_plays",
        json!({"playlist": 1, "plays": "InSequence"}),
    )
    .unwrap();
    assert_eq!(e.state().station.main().plays, PlayMode::InSequence);
}
