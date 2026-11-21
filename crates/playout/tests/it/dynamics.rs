//! `mixer.set_dynamics` reaches the mixer, live and after a restart.

use radio_engine::{Command, LimiterPatch};

use crate::fixtures::{SR, Side, engine_with, file_item, offline, peak, run, wav};

/// A ceiling far under the default, so its effect shows in Program.
const LOW_CEILING_DB: f32 = -6.0;

fn low_ceiling() -> Command {
    Command::MixerSetDynamics {
        compressor: None,
        limiter: Some(LimiterPatch {
            ceiling_db: Some(LOW_CEILING_DB),
            ..LimiterPatch::default()
        }),
    }
}

fn hot_out() -> Command {
    Command::MixerSetStrip {
        strip: "Out".into(),
        gain_db: Some(12.0),
        mute: None,
        pfl: None,
        route: None,
    }
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// A new ceiling applies while the station plays, without a restart.
#[test]
fn a_new_ceiling_applies_live() {
    let path = wav("hot", 3.0, Side::Both);
    let mut engine = engine_with(vec![file_item(1, "Song", &path, 3.0, 3.0, 0.0)]);
    engine.dispatch(hot_out()).unwrap();
    let (p, clock) = offline(engine, |_| {});
    let before = run(&p, &clock, 0.5);
    let loud = db(peak(&before, 0, SR / 4, SR / 2));
    assert!(loud > LOW_CEILING_DB + 3.0, "Program starts at {loud} dBFS");
    let _ = p.send(low_ceiling()).unwrap();
    let after = run(&p, &clock, 0.5);
    let limited = db(peak(&after, 0, SR / 4, SR / 2));
    assert!(
        limited <= LOW_CEILING_DB + 0.1,
        "Program reaches {limited} dBFS after the change"
    );
}

/// A stored ceiling holds after a restart.
#[test]
fn a_stored_ceiling_holds_after_a_restart() {
    let path = wav("hot-stored", 2.0, Side::Both);
    let mut engine = engine_with(vec![file_item(1, "Song", &path, 2.0, 2.0, 0.0)]);
    engine.dispatch(hot_out()).unwrap();
    engine.dispatch(low_ceiling()).unwrap();
    let (p, clock) = offline(engine, |_| {});
    let program = run(&p, &clock, 0.5);
    let limited = db(peak(&program, 0, SR / 10, SR / 2));
    assert!(
        limited <= LOW_CEILING_DB + 0.1,
        "Program reaches {limited} dBFS at the start"
    );
}
