//! `PUT /api/station/outputs`: a station's key changes its re-encoded outputs, saved in the
//! server's `state_dir` and applied while the station is on the air.

use crate::server::{Drain, EAR_PATIENCE, MORNING_KEY, Pusher, Tone, on_air, push_when_free};
use crate::support::{
    Head, PATIENCE, client, entry, hears, read_head, read_to_end_within, request, stats,
};
use base64::Engine;
use radio_server::{Config, Format, Server, ServerHandle, hash_key};
use serde_json::{Value, json};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SLUG: &str = "morning";
const NO_STATE_DIR: &str = "this server keeps no state_dir: outputs are set in its config";

/// One station, "morning", with `outputs` (TOML `[[station.output]]` tables) and, when
/// given, a `state_dir`.
fn config(state_dir: Option<&Path>, outputs: &str) -> Config {
    let state_dir = state_dir
        .map(|dir| format!("state_dir = {:?}\n", dir.display().to_string()))
        .unwrap_or_default();
    Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n{state_dir}\n\
         [[station]]\nslug = \"{SLUG}\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\n\n\
         {outputs}",
        hash_key(MORNING_KEY)
    ))
    .unwrap()
}

const OPUS_64: &str = "[[station.output]]\nmount = \"opus\"\nformat = \"opus\"\nkbps = 64\n";

fn saved_file(dir: &Path) -> PathBuf {
    dir.join(format!("{SLUG}.outputs.json"))
}

fn authorization(key: Option<&str>) -> String {
    key.map(|key| {
        let pair = base64::engine::general_purpose::STANDARD.encode(format!("source:{key}"));
        format!("Authorization: Basic {pair}\r\n")
    })
    .unwrap_or_default()
}

