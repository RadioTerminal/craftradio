//! The station's re-encoded outputs on its server, set from the app: Save sends the whole
//! list to a real station server on this computer, and the settings follow its answer.

use crate::fixtures::*;
use crate::pairing::{
    DAWN_KEY, Rotating, answering, document_for, get, keep_key, keep_snapshotting, pair,
    paired_with, server_with, setting, station,
};
use radio_engine::Command;
use radio_playout::Clock;
use radio_ui_egui::view::{
    Edit, OUTPUTS_READ_ONLY, OutputRow, OutputsState, PairState, StreamFormat, ViewSource,
};
use radio_ui_engine::EngineSource;
use std::time::Duration;

fn save(source: &mut EngineSource, rows: &[OutputRow]) -> Result<usize, String> {
    source.edit(Edit::SaveServerOutputs(rows.to_vec()))
}

fn low_opus() -> Vec<OutputRow> {
    vec![OutputRow {
        mount: "low.opus".into(),
        format: StreamFormat::Opus,
        kbps: 32,
    }]
}

/// Save on a paired station: the page says `Saving…`, then `Saved`, and the settings, the
/// page and the server all list the new outputs.
#[test]
fn saving_outputs_updates_the_settings_and_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, address) = server_with(Some(dir.path()));
    let (mut source, clock, _store) = station();
    assert!(
        save(&mut source, &low_opus()).is_err(),
        "an unpaired station has no server to save to"
    );
    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired
    });
    assert!(
        view.pairing.outputs_editable,
        "the server keeps a state_dir"
    );
    assert!(view.pairing.outputs.is_empty());
    assert_eq!(view.pairing.outputs_state, OutputsState::Idle);

    assert_eq!(save(&mut source, &low_opus()), Ok(1));
    let view = source.snapshot(clock.now());
    assert_eq!(view.pairing.outputs_state, OutputsState::Saving);
    assert_eq!(view.pairing.outputs_state.label(), "Saving…");
    assert_eq!(view.pairing.outputs, low_opus(), "the rows sent stay shown");
    assert!(
        save(&mut source, &low_opus()).is_err(),
        "one save at a time"
    );

    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.outputs_state != OutputsState::Saving
    });
    assert_eq!(view.pairing.outputs_state, OutputsState::Saved);
    assert_eq!(view.pairing.outputs_state.label(), "Saved");
    assert_eq!(view.pairing.outputs, low_opus());
    let outputs = setting(&view, "server", "outputs").unwrap();
    assert_eq!(outputs[0]["mount"], "low.opus");
    assert_eq!(outputs[0]["format"], "opus");
    assert_eq!(outputs[0]["kbps"], 32);
    assert!(
        outputs[0]["url"]
            .as_str()
            .unwrap()
            .ends_with("/dawn/low.opus"),
        "{outputs}"
    );
    let (_, document) = get(&address, "/api/station", Some(DAWN_KEY));
    assert!(
        document.contains("low.opus"),
        "the server lists it: {document}"
    );
    assert!(dir.path().join("dawn.outputs.json").is_file());
}

/// The server refuses a bitrate its encoder can't make: the page says why, keeps the rows
/// that were typed, and the settings and the server are as they were.
#[test]
fn a_refused_save_keeps_the_rows_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, address) = server_with(Some(dir.path()));
    let (mut source, clock, _store) = station();
    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    let before = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired
    });
    let typed = vec![OutputRow {
        mount: "high".into(),
        format: StreamFormat::Opus,
        kbps: 9999,
    }];
    assert_eq!(save(&mut source, &typed), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        matches!(v.pairing.outputs_state, OutputsState::Failed(_))
    });
    let OutputsState::Failed(reason) = &view.pairing.outputs_state else {
        panic!("not failed: {:?}", view.pairing.outputs_state);
    };
    assert!(reason.contains("9999"), "the server's reason: {reason}");
    assert_eq!(view.pairing.outputs_state.label(), reason);
    assert_eq!(view.pairing.outputs, typed, "nothing typed is lost");
    assert_eq!(
        setting(&view, "server", "outputs"),
        setting(&before, "server", "outputs"),
        "the settings are unchanged"
    );
    assert_eq!(view.pairing.state, PairState::Paired);
    let (_, document) = get(&address, "/api/station", Some(DAWN_KEY));
    assert!(!document.contains("\"high\""), "{document}");
}

