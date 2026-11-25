//! The cover of what plays goes to the paired station server, which shows it on its
//! listener page: with a real server on this computer.

use crate::fixtures::*;
use crate::icecast::Shared;
use crate::pairing::{DAWN_KEY, keep_key, pair, server_with};
use radio_playout::{Clock, ManualClock};
use radio_ui_egui::view::{DeckState, Edit, PairState, StationView, Status, ViewSource};
use radio_ui_engine::{EngineSource, MemoryStore, SecretStore, server_entry};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A valid 1×1 PNG, the item's own picture.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A station of two long songs: the first with a picture of its own, the second with
/// none (its WAV has no tags), and a memory keychain the test fills.
fn two_songs() -> (EngineSource, ManualClock, Arc<MemoryStore>) {
    let d = dir();
    let picture = d.join("cover.png");
    std::fs::write(&picture, PNG).unwrap();
    let mut with_cover = track(1, "Invented Dawn", &wav(&d, "one", 60.0), 60.0);
    with_cover.meta.picture = picture.to_string_lossy().into_owned();
    let plain = track(2, "Invented Noon", &wav(&d, "two", 60.0), 60.0);
    let store = Arc::new(MemoryStore::default());
    let (source, clock) = source(vec![with_cover, plain]);
    let source = source.with_secrets(Arc::new(Shared(store.clone())));
    (source, clock, store)
}

/// `GET <address>/dawn/cover`: the status and the `Content-Type`.
fn cover(address: &str) -> (u16, Option<String>) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(5)))
            .build(),
    );
    let response = agent.get(format!("{address}/dawn/cover")).call().unwrap();
    let kind = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (response.status().as_u16(), kind)
}

/// Takes snapshots, the clock following real time, until `done` holds, for at most twenty
/// seconds.
fn until(
    source: &mut EngineSource,
    clock: &ManualClock,
    mut done: impl FnMut(&StationView) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = Instant::now();
    loop {
        clock.advance(last.elapsed());
        last = Instant::now();
        let view = source.snapshot(clock.now());
        if done(&view) {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn playing(view: &StationView) -> Option<u64> {
    view.decks
        .iter()
        .find(|d| d.state == DeckState::Playing)
        .and_then(|d| d.item)
}

/// Paired, the cover of the item on air reaches the server; the next item, which has none,
/// clears it, with a body of no bytes that the server takes.
#[test]
fn the_item_on_air_sends_its_cover_and_the_next_clears_it() {
    let (_server, address) = server_with(None);
    let (mut source, clock, _store) = two_songs();
    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    assert!(
        until(&mut source, &clock, |v| {
            v.pairing.state == PairState::Paired
                && v.output.icecast == Status::Connected
                && playing(v) == Some(1)
        }),
        "paired, pushing and playing the first song"
    );
    assert!(
        until(&mut source, &clock, |_| cover(&address).0 == 200),
        "the server never got the cover"
    );
    let (_, kind) = cover(&address);
    assert!(
        kind.as_deref().is_some_and(|k| k.starts_with("image/")),
        "an image: {kind:?}"
    );

    assert!(source.edit(Edit::PlayNext { fade_in_s: 0.0 }).is_ok());
    assert!(
        until(&mut source, &clock, |v| playing(v) == Some(2)),
        "the second song never came on air"
    );
    assert!(
        until(&mut source, &clock, |_| cover(&address).0 == 404),
        "the cover of the first song stayed after it"
    );
}

/// A push set up by hand, with no pairing, sends no cover, though the keychain holds the
/// key for that very server.
#[test]
fn no_cover_goes_without_a_pairing() {
    let (_server, address) = server_with(None);
    let (mut source, clock, store) = two_songs();
    let host_port = address.trim_start_matches("http://");
    let (host, port) = host_port.split_once(':').unwrap();
    store
        .set(&format!("icecast:source@{host_port}/dawn/stream"), DAWN_KEY)
        .unwrap();
    store.set(&server_entry(&address), DAWN_KEY).unwrap();
    for (key, value) in [
        ("host", serde_json::json!(host)),
        ("port", serde_json::json!(port.parse::<u16>().unwrap())),
        ("mount", serde_json::json!("/dawn/stream")),
        ("tls", serde_json::json!(false)),
    ] {
        source
            .playout()
            .dispatch(radio_engine::Command::SettingsSet {
                part: radio_ui_engine::ICECAST_PART.into(),
                key: key.into(),
                value,
            })
            .unwrap();
    }
    assert!(
        until(&mut source, &clock, |v| {
            v.output.icecast == Status::Connected && playing(v) == Some(1)
        }),
        "pushing and playing the song with a cover"
    );
    let watched = Instant::now() + Duration::from_secs(3);
    let _ = until(&mut source, &clock, |_| {
        cover(&address).0 != 404 || Instant::now() > watched
    });
    assert_eq!(cover(&address).0, 404, "a cover went without a pairing");
}

/// The paired address rewritten by hand to another server: the key, stored for the address
/// it was paired through, goes nowhere else, so that server gets no cover.
#[test]
fn the_key_goes_only_to_the_address_it_was_paired_through() {
    let (_first, address) = server_with(None);
    let (_other, elsewhere) = server_with(None);
    let (mut source, clock, store) = two_songs();
    assert_eq!(pair(&mut source, &address, DAWN_KEY), Ok(1));
    assert!(
        until(&mut source, &clock, |v| {
            v.pairing.state == PairState::Paired && v.output.icecast == Status::Connected
        }),
        "paired and pushing"
    );
    assert!(
        until(&mut source, &clock, |_| cover(&address).0 == 200),
        "the paired server got the cover"
    );
    // Only the paired address's own entry holds the key, as a pairing leaves it.
    keep_key(&store, &address, DAWN_KEY);
    source
        .playout()
        .dispatch(radio_engine::Command::SettingsSet {
            part: radio_model::SERVER_PART.into(),
            key: radio_model::SERVER_ADDRESS_KEY.into(),
            value: serde_json::json!(elsewhere),
        })
        .unwrap();
    let _ = source.edit(Edit::PlayNext { fade_in_s: 0.0 });
    let watched = Instant::now() + Duration::from_secs(4);
    let _ = until(&mut source, &clock, |_| Instant::now() > watched);
    assert_eq!(
        cover(&elsewhere).0,
        404,
        "the other server got a cover, so it got the key"
    );
}
