use crate::fixtures::*;
use radio_engine::Command;
use radio_model::Source;
use radio_playout::{Alarm, DeckPhase};
use radio_stream::WebRtcConfig;
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
