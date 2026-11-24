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

/// A file that goes away with the test. Its name is unique to the process and the test.
struct Temp(std::path::PathBuf);

impl Temp {
    fn new(test: &str, ext: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "craftradio-webrtc-it-{test}-{}.{ext}",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }

    fn text(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A 1 by 1 PNG picture.
const PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x38, 0x11, 0xb5, 0x00,
    0x00, 0x03, 0xb0, 0x01, 0xc3, 0x1a, 0x0f, 0x0d, 0x28, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

fn branded_station() -> (u16, Playout, radio_playout::ManualClock) {
    branded_station_with(|_| {})
}

fn branded_station_with(
    tune: impl FnOnce(&mut radio_playout::PlayoutConfig),
) -> (u16, Playout, radio_playout::ManualClock) {
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
        tune(cfg);
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
    let file = Temp::new("not-a-picture", "txt", b"this is text, not a picture");
    let logo = file.text();
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

/// A station whose one item airs at once with the given publish mode and picture. The
/// picture is a text file with a picture name, so it never loads and says so with an alarm.
fn station_with_picture(
    name: &str,
    publish: radio_model::PublishMode,
) -> (Temp, Playout, radio_playout::ManualClock) {
    let file = Temp::new(name, "png", b"text, not a picture");
    let picture = file.text();
    let mut item = file_item(1, "Show", &wav(name, 2.0, Side::Left), 2.0, 2.0, 0.0);
    item.meta.title = "Show".into();
    item.meta.publish = publish;
    item.meta.custom = "Our show".into();
    item.meta.picture.clone_from(&picture);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (p, clock) = offline(engine_with(vec![item]), |cfg| {
        let mut w = WebRtcConfig::new(([127, 0, 0, 1], port).into());
        w.stun.clear();
        w.trackers.clear();
        cfg.webrtc = Some(w);
        cfg.webrtc_always = true;
    });
    (file, p, clock)
}

/// True when an alarm about `picture` arrives while 1.5 s of the station play.
fn picture_alarm(p: &Playout, clock: &radio_playout::ManualClock, picture: &str) -> bool {
    let alarms = p.alarms().unwrap();
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        run(p, clock, 0.05);
        if alarms.try_iter().any(
            |a| matches!(&a.alarm, radio_playout::Alarm::PictureFailed { path, .. } if path == picture),
        ) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// An item's own picture is read when it shows its tags or its own text, and a hidden item
/// never reads it. A picture that cannot load raises an alarm, so the alarm shows which
/// items the station read the picture of.
#[test]
fn a_broken_item_picture_is_reported_in_the_modes_that_show_it() {
    use radio_model::PublishMode::{Custom, FromTags, Hide};
    for (name, mode, read) in [
        ("pic-tags", FromTags, true),
        ("pic-custom", Custom, true),
        ("pic-hide", Hide, false),
    ] {
        let (picture, p, clock) = station_with_picture(name, mode);
        assert_eq!(picture_alarm(&p, &clock, &picture.text()), read, "{name}");
    }
}

/// A picture that is not one raises an alarm that names the file.
#[test]
fn a_broken_item_picture_raises_an_alarm() {
    let (picture, p, clock) = station_with_picture("pic-broken", radio_model::PublishMode::Custom);
    assert!(
        picture_alarm(&p, &clock, &picture.text()),
        "no picture alarm"
    );
}

/// The open picture alarms of `path`, from the snapshot.
fn open_picture_alarms(p: &Playout, path: &str) -> usize {
    p.snapshot()
        .alarms
        .iter()
        .filter(|a| {
            a.cleared.is_none()
                && matches!(&a.alarm, radio_playout::Alarm::PictureFailed { path: x, .. } if x == path)
        })
        .count()
}

/// Ticks playout until `check` holds, up to a bound.
fn tick_until(
    p: &Playout,
    clock: &radio_playout::ManualClock,
    what: &str,
    check: impl Fn() -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "never: {what}");
        run(p, clock, 0.05);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A logo file that is overwritten at the same path is read again.
#[test]
fn a_logo_file_that_changes_on_disk_is_sent_again() {
    let (port, p, clock) = branded_station();
    let file = Temp::new("logo-changes", "png", b"this is text, not a picture");
    let logo = file.text();
    set_branding(&p, radio_model::BRANDING_LOGO_KEY, &logo);
    tick_until(&p, &clock, "the alarm for the text file", || {
        open_picture_alarms(&p, &logo) > 0
    });
    assert!(http_get(port, "/brand").contains("\"logo\":null"));
    std::fs::write(&file.0, PNG).unwrap();
    tick_until(&p, &clock, "a logo after the file changed", || {
        http_get(port, "/brand").contains("data:image/")
    });
    // The alarm of the half-written or broken file does not stay open on a logo that works.
    tick_until(&p, &clock, "no open alarm for the working logo", || {
        open_picture_alarms(&p, &logo) == 0
    });
}

/// A new station name leaves the alarm of a logo that does not load as it is.
#[test]
fn a_name_change_does_not_raise_the_logo_alarm_again() {
    let (port, p, clock) = branded_station();
    let file = Temp::new("logo-name", "png", b"this is text, not a picture");
    let logo = file.text();
    set_branding(&p, radio_model::BRANDING_LOGO_KEY, &logo);
    tick_until(&p, &clock, "the alarm for the text file", || {
        open_picture_alarms(&p, &logo) > 0
    });
    let raised = || {
        p.snapshot()
            .alarms
            .iter()
            .filter(|a| matches!(&a.alarm, radio_playout::Alarm::PictureFailed { path, .. } if *path == logo))
            .count()
    };
    let before = raised();
    set_branding(&p, radio_model::BRANDING_NAME_KEY, "Radio Kresnica");
    tick_until(&p, &clock, "the new name", || {
        http_get(port, "/brand").contains("Radio Kresnica")
    });
    // Long enough for the logo to be read again and to fail again.
    let ticks = std::cell::Cell::new(0);
    tick_until(&p, &clock, "a few more ticks", || {
        ticks.set(ticks.get() + 1);
        ticks.get() > 20
    });
    assert_eq!(raised(), before, "the same alarm, not a new one");
    assert_eq!(open_picture_alarms(&p, &logo), 1, "and it is still open");
}

/// The same broken picture on air twice leaves one open alarm.
#[test]
fn the_same_broken_picture_airing_twice_raises_one_alarm() {
    let file = Temp::new("twice", "png", b"text, not a picture");
    let picture = file.text();
    let items: Vec<_> = (1..=2)
        .map(|n| {
            let mut item = file_item(
                n,
                "Show",
                &wav(&format!("twice-{n}"), 1.0, Side::Left),
                1.0,
                1.0,
                0.0,
            );
            item.meta.title = format!("Show {n}");
            item.meta.publish = radio_model::PublishMode::FromTags;
            item.meta.picture.clone_from(&picture);
            item
        })
        .collect();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (p, clock) = offline(engine_with(items), |cfg| {
        let mut w = WebRtcConfig::new(([127, 0, 0, 1], port).into());
        w.stun.clear();
        w.trackers.clear();
        cfg.webrtc = Some(w);
        cfg.webrtc_always = true;
    });
    // Both items air, and the picture is read for each.
    run(&p, &clock, 3.0);
    tick_until(&p, &clock, "the picture alarm", || {
        open_picture_alarms(&p, &picture) > 0
    });
    run(&p, &clock, 1.0);
    std::thread::sleep(Duration::from_millis(300));
    run(&p, &clock, 0.1);
    assert_eq!(open_picture_alarms(&p, &picture), 1);
}

/// Putting a working logo in place of a broken one clears the alarm of the broken one.
#[test]
fn a_fixed_logo_clears_its_alarm() {
    let (_port, p, clock) = branded_station();
    let bad = Temp::new("fix-bad", "png", b"this is text, not a picture");
    let good = Temp::new("fix-good", "png", &PNG);
    set_branding(&p, radio_model::BRANDING_LOGO_KEY, &bad.text());
    tick_until(&p, &clock, "the alarm for the broken logo", || {
        open_picture_alarms(&p, &bad.text()) > 0
    });
    set_branding(&p, radio_model::BRANDING_LOGO_KEY, &good.text());
    tick_until(&p, &clock, "the alarm cleared", || {
        open_picture_alarms(&p, &bad.text()) == 0
    });
}

/// With no name set, the brand has an empty name, so the page shows its own fallback.
#[test]
fn the_brand_has_no_name_until_one_is_set() {
    let (port, p, clock) = branded_station_with(|cfg| cfg.station_name = "Radio Config".into());
    tick_until(&p, &clock, "a brand", || {
        http_get(port, "/brand").contains("\"name\"")
    });
    let brand: serde_json::Value = serde_json::from_str(&http_get(port, "/brand")).unwrap();
    assert_eq!(brand["name"], "");
}

/// Off air stops the listener server even when the config runs it in any case, and on
/// air starts it again.
#[test]
fn off_air_stops_webrtc_even_when_always_on() {
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

    let air = |on: bool| {
        p.dispatch(Command::SettingsSet {
            part: radio_model::AIR_PART.into(),
            key: radio_model::ON_AIR_KEY.into(),
            value: on.into(),
        })
        .unwrap();
    };
    air(false);
    until(&p, &clock, |s| s.is_none());

    air(true);
    until(&p, &clock, listening);
}
