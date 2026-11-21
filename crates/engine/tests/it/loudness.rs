//! Loudness normalisation: `mixer.set_loudness`, `item.set_loudness` (the measurement
//! cache) and `item.set_gain_trim`.

use radio_engine::{AudioIntent, Command, Engine, EngineEvent};
use radio_model::{ItemId, LoudnessSettings, PlaylistId, Source};

use super::temp_db;

fn file_item(id: u64) -> radio_model::Item {
    let mut item = super::item(id, "Paper Tide");
    item.source = Source::File {
        path: "paper-tide.wav".into(),
    };
    item
}

fn engine_with_item(e: &mut Engine) {
    e.dispatch(Command::PlaylistAddItems {
        playlist: PlaylistId(1),
        items: vec![file_item(1), file_item(2)],
        at: None,
        lock: true,
    })
    .unwrap();
}

fn set(enabled: Option<bool>, target: Option<f32>, boost: Option<f32>) -> Command {
    Command::MixerSetLoudness {
        enabled,
        target_lufs: target,
        max_boost_db: boost,
    }
}

fn measure(item: u64, hash: &str, lufs: f32) -> Command {
    Command::ItemSetLoudness {
        item: ItemId(item),
        hash: hash.into(),
        size: 10,
        lufs,
        peak_db: Some(-3.0),
    }
}

/// A new station normalises, to the default target.
#[test]
fn a_new_station_normalises_by_default() {
    let e = Engine::open_in_memory().unwrap();
    assert_eq!(e.state().loudness, LoudnessSettings::default());
    assert!(e.state().loudness.enabled);
}

/// The settings survive a reopen, and the fields the command left out keep their value.
#[test]
fn the_loudness_settings_survive_reopen() {
    let path = temp_db("loudness-reopen");
    {
        let mut e = Engine::open(&path).unwrap();
        engine_with_item(&mut e);
        e.dispatch(set(Some(false), Some(-23.0), None)).unwrap();
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    let l = e.state().loudness;
    assert!(!l.enabled);
    assert_eq!(l.target_lufs, -23.0);
    assert_eq!(l.max_boost_db, LoudnessSettings::default().max_boost_db);
}

/// A change goes to the audio glue, so playout applies it live.
#[test]
fn a_settings_change_goes_to_the_audio_glue() {
    let mut e = Engine::open_in_memory().unwrap();
    let events = e.events();
    e.dispatch(set(None, Some(-20.0), None)).unwrap();
    let sent = events.try_iter().find_map(|ev| match ev {
        EngineEvent::AudioIntent(AudioIntent::SetLoudness(l)) => Some(l),
        _ => None,
    });
    assert!(sent.is_some_and(|l| l.target_lufs == -20.0));
}

/// Values out of range, or not numbers, are refused and change nothing.
#[test]
fn bad_loudness_values_are_refused() {
    let mut e = Engine::open_in_memory().unwrap();
    for cmd in [
        set(None, Some(-40.0), None),
        set(None, Some(f32::NAN), None),
        set(None, None, Some(20.0)),
        set(None, None, Some(-1.0)),
    ] {
        assert!(e.dispatch(cmd).is_err());
    }
    assert_eq!(e.state().loudness, LoudnessSettings::default());
}

/// A measurement is kept by file hash, survives a reopen, and needs no new plan.
#[test]
fn a_measurement_is_cached_by_hash_and_survives_reopen() {
    let path = temp_db("loudness-cache");
    {
        let mut e = Engine::open(&path).unwrap();
        engine_with_item(&mut e);
        let events = e.events();
        e.dispatch(measure(1, "abc", -12.5)).unwrap();
        let replan = events
            .try_iter()
            .any(|ev| matches!(ev, EngineEvent::StateChanged));
        assert!(!replan, "a measurement does not change the plan");
        e.flush().unwrap();
    }
    let mut e = Engine::open(&path).unwrap();
    let got = e.state().loudness_of(ItemId(1)).expect("cached");
    assert_eq!(got.lufs, -12.5);
    assert!(e.state().loudness_of(ItemId(2)).is_none());
    // The same file on another item shares the measurement.
    e.dispatch(Command::ItemSetHash {
        item: ItemId(2),
        hash: "abc".into(),
        size: 10,
    })
    .unwrap();
    assert!(e.state().loudness_of(ItemId(2)).is_some());
}

/// A measurement that is not a number is refused.
#[test]
fn a_bad_measurement_is_refused() {
    let mut e = Engine::open_in_memory().unwrap();
    engine_with_item(&mut e);
    assert!(e.dispatch(measure(1, "abc", f32::NAN)).is_err());
    assert!(
        e.dispatch(measure(9, "abc", -12.0)).is_err(),
        "unknown item"
    );
    assert!(e.state().loudness_of(ItemId(1)).is_none());
}

/// The trim of an item survives a reopen, undo takes it back, and a bad trim is refused.
#[test]
fn the_item_trim_persists_and_undoes() {
    let path = temp_db("loudness-trim");
    {
        let mut e = Engine::open(&path).unwrap();
        engine_with_item(&mut e);
        let trim = |db| Command::ItemSetGainTrim {
            item: ItemId(1),
            db,
        };
        e.dispatch(trim(-2.5)).unwrap();
        assert!(e.dispatch(trim(40.0)).is_err());
        assert!(e.dispatch(trim(f32::INFINITY)).is_err());
        e.dispatch(trim(1.5)).unwrap();
        e.dispatch(Command::HistoryUndo).unwrap();
        assert_eq!(e.state().trim_of(ItemId(1)), -2.5, "undo restores");
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.state().trim_of(ItemId(1)), -2.5);
    assert_eq!(e.state().trim_of(ItemId(2)), 0.0);
}

/// The commands arrive as JSON, as the CLI and MCP send them.
#[test]
fn the_loudness_commands_parse_from_json() {
    let json = serde_json::json!({"id": "mixer.set_loudness", "args": {"target_lufs": -16.0}});
    let cmd: Command = serde_json::from_value(json).unwrap();
    assert_eq!(cmd.id(), "mixer.set_loudness");
    let json = serde_json::json!({"id": "item.set_gain_trim", "args": {"item": 1, "db": -1.0}});
    let cmd: Command = serde_json::from_value(json).unwrap();
    assert_eq!(cmd.id(), "item.set_gain_trim");
}

/// A copy of a playlist keeps the trims and the measured loudness of its items.
#[test]
fn a_duplicated_playlist_keeps_trims_and_loudness() {
    let mut e = Engine::open_in_memory().unwrap();
    engine_with_item(&mut e);
    e.dispatch(measure(1, "abc", -12.0)).unwrap();
    e.dispatch(Command::ItemSetGainTrim {
        item: ItemId(1),
        db: 1.5,
    })
    .unwrap();
    let made = e
        .dispatch(Command::PlaylistDuplicate {
            playlist: PlaylistId(1),
            name: "Copy".into(),
        })
        .unwrap();
    let copy = PlaylistId(made.data.and_then(|v| v.as_u64()).unwrap());
    let state = e.state();
    let first = state
        .station
        .playlists
        .iter()
        .find(|p| p.id == copy)
        .and_then(|p| p.items.first())
        .map(|i| i.id)
        .unwrap();
    assert_ne!(first, ItemId(1));
    assert_eq!(state.trim_of(first), 1.5);
    assert!(state.loudness_of(first).is_some(), "no second measurement");
}
