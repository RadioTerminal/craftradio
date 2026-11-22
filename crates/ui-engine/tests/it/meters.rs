//! The peak hold of Preferences › Meters reaches the playout's mixer at the start.

use radio_engine::{Command, Engine};
use radio_ui_engine::peak_hold;
use std::time::Duration;

#[test]
fn the_peak_hold_that_preferences_kept_is_the_one_the_playout_starts_with() {
    let mut engine = Engine::open_in_memory().expect("engine");
    assert_eq!(
        peak_hold(engine.state()),
        None,
        "none kept: the playout keeps its own"
    );
    engine
        .dispatch(Command::SettingsSet {
            part: "mixer".into(),
            key: "peak_hold_s".into(),
            value: serde_json::json!(3.0),
        })
        .expect("set");
    assert_eq!(peak_hold(engine.state()), Some(Duration::from_secs(3)));
}
