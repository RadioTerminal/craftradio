use crate::fixtures::*;
use radio_engine::Command;
use radio_model::Source;
use radio_playout::{Alarm, DeckPhase};
use radio_stream::{StreamStatus, WebRtcConfig};
use std::net::TcpListener;
use std::time::Duration;

/// A server that accepts each connection and closes it at once.
fn dropping_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/live", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            drop(conn);
        }
    });
    url
}

/// A keep-alive stream without audio stays the current item. A bed from the fill pool
/// plays under it, and the next item does not start.
#[test]
fn keep_alive_stream_does_not_advance_when_down() {
    let mut relay = file_item(1, "Relay", "unused", 1.0, 1.0, 0.0);
    relay.source = Source::Stream {
        url: dropping_server(),
        keep_alive: true,
    };
    relay.timing = None;
    relay.planned_len = Some(Duration::from_secs(20));
    let next = file_item(2, "Next", &wav("k-next", 2.0, Side::Right), 2.0, 1.5, 0.0);
    let mut engine = engine_with(vec![relay, next]);
    let bed = file_item(20, "Bed", &wav("k-bed", 3.0, Side::Left), 3.0, 3.0, 0.0);
    let beds = add_playlist(&mut engine, "Beds", vec![bed], "");
    engine
        .dispatch(Command::PlaylistSetFillPool {
            playlist: beds,
            fill_pool: true,
        })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let program = run(&p, &clock, 6.0);

    assert!(
        first_audible(&program, 0, 0).is_some(),
        "no bed under the stream"
    );
    assert!(
        first_audible(&program, 1, 0).is_none(),
        "the next item started"
    );
    let view = p.snapshot();
    let relay_deck = view.decks.iter().find(|d| d.item.is_some_and(|i| i.0 == 1));
    assert!(relay_deck.is_some_and(|d| d.phase == DeckPhase::Playing));
    assert!(view.stream_states.iter().any(|s| s.keep_alive && !s.up));
    assert!(view.alarms.iter().any(|a| {
        matches!(a.alarm, Alarm::StreamReconnecting { item, .. } if item.0 == 1)
            && a.cleared.is_none()
    }));
    let rows = p.history(t0(), 10).unwrap();
    assert!(rows.iter().all(|r| r.entry.title != "Next"));
}

/// A server that answers each connection in plain HTTP, so a TLS client fails at once.
fn plain_text_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("https://{}/live", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            use std::io::Write;
            let _ = conn.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
        }
    });
    url
}

/// The alarm of a keep-alive stream says why its https connection failed.
#[test]
fn a_stream_whose_tls_fails_says_why_in_its_alarm() {
    let mut relay = file_item(1, "Relay", "unused", 1.0, 1.0, 0.0);
    relay.source = Source::Stream {
        url: plain_text_server(),
        keep_alive: true,
    };
    relay.timing = None;
    relay.planned_len = Some(Duration::from_secs(20));
    let engine = engine_with(vec![relay]);
    let (p, clock) = offline(engine, |_| {});
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let reason = loop {
        run(&p, &clock, 0.5);
        let found = p.snapshot().alarms.iter().find_map(|a| match &a.alarm {
            Alarm::StreamReconnecting {
                item,
                reason: Some(r),
            } if item.0 == 1 => Some(r.clone()),
            _ => None,
        });
        if let Some(r) = found {
            break r;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no alarm with a reason"
        );
    };
    assert!(reason.starts_with("TLS: "), "{reason}");
}

/// The title that `GET /now` of the listener server gives.
fn now_title(port: u16) -> String {
    use std::io::{Read, Write};
    let mut conn = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    conn.write_all(b"GET /now HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut reply = String::new();
    conn.read_to_string(&mut reply).unwrap();
    reply
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_owned()
}

/// An item that hides its own title shows the station name from Preferences, and the
/// configured name when that setting is blank.
#[test]
fn a_hidden_title_shows_the_station_name_from_the_settings() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut first = file_item(1, "First", &wav("n-first", 2.0, Side::Left), 2.0, 2.0, 0.0);
    first.meta.publish = radio_model::PublishMode::Hide;
    let mut second = file_item(
        2,
        "Second",
        &wav("n-second", 2.0, Side::Left),
        2.0,
        2.0,
        0.0,
    );
    second.meta.publish = radio_model::PublishMode::Hide;
    let (p, clock) = offline(engine_with(vec![first, second]), |cfg| {
        let mut w = WebRtcConfig::new(([127, 0, 0, 1], port).into());
        w.stun.clear();
        w.trackers.clear();
        cfg.webrtc = Some(w);
        cfg.webrtc_always = true;
        cfg.station_name = "Configured Radio".into();
    });
    let set_name = |name: &str| {
        p.dispatch(Command::SettingsSet {
            part: radio_model::BRANDING_PART.into(),
            key: radio_model::BRANDING_NAME_KEY.into(),
            value: name.into(),
        })
        .unwrap();
    };
    set_name("Radio Lipa");
    run(&p, &clock, 0.5);
    assert_eq!(now_title(port), "Radio Lipa");

    set_name("  ");
    run(&p, &clock, 2.5);
    assert_eq!(now_title(port), "Configured Radio");
}

