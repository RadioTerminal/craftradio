use crate::fixtures::*;
use radio_engine::Command;
use radio_playout::{Playout, WEBRTC_PART};
use radio_stream::{WebRtcConfig, WebRtcStatus};
use std::time::{Duration, Instant};

fn switch(p: &Playout, on: bool) {
    p.dispatch(Command::SettingsSet {
        part: WEBRTC_PART.into(),
        key: "enabled".into(),
        value: on.into(),
    })
    .unwrap();
}

/// Ticks playout until the WebRTC state passes `check`.
fn until(
    p: &Playout,
    clock: &radio_playout::ManualClock,
    check: impl Fn(Option<&WebRtcStatus>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        run(p, clock, 0.05);
        if check(p.snapshot().outputs.webrtc.as_ref()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "WebRTC stayed {:?}",
            p.snapshot().outputs.webrtc
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn listening(s: Option<&WebRtcStatus>) -> bool {
    matches!(s, Some(WebRtcStatus::Listening { .. }))
}

/// The setting turns the listener server on and off while the station plays, and it
/// starts again on the same port with the same audio.
#[test]
fn the_setting_turns_webrtc_listening_on_and_off_and_on_again() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (p, clock) = offline(engine_with(Vec::new()), |cfg| {
        let mut w = WebRtcConfig::new(([127, 0, 0, 1], port).into());
        w.stun.clear();
        w.trackers.clear();
        cfg.webrtc = Some(w);
    });
    run(&p, &clock, 0.2);
    assert_eq!(p.snapshot().outputs.webrtc, None, "it starts off");

    switch(&p, true);
    until(&p, &clock, listening);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());

    switch(&p, false);
    until(&p, &clock, |s| s.is_none());

    switch(&p, true);
    until(&p, &clock, listening);
}

/// With the server on and nobody listening, nothing is sent, so the WebRTC meter reads
/// silence even while Program plays.
#[test]
fn webrtc_meter_is_silent_with_no_listeners() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let song = file_item(
        93,
        "Meter test",
        &wav("webrtc-meter", 10.0, Side::Left),
        10.0,
        10.0,
        0.0,
    );
    let (p, clock) = offline(engine_with(vec![song]), |cfg| {
        let mut w = WebRtcConfig::new(([127, 0, 0, 1], port).into());
        w.stun.clear();
        w.trackers.clear();
        cfg.webrtc = Some(w);
    });
    switch(&p, true);
    until(&p, &clock, listening);
    let meters = p.output_meters();
    let _ = meters.webrtc.take();
    let audio = run(&p, &clock, 0.5);
    assert!(audio.iter().any(|x| x.abs() > 0.1), "Program plays");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(meters.webrtc.take(), [radio_stream::LEVEL_FLOOR_DB; 2]);
    assert_eq!(
        meters.device.take(),
        [radio_stream::LEVEL_FLOOR_DB; 2],
        "no sound card"
    );
}

/// The body of `GET path` on the listener server.
fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut conn = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        conn,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    conn.read_to_string(&mut reply).unwrap();
    reply
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_owned()
}

fn set_branding(p: &Playout, key: &str, value: &str) {
    p.dispatch(Command::SettingsSet {
        part: radio_model::BRANDING_PART.into(),
        key: key.into(),
        value: value.into(),
    })
    .unwrap();
}

fn branded_station() -> (u16, Playout, radio_playout::ManualClock) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (p, clock) = offline(engine_with(Vec::new()), |cfg| {
        let mut w = WebRtcConfig::new(([127, 0, 0, 1], port).into());
        w.stun.clear();
        w.trackers.clear();
        cfg.webrtc = Some(w);
        cfg.webrtc_always = true;
    });
    until(&p, &clock, listening);
    (port, p, clock)
}

/// The brand that listeners get follows the station name in the settings.
#[test]
fn the_brand_follows_the_settings() {
    let (port, p, clock) = branded_station();
    for name in ["Radio Lipa", "Radio Hrast"] {
        set_branding(&p, radio_model::BRANDING_NAME_KEY, name);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !http_get(port, "/brand").contains(name) {
            assert!(Instant::now() < deadline, "the brand never became {name}");
            run(&p, &clock, 0.05);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A logo that is not a picture raises an alarm that names the file.
#[test]
fn a_logo_that_does_not_load_raises_an_alarm() {
    let (_port, p, clock) = branded_station();
    let alarms = p.alarms().unwrap();
    let logo = std::env::temp_dir().join("craftradio-not-a-picture.txt");
    std::fs::write(&logo, "this is text, not a picture").unwrap();
    let logo = logo.to_string_lossy().into_owned();
    set_branding(&p, radio_model::BRANDING_LOGO_KEY, &logo);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        run(&p, &clock, 0.05);
        let found = alarms.try_iter().any(
            |a| matches!(&a.alarm, radio_playout::Alarm::PictureFailed { path, .. } if *path == logo),
        );
        if found {
            return;
        }
        assert!(Instant::now() < deadline, "no picture alarm was raised");
        std::thread::sleep(Duration::from_millis(20));
    }
}
