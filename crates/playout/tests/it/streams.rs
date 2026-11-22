use crate::fixtures::*;
use radio_engine::Command;
use radio_model::Source;
use radio_playout::{Alarm, DeckPhase};
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
