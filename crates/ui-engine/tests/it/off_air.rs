//! What the adapter shows while the station is off air, and while it goes back on air.

use crate::fixtures::*;
use radio_engine::{Command, Engine};
use radio_playout::{Clock, ManualClock, OutputChoice, Playout, PlayoutConfig};
use radio_ui_egui::view::{Edit, OutputKind, Status, ViewSource};
use radio_ui_engine::{EngineSource, SourceOptions};
use std::net::TcpListener;
use std::sync::Arc;

/// An Icecast config for a port where nobody listens, so a source never connects.
fn dead_icecast() -> radio_stream::IcecastConfig {
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    radio_stream::IcecastConfig {
        host: "127.0.0.1".into(),
        port,
        mount: "/live".into(),
        user: "source".into(),
        password: "test-only".into(),
        name: STATION.into(),
        description: "Test source".into(),
        genre: "Test".into(),
        format: radio_stream::StreamFormat::Opus { kbps: 96 },
        tls: false,
    }
}

/// A station with a dead Icecast mount, on air or off air as saved.
fn station(on_air: bool) -> (EngineSource, ManualClock) {
    let mut engine = Engine::open_in_memory().unwrap();
    engine
        .dispatch(Command::SettingsSet {
            part: radio_model::AIR_PART.into(),
            key: radio_model::ON_AIR_KEY.into(),
            value: on_air.into(),
        })
        .unwrap();
    let clock = ManualClock::new(t0());
    let mut cfg = PlayoutConfig::new(Arc::new(clock.clone()), OutputChoice::Null);
    cfg.station_name = STATION.into();
    cfg.icecast = Some(dead_icecast());
    let playout = Playout::start(engine, cfg).unwrap();
    let opts = SourceOptions {
        tz: jiff::tz::TimeZone::UTC,
        station_name: STATION.into(),
    };
    (EngineSource::new(playout, opts), clock)
}

fn set_on_air(source: &mut EngineSource, on: bool) {
    source
        .edit(Edit::SetPartSetting {
            part: radio_model::AIR_PART.into(),
            key: radio_model::ON_AIR_KEY.into(),
            value: on.into(),
        })
        .expect("the setting is taken");
}

#[test]
fn off_air_strips_say_off_air_and_are_not_live() {
    let (mut source, clock) = station(false);
    let view = wait_for(&mut source, &clock, |v| v.output.icecast == Status::OffAir);
    assert!(!view.on_air);
    assert!(!view.going_on_air);
    assert_eq!(view.output.icecast, Status::OffAir);
    assert_eq!(view.output.webrtc, Status::OffAir);
    for kind in [OutputKind::Icecast, OutputKind::WebRtc] {
        let strip = view
            .output_meters
            .iter()
            .find(|m| m.kind == kind)
            .expect("the strip");
        assert!(!strip.live, "{kind:?} is not live off air");
        assert_eq!(strip.note, "Off air", "{kind:?} says why");
    }
}

#[test]
fn going_on_air_while_icecast_connects() {
    let (mut source, clock) = station(false);
    wait_for(&mut source, &clock, |v| v.output.icecast == Status::OffAir);
    set_on_air(&mut source, true);
    let view = wait_for(&mut source, &clock, |v| {
        v.on_air && v.output.icecast != Status::OffAir
    });
    assert!(view.on_air);
    assert_ne!(view.output.icecast, Status::OffAir);
    assert!(view.going_on_air, "Icecast has not connected yet");

    // Thirty seconds on, a mount that never connects is a fault, not "going on air".
    clock.advance(std::time::Duration::from_secs(31));
    let view = source.snapshot(clock.now());
    assert!(view.on_air);
    assert!(!view.going_on_air);
}

#[test]
fn a_station_that_started_on_air_is_not_going_on_air() {
    let (mut source, clock) = station(true);
    let view = wait_for(&mut source, &clock, |v| v.on_air);
    assert!(view.on_air);
    assert!(!view.going_on_air);
}

fn set_icecast_enabled(source: &mut EngineSource, on: bool) {
    source
        .edit(Edit::SetPartSetting {
            part: radio_model::ICECAST_PART.into(),
            key: radio_model::ICECAST_ENABLED_KEY.into(),
            value: on.into(),
        })
        .expect("the setting is taken");
}

/// Icecast switched off on air says `Off`, never live and never off air; off air still wins.
#[test]
fn icecast_switched_off_says_off_and_off_air_still_wins() {
    let (mut source, clock) = station(true);
    set_icecast_enabled(&mut source, false);
    let view = wait_for(&mut source, &clock, |v| v.output.icecast == Status::Off);
    assert!(view.on_air);
    assert!(
        !view.going_on_air,
        "a switched-off Icecast is not going on air"
    );
    let strip = view
        .output_meters
        .iter()
        .find(|m| m.kind == OutputKind::Icecast)
        .expect("the strip");
    assert!(!strip.live);
    assert_eq!(strip.note, "Off");
    assert!(
        !view.alarms.iter().any(|a| a.text.contains("Icecast")),
        "a switched-off Icecast raises no alarm"
    );

    set_on_air(&mut source, false);
    let view = wait_for(&mut source, &clock, |v| v.output.icecast == Status::OffAir);
    let strip = view
        .output_meters
        .iter()
        .find(|m| m.kind == OutputKind::Icecast)
        .expect("the strip");
    assert_eq!(strip.note, "Off air");
}
