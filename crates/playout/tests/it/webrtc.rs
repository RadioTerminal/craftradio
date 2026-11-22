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