/// What the Icecast test server saw.
#[derive(Debug, PartialEq)]
enum Seen {
    Connected,
    Left,
}

/// An Icecast server that takes each source, says 200 and reads it until it leaves.
fn icecast_server() -> (radio_stream::IcecastConfig, std::sync::mpsc::Receiver<Seen>) {
    use std::io::{Read, Write};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                if conn.read_exact(&mut byte).is_err() {
                    break;
                }
                header.push(byte[0]);
            }
            let _ = conn.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
            if tx.send(Seen::Connected).is_err() {
                return;
            }
            let mut bytes = [0; 4096];
            while matches!(conn.read(&mut bytes), Ok(n) if n > 0) {}
            if tx.send(Seen::Left).is_err() {
                return;
            }
        }
    });
    let cfg = radio_stream::IcecastConfig {
        host: "127.0.0.1".into(),
        port,
        mount: "/live".into(),
        user: "source".into(),
        password: "test-only".into(),
        name: "Test Radio".into(),
        description: "Test source".into(),
        genre: "Test".into(),
        format: radio_stream::StreamFormat::Opus { kbps: 96 },
        tls: false,
    };
    (cfg, rx)
}

fn set_on_air(p: &radio_playout::Playout, on: bool) {
    p.dispatch(Command::SettingsSet {
        part: radio_model::AIR_PART.into(),
        key: radio_model::ON_AIR_KEY.into(),
        value: on.into(),
    })
    .unwrap();
}

/// Ticks playout until its view passes `check`, up to a bound.
fn until_view(
    p: &radio_playout::Playout,
    clock: &radio_playout::ManualClock,
    what: &str,
    check: impl Fn(&radio_playout::PlayoutView) -> bool,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        run(p, clock, 0.05);
        let view = p.snapshot();
        if check(&view) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never {what}: {:?}",
            view.outputs
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

const PATIENCE: Duration = Duration::from_secs(5);

/// Off air in the settings closes the Icecast source and the view says so; on air again
/// connects it again.
#[test]
fn off_air_setting_takes_icecast_off_air() {
    let (ice, seen) = icecast_server();
    let (p, clock) = offline(engine_with(Vec::new()), |cfg| cfg.icecast = Some(ice));
    until_view(&p, &clock, "connected", |v| {
        matches!(v.outputs.icecast, Some(StreamStatus::Connected { .. }))
    });
    assert_eq!(seen.recv_timeout(PATIENCE).unwrap(), Seen::Connected);
    assert!(p.snapshot().outputs.on_air);

    set_on_air(&p, false);
    until_view(&p, &clock, "off air", |v| {
        v.outputs.icecast == Some(StreamStatus::OffAir)
    });
    assert!(!p.snapshot().outputs.on_air);
    assert_eq!(seen.recv_timeout(PATIENCE).unwrap(), Seen::Left);

    set_on_air(&p, true);
    until_view(&p, &clock, "back on air", |v| {
        v.outputs.on_air && v.outputs.icecast != Some(StreamStatus::OffAir)
    });
    assert_eq!(seen.recv_timeout(PATIENCE).unwrap(), Seen::Connected);
}

/// A station saved off air never dials Icecast at start.
#[test]
fn a_station_saved_off_air_starts_off_air() {
    let (ice, seen) = icecast_server();
    let mut engine = engine_with(Vec::new());
    engine
        .dispatch(Command::SettingsSet {
            part: radio_model::AIR_PART.into(),
            key: radio_model::ON_AIR_KEY.into(),
            value: false.into(),
        })
        .unwrap();
    let (p, clock) = offline(engine, |cfg| cfg.icecast = Some(ice));
    run(&p, &clock, 1.0);
    let view = p.snapshot();
    assert_eq!(view.outputs.icecast, Some(StreamStatus::OffAir));
    assert!(!view.outputs.on_air);
    assert!(
        seen.recv_timeout(Duration::from_secs(1)).is_err(),
        "a source reached the server"
    );
}

/// Off air stops only what the station sends: a deck that plays goes on playing.
#[test]
fn the_decks_keep_playing_off_air() {
    let (ice, _seen) = icecast_server();
    let song = file_item(1, "Song", &wav("air-song", 4.0, Side::Left), 4.0, 4.0, 0.0);
    let (p, clock) = offline(engine_with(vec![song]), |cfg| cfg.icecast = Some(ice));
    let playing = |p: &radio_playout::Playout| {
        p.snapshot()
            .decks
            .iter()
            .any(|d| d.item.is_some_and(|i| i.0 == 1) && d.phase == DeckPhase::Playing)
    };
    run(&p, &clock, 0.5);
    assert!(playing(&p), "the song did not start");

    set_on_air(&p, false);
    let program = run(&p, &clock, 1.0);
    assert!(!p.snapshot().outputs.on_air);
    assert!(playing(&p), "the song stopped off air");
    assert!(
        first_audible(&program, 0, program.len() / 4).is_some(),
        "Program went silent off air"
    );
}