/// A server older than `outputs_editable` sends no such field: the list reads as not
/// editable, and Save is refused with the read-only hint before anything is sent.
#[test]
fn an_old_server_document_reads_as_not_editable() {
    let old = answering(
        "200 OK",
        r#"{"version": 1, "slug": "dawn", "name": "Dawn Chorus",
            "public_url": "http://127.0.0.1:9",
            "icecast": {"host": "127.0.0.1", "port": 9, "tls": false,
                        "mount": "/dawn/stream", "user": "source"},
            "stream_url": "http://127.0.0.1:9/dawn/stream",
            "outputs": [{"mount": "low", "format": "mp3", "kbps": 64,
                         "url": "http://127.0.0.1:9/dawn/low"}]}"#,
    );
    let (mut source, clock, _store) = station();
    assert_eq!(pair(&mut source, &old, DAWN_KEY), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired
    });
    assert!(!view.pairing.outputs_editable);
    assert_eq!(
        view.pairing.outputs,
        vec![OutputRow {
            mount: "low".into(),
            format: StreamFormat::Mp3,
            kbps: 64,
        }],
        "the list is still shown, read-only"
    );
    let refused = save(&mut source, &low_opus()).unwrap_err();
    assert_eq!(refused, OUTPUTS_READ_ONLY);
    assert_eq!(
        source.snapshot(clock.now()).pairing.outputs_state,
        OutputsState::Idle
    );
}

/// The key goes only to the address it was paired through: another client rewrites
/// `server.address`, and a Save then sends that host nothing and says why.
#[test]
fn the_key_goes_only_to_the_paired_address() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, address) = server_with(Some(dir.path()));
    let other = Rotating::start(DAWN_KEY);
    let (mut source, clock, _store) = station();
    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Paired
    });
    source
        .playout()
        .dispatch(Command::SettingsSet {
            part: "server".into(),
            key: "address".into(),
            value: other.address.clone().into(),
        })
        .unwrap();
    let _ = wait_for(&mut source, &clock, |v| {
        matches!(v.pairing.state, PairState::Refused(_))
    });

    assert_eq!(save(&mut source, &low_opus()), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        matches!(v.pairing.outputs_state, OutputsState::Failed(_))
    });
    let OutputsState::Failed(reason) = &view.pairing.outputs_state else {
        panic!("not failed: {:?}", view.pairing.outputs_state);
    };
    assert!(reason.contains("enter the key again"), "{reason}");
    keep_snapshotting(&mut source, &clock, Duration::from_secs(1));
    assert_eq!(other.connections(), 0, "the other host saw nothing");
    let (_, document) = get(&address, "/api/station", Some(DAWN_KEY));
    assert!(!document.contains("low.opus"), "{document}");
}

/// Save is not offered while the server is being asked, nor while it refused the key: each
/// would send the key once more toward the server's lockout.
#[test]
fn save_is_refused_mid_pairing_and_after_a_refused_key() {
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", silent.local_addr().unwrap());
    let (mut source, clock, store) = station();
    paired_with(&mut source, &address, document_for(&address));
    keep_key(&store, &address, DAWN_KEY);
    let view = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::Pairing
    });
    assert_eq!(view.pairing.state, PairState::Pairing, "asking at start");
    assert!(
        save(&mut source, &low_opus()).is_err(),
        "not offered mid-pairing"
    );
    assert_eq!(
        source.snapshot(clock.now()).pairing.outputs_state,
        OutputsState::Idle
    );
    drop(silent);

    let rotated = Rotating::start("invented-rotated-key-5Hn1");
    let (mut source, clock, store) = station();
    paired_with(
        &mut source,
        &rotated.address,
        document_for(&rotated.address),
    );
    keep_key(&store, &rotated.address, DAWN_KEY);
    let _ = wait_for(&mut source, &clock, |v| {
        v.pairing.state == PairState::KeyRefused
    });
    let refused = rotated.refused();
    let reason = save(&mut source, &low_opus()).unwrap_err();
    assert!(reason.contains("refused the station key"), "{reason}");
    keep_snapshotting(&mut source, &clock, Duration::from_secs(1));
    assert_eq!(
        rotated.refused(),
        refused,
        "the refused key was not sent again"
    );
}