/// `PUT /api/station/outputs` with `body`, its `Content-Length` (or none), and `key` as
/// the Basic password (or no `Authorization`); the reply head and body.
fn put_raw(
    addr: SocketAddr,
    key: Option<&str>,
    body: &[u8],
    content_length: Option<usize>,
) -> (Head, Vec<u8>) {
    let mut stream = client(addr);
    let length = content_length
        .map(|length| format!("Content-Length: {length}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "PUT /api/station/outputs HTTP/1.1\r\nHost: radio.example.test\r\n{}\
         Content-Type: application/json\r\n{length}\r\n",
        authorization(key)
    )
    .unwrap();
    // A server that answers before the whole body is sent may close early.
    let _ = stream.write_all(body);
    let head = read_head(&mut stream);
    let body = read_to_end_within(&mut stream, PATIENCE);
    (head, body)
}

fn put(addr: SocketAddr, outputs: &Value) -> (Head, Vec<u8>) {
    let body = serde_json::to_vec(outputs).unwrap();
    put_raw(addr, Some(MORNING_KEY), &body, Some(body.len()))
}

/// A PUT that the server takes; the pairing document it answers with.
fn put_ok(addr: SocketAddr, outputs: &Value) -> Value {
    let (head, body) = put(addr, outputs);
    assert_eq!(
        head.status,
        200,
        "the PUT was refused: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(head.header("Content-Type"), Some("application/json"));
    serde_json::from_slice(&body).expect("the reply is the pairing document")
}

/// `GET /api/station` with the station's key.
fn document(addr: SocketAddr) -> Value {
    let mut stream = client(addr);
    write!(
        stream,
        "GET /api/station HTTP/1.1\r\nHost: radio.example.test\r\n{}\r\n",
        authorization(Some(MORNING_KEY))
    )
    .unwrap();
    let head = read_head(&mut stream);
    assert_eq!(head.status, 200);
    serde_json::from_slice(&read_to_end_within(&mut stream, PATIENCE)).unwrap()
}

fn mounts(document: &Value) -> Vec<String> {
    document["outputs"]
        .as_array()
        .expect("outputs is an array")
        .iter()
        .map(|output| output["mount"].as_str().unwrap().to_owned())
        .collect()
}

/// Joins `/<slug>/<mount>`, asking again while the output answers that it is starting.
fn listen_output(addr: SocketAddr, mount: &str) -> TcpStream {
    let deadline = Instant::now() + EAR_PATIENCE;
    loop {
        let (mut stream, head) = request(addr, "GET", &format!("/{SLUG}/{mount}"));
        if head.status == 200 {
            return stream;
        }
        let body = read_to_end_within(&mut stream, PATIENCE);
        let body = String::from_utf8_lossy(&body);
        assert_eq!(head.status, 503, "/{SLUG}/{mount}: {body}");
        assert!(body.contains("starting"), "/{SLUG}/{mount} refused: {body}");
        assert!(Instant::now() < deadline, "/{SLUG}/{mount} never started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn put_outputs_adds_an_output_live() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), "")).unwrap();
    let addr = server.local_addr();
    let pusher = on_air(addr, SLUG, MORNING_KEY, Tone::Ogg);
    let (pushed, head) = request(addr, "GET", &format!("/{SLUG}/stream"));
    assert_eq!(head.status, 200);
    let pushed = Drain::start(pushed);

    let document = put_ok(
        addr,
        &json!([{"mount": "low.opus", "format": "opus", "kbps": 32}]),
    );
    assert_eq!(mounts(&document), ["low.opus"]);
    assert_eq!(document["outputs_editable"], json!(true));
    assert!(
        document["outputs"][0]["url"]
            .as_str()
            .unwrap()
            .ends_with("/morning/low.opus")
    );

    let low = listen_output(addr, "low.opus");
    assert!(
        hears(low, Format::Ogg, EAR_PATIENCE),
        "the new output plays the source"
    );
    // The push went on through the change: the same connection still sends, and the
    // pushed stream's listener was never cut off.
    let before = pushed.bytes();
    pusher.wait_packets(10);
    eventually("the pushed stream stopped flowing", || {
        pushed.bytes() > before
    });
    assert!(!pushed.ended(), "the /stream listener was cut off");
    assert_eq!(
        entry(&stats(addr), SLUG, "stream").map(|e| e["listeners"].clone()),
        Some(json!(1))
    );

    // The source's end finishes the transcoder the change started, with no change after.
    drop(pusher);
    eventually("the source never ended", || {
        entry(&stats(addr), SLUG, "stream").is_none()
    });
    assert!(
        !server.transcoding(SLUG),
        "a transcoder was left running after the source ended"
    );
    let _again = on_air(addr, SLUG, MORNING_KEY, Tone::Ogg);
    assert!(
        hears(listen_output(addr, "low.opus"), Format::Ogg, EAR_PATIENCE),
        "the next source plays on the new output"
    );
}

#[test]
fn put_outputs_removes_an_output() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    let addr = server.local_addr();
    let _pusher = on_air(addr, SLUG, MORNING_KEY, Tone::Ogg);
    let (pushed, head) = request(addr, "GET", &format!("/{SLUG}/stream"));
    assert_eq!(head.status, 200);
    let pushed = Drain::start(pushed);
    let listener = Drain::start(listen_output(addr, "opus"));
    eventually("the output's listener got no audio", || {
        listener.bytes() > 0
    });
    assert!(entry(&stats(addr), SLUG, "opus").is_some());

    let document = put_ok(addr, &json!([]));
    assert!(mounts(&document).is_empty());
    eventually("the removed output's listener was not cut off", || {
        listener.ended()
    });
    let (_, head) = request(addr, "GET", &format!("/{SLUG}/opus"));
    assert_eq!(head.status, 404);
    let listed = stats(addr);
    assert!(entry(&listed, SLUG, "opus").is_none(), "{listed}");
    assert!(entry(&listed, SLUG, "stream").is_some(), "{listed}");
    assert!(!pushed.ended(), "the /stream listener was cut off");
}

#[test]
fn put_outputs_is_saved_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    put_ok(
        server.local_addr(),
        &json!([{"mount": "mp3", "format": "mp3", "kbps": 128}]),
    );
    server.stop();
    drop(server);

    let saved: Value =
        serde_json::from_slice(&std::fs::read(saved_file(dir.path())).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!([{"mount": "mp3", "format": "mp3", "kbps": 128}])
    );
    // No temporary file is left beside it.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");

    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    let document = document(server.local_addr());
    assert_eq!(
        mounts(&document),
        ["mp3"],
        "the saved outputs win over the config's"
    );
    assert_eq!(document["outputs"][0]["kbps"], json!(128));

    // An empty saved list is no outputs, not the config's.
    put_ok(server.local_addr(), &json!([]));
    drop(server);
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    assert!(mounts(&document_of(&server)).is_empty());
}

fn document_of(server: &ServerHandle) -> Value {
    document(server.local_addr())
}

#[test]
fn a_bad_saved_file_stops_the_server_naming_it() {
    for bad in [
        "not json".to_owned(),
        json!([{"mount": "low", "format": "opus", "kbps": 9999}]).to_string(),
        json!([{"mount": "stream", "format": "opus", "kbps": 64}]).to_string(),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(saved_file(dir.path()), &bad).unwrap();
        let error = Server::start(config(Some(dir.path()), OPUS_64))
            .err()
            .unwrap_or_else(|| panic!("{bad:?} was taken"));
        assert!(
            error.to_string().contains("morning.outputs.json"),
            "{bad:?}: {error}"
        );
    }
}

#[test]
fn refused_puts_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), "")).unwrap();
    let addr = server.local_addr();
    put_ok(
        addr,
        &json!([{"mount": "low", "format": "opus", "kbps": 64}]),
    );
    let file = std::fs::read(saved_file(dir.path())).unwrap();
    let live = document(addr);

    let nine: Vec<Value> = (0..9)
        .map(|n| json!({"mount": format!("low{n}"), "format": "opus", "kbps": 64}))
        .collect();
    let bodies: Vec<(&str, Vec<u8>)> = vec![
        (
            "a bitrate the encoder can't make",
            json!([{"mount": "high", "format": "opus", "kbps": 9999}])
                .to_string()
                .into_bytes(),
        ),
        (
            "two outputs on one mount",
            json!([
                {"mount": "twin", "format": "opus", "kbps": 64},
                {"mount": "twin", "format": "mp3", "kbps": 128}
            ])
            .to_string()
            .into_bytes(),
        ),
        (
            "the server's own mount",
            json!([{"mount": "stream", "format": "opus", "kbps": 64}])
                .to_string()
                .into_bytes(),
        ),
        ("nine outputs", Value::Array(nine).to_string().into_bytes()),
        ("JSON that does not parse", b"[{\"mount\":".to_vec()),
        (
            "a format there is none of",
            br#"[{"mount":"x","format":"flac","kbps":64}]"#.to_vec(),
        ),
    ];
    for (what, body) in bodies {
        let (head, reply) = put_raw(addr, Some(MORNING_KEY), &body, Some(body.len()));
        let reason = String::from_utf8_lossy(&reply);
        assert_eq!(head.status, 400, "{what}: {reason}");
        assert!(!reason.trim().is_empty(), "{what}: no reason given");
        assert_eq!(
            std::fs::read(saved_file(dir.path())).unwrap(),
            file,
            "{what}: the saved file changed"
        );
        assert_eq!(document(addr), live, "{what}: the live outputs changed");
    }
}

