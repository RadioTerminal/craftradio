//! The control channel against a real playout: an in-memory station on no sound card, with a
//! clock that stands still.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use radio_control::{APP, Connection, Control, Request, serve};
use radio_engine::Engine;
use radio_playout::{ManualClock, OutputChoice, Playout, PlayoutConfig};
use serde_json::{Value, json};

/// A token long enough for craft-control, the same in every test.
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

fn playout() -> Playout {
    let engine = Engine::open_in_memory().expect("an in-memory station");
    let clock = ManualClock::new("2026-11-21T13:00:00Z".parse().expect("a timestamp"));
    let cfg = PlayoutConfig::new(Arc::new(clock), OutputChoice::Null);
    Playout::start(engine, cfg).expect("playout starts")
}

/// The channel of `playout` on a free loopback port.
fn channel(playout: &Playout) -> Control {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let sender = playout.command_sender().expect("a running playout");
    serve(sender, listener, TOKEN.to_owned()).expect("the channel serves")
}

fn client(control: &Control, token: &str) -> Connection {
    Connection::new(APP, &format!("127.0.0.1:{}", control.port()), Some(token))
}

fn run(conn: &mut Connection, command: Value) -> Result<Value, String> {
    conn.call(Request::new("station.run", command))
        .map_err(|e| e.message().to_owned())
}

fn playlist_names(conn: &mut Connection) -> Vec<String> {
    let outcome = run(conn, json!({"id": "station.get"})).expect("station.get answers");
    outcome["data"]["station"]["playlists"]
        .as_array()
        .or_else(|| outcome["data"]["playlists"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn a_command_over_the_channel_changes_the_running_station() {
    let playout = playout();
    let control = channel(&playout);
    let mut conn = client(&control, TOKEN);
    assert!(!playlist_names(&mut conn).contains(&"Night Owls".to_owned()));
    run(
        &mut conn,
        json!({"id": "playlist.create", "args": {"name": "Night Owls"}}),
    )
    .expect("the station takes the command");
    assert!(
        playlist_names(&mut conn).contains(&"Night Owls".to_owned()),
        "the running station has the new playlist"
    );
}

#[test]
fn a_wrong_token_is_refused_and_changes_nothing() {
    let playout = playout();
    let control = channel(&playout);
    let mut stranger = client(&control, "ffffffffffffffffffffffffffffffffffffffffffffffff");
    assert!(
        run(
            &mut stranger,
            json!({"id": "playlist.create", "args": {"name": "Intruder"}})
        )
        .is_err()
    );
    let mut owner = client(&control, TOKEN);
    assert!(!playlist_names(&mut owner).contains(&"Intruder".to_owned()));
}

#[test]
fn a_bad_command_and_an_unknown_method_are_errors_not_silence() {
    let playout = playout();
    let control = channel(&playout);
    let mut conn = client(&control, TOKEN);
    let bad = run(&mut conn, json!({"id": "deck.levitate"}));
    assert!(bad.is_err_and(|e| e.contains("cannot read the command")));
    let unknown = conn.call(Request::new("station.revision", json!({})));
    assert!(unknown.is_err_and(|e| e.message().contains("station.run")));
}

#[test]
fn the_command_list_names_every_engine_command() {
    let playout = playout();
    let control = channel(&playout);
    let mut conn = client(&control, TOKEN);
    let list = conn
        .call(Request::new("station.commands", json!({})))
        .expect("station.commands answers");
    let ids: Vec<&str> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str())
        .collect();
    for spec in radio_engine::registry() {
        assert!(ids.contains(&spec.id), "{} is listed", spec.id);
    }
}

#[test]
fn closing_the_channel_lets_the_playout_stop() {
    let playout = playout();
    let control = channel(&playout);
    drop(control);
    let (done, stopped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(playout);
        let _ = done.send(());
    });
    assert!(
        stopped.recv_timeout(Duration::from_secs(10)).is_ok(),
        "the playout stops once the channel is closed"
    );
}
