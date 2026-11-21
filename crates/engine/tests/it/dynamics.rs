//! The compressor and the limiter on Program: `mixer.set_dynamics`.

use radio_engine::{AudioIntent, Command, CompressorPatch, Engine, EngineEvent, LimiterPatch};
use radio_model::DynamicsSettings;

use super::temp_db;

fn set(compressor: Option<CompressorPatch>, limiter: Option<LimiterPatch>) -> Command {
    Command::MixerSetDynamics {
        compressor,
        limiter,
    }
}

/// A new database has no dynamics row, so playout keeps its own default.
#[test]
fn a_new_station_has_no_dynamics_setting() {
    let path = temp_db("dynamics-none");
    {
        let mut e = Engine::open(&path).unwrap();
        e.dispatch(Command::PlaylistCreate {
            name: "Jingles".into(),
        })
        .unwrap();
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.state().dynamics, None);
}

/// The setting survives a reopen, and the fields that the command left out keep the
/// default.
#[test]
fn the_dynamics_setting_survives_reopen() {
    let path = temp_db("dynamics-reopen");
    {
        let mut e = Engine::open(&path).unwrap();
        // A station on disk: the first checkpoint writes it.
        e.dispatch(Command::PlaylistCreate {
            name: "Jingles".into(),
        })
        .unwrap();
        let compressor = CompressorPatch {
            enabled: Some(true),
            ratio: Some(3.0),
            ..CompressorPatch::default()
        };
        let limiter = LimiterPatch {
            ceiling_db: Some(-2.0),
            ..LimiterPatch::default()
        };
        e.dispatch(set(Some(compressor), Some(limiter))).unwrap();
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    let d = e.state().dynamics.expect("a stored setting");
    let default = DynamicsSettings::default();
    assert!(d.compressor.enabled);
    assert_eq!(d.compressor.ratio, 3.0);
    assert_eq!(d.limiter.ceiling_db, -2.0);
    assert_eq!(d.compressor.threshold_db, default.compressor.threshold_db);
    assert_eq!(d.limiter.enabled, default.limiter.enabled);
}

/// The command hands the merged setting to the audio glue.
#[test]
fn the_setting_goes_to_the_audio_glue() {
    let mut e = Engine::open_in_memory().unwrap();
    let events = e.events();
    let limiter = LimiterPatch {
        enabled: Some(false),
        ..LimiterPatch::default()
    };
    e.dispatch(set(None, Some(limiter))).unwrap();
    let sent = events.try_iter().find_map(|ev| match ev {
        EngineEvent::AudioIntent(AudioIntent::SetDynamics(d)) => Some(d),
        _ => None,
    });
    assert!(sent.is_some_and(|d| !d.limiter.enabled));
}

/// A value out of range, or not a number, is refused with an error, and the setting in
/// force stays.
#[test]
fn bad_values_are_refused_and_change_nothing() {
    let mut e = Engine::open_in_memory().unwrap();
    for patch in [
        CompressorPatch {
            ratio: Some(0.5),
            ..CompressorPatch::default()
        },
        CompressorPatch {
            attack_ms: Some(f32::NAN),
            ..CompressorPatch::default()
        },
        CompressorPatch {
            threshold_db: Some(6.0),
            ..CompressorPatch::default()
        },
    ] {
        assert!(e.dispatch(set(Some(patch), None)).is_err(), "{patch:?}");
    }
    let ceiling = LimiterPatch {
        ceiling_db: Some(3.0),
        ..LimiterPatch::default()
    };
    assert!(e.dispatch(set(None, Some(ceiling))).is_err());
    assert_eq!(e.state().dynamics, None, "nothing was stored");
}

/// The command also arrives as JSON, as the CLI and MCP send it.
#[test]
fn the_command_parses_from_json() {
    let json = serde_json::json!({
        "id": "mixer.set_dynamics",
        "args": {"compressor": {"enabled": true, "threshold_db": -18.0}}
    });
    let cmd: Command = serde_json::from_value(json).unwrap();
    assert_eq!(cmd.id(), "mixer.set_dynamics");
    let unknown = serde_json::json!({
        "id": "mixer.set_dynamics",
        "args": {"compressor": {"knee_db": 6.0}}
    });
    assert!(serde_json::from_value::<Command>(unknown).is_err());
}