#[test]
fn put_without_state_dir_is_403() {
    let server = Server::start(config(None, OPUS_64)).unwrap();
    let addr = server.local_addr();
    assert_eq!(document(addr)["outputs_editable"], json!(false));
    let (head, body) = put(addr, &json!([]));
    assert_eq!(head.status, 403);
    assert_eq!(String::from_utf8_lossy(&body), NO_STATE_DIR);
    assert_eq!(mounts(&document(addr)), ["opus"]);
}

#[test]
fn put_with_a_wrong_key_is_401_and_counts_toward_the_lockout() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    let addr = server.local_addr();
    let body = b"[]";
    let mut refused = 0;
    loop {
        let (head, _) = put_raw(addr, Some("invented-wrong-key"), body, Some(body.len()));
        if head.status == 429 {
            break;
        }
        assert_eq!(head.status, 401);
        refused += 1;
        assert!(refused <= 10, "wrong keys never locked the address out");
    }
    assert!(refused >= 1);
    assert!(
        !saved_file(dir.path()).exists(),
        "a refused PUT saved outputs"
    );
    // Locked out: the right key waits too, on a PUT and on the pairing document.
    assert_eq!(put(addr, &json!([])).0.status, 429);
    let mut stream = client(addr);
    write!(
        stream,
        "GET /api/station HTTP/1.1\r\nHost: radio.example.test\r\n{}\r\n",
        authorization(Some(MORNING_KEY))
    )
    .unwrap();
    assert_eq!(read_head(&mut stream).status, 429);
}

#[test]
fn put_without_a_key_is_401() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    let addr = server.local_addr();
    let body = b"[]";
    for _ in 0..8 {
        let (head, _) = put_raw(addr, None, body, Some(body.len()));
        assert_eq!(head.status, 401);
    }
    assert!(!saved_file(dir.path()).exists());
    assert_eq!(mounts(&document(addr)), ["opus"]);
}

#[test]
fn a_body_over_16_kib_is_413() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    let addr = server.local_addr();
    // A list of valid JSON, padded with spaces past 16 KiB.
    let mut body = b"[".to_vec();
    body.resize(16 * 1024 + 1, b' ');
    *body.last_mut().unwrap() = b']';
    let (head, _) = put_raw(addr, Some(MORNING_KEY), &body, Some(body.len()));
    assert_eq!(head.status, 413);
    // One at the limit is taken.
    body.truncate(16 * 1024);
    *body.last_mut().unwrap() = b']';
    let (head, _) = put_raw(addr, Some(MORNING_KEY), &body, Some(body.len()));
    assert_eq!(head.status, 200);
}

#[test]
fn no_content_length_is_411() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    let addr = server.local_addr();
    let (head, _) = put_raw(addr, Some(MORNING_KEY), b"", None);
    assert_eq!(head.status, 411);
    assert!(!saved_file(dir.path()).exists());
}

#[test]
fn a_swap_while_the_source_ends_leaves_no_transcoder() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(config(Some(dir.path()), OPUS_64)).unwrap();
    let addr = server.local_addr();
    let sets = [
        json!([{"mount": "opus", "format": "opus", "kbps": 64}]),
        json!([
            {"mount": "opus", "format": "opus", "kbps": 48},
            {"mount": "mp3", "format": "mp3", "kbps": 128}
        ]),
        json!([]),
    ];
    let mut seen = vec![server.outputs(SLUG).unwrap()];
    std::thread::scope(|scope| {
        let source = scope.spawn(|| {
            for round in 0..12u64 {
                let pusher = Pusher::start(
                    push_when_free(addr, SLUG, MORNING_KEY, "audio/ogg"),
                    Tone::Ogg,
                );
                std::thread::sleep(Duration::from_millis(15 * (round % 4)));
                drop(pusher);
                // Changes go on while the server notices the source has gone. (A change
                // made off the air also clears the slot, so this check alone can't catch
                // a source's end that leaves a transcoder; the first test does.)
                eventually("the source never ended", || {
                    entry(&stats(addr), SLUG, "stream").is_none()
                });
                assert!(
                    !server.transcoding(SLUG),
                    "a transcoder was left running after the source ended"
                );
            }
        });
        let mut round = 0;
        while !source.is_finished() {
            put_ok(addr, &sets[round % sets.len()]);
            seen.push(server.outputs(SLUG).unwrap());
            round += 1;
        }
    });

    eventually("the last source never ended", || {
        entry(&stats(addr), SLUG, "stream").is_none()
    });
    assert!(
        !server.transcoding(SLUG),
        "a transcoder was left running after the source ended"
    );

    // A new source gets outputs that work, and only one transcoder makes them.
    put_ok(addr, &sets[0]);
    seen.push(server.outputs(SLUG).unwrap());
    let _pusher = on_air(addr, SLUG, MORNING_KEY, Tone::Ogg);
    let listener = listen_output(addr, "opus");
    assert!(hears(listener, Format::Ogg, EAR_PATIENCE));
    // With no one listening, nothing decodes, in any set of outputs there ever was.
    eventually("the output kept a listener", || {
        entry(&stats(addr), SLUG, "opus").is_some_and(|e| e["listeners"] == json!(0))
    });
    std::thread::sleep(Duration::from_millis(200));
    let before: Vec<u64> = seen
        .iter()
        .map(|outputs| outputs.decoded_blocks())
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    let after: Vec<u64> = seen
        .iter()
        .map(|outputs| outputs.decoded_blocks())
        .collect();
    assert_eq!(
        before, after,
        "something still decodes with no one listening"
    );
}
