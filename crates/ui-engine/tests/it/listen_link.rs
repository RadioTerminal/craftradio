//! The WebRTC listen link follows the pairing as it is now: on the paired server's listener
//! page while the station is paired, back on the listener site once it is not, with no
//! restart between.

use radio_engine::{Command, Engine};
use radio_model::{PairingDocument, PairingIcecast};

const ADDRESS: &str = "https://radio.example.invalid";
/// An invented station ID, in the shape of one.
const ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// The link the WebRTC output made at its start, before any pairing.
fn started_link() -> String {
    radio_stream::link_for(ID, None)
}

fn document(player_url: &str) -> PairingDocument {
    PairingDocument {
        version: 1,
        slug: "night-owls".into(),
        name: "Night Owls FM".into(),
        public_url: ADDRESS.into(),
        icecast: PairingIcecast {
            host: "radio.example.invalid".into(),
            port: 443,
            tls: true,
            mount: "/night-owls/stream".into(),
            user: "source".into(),
        },
        stream_url: format!("{ADDRESS}/night-owls/stream"),
        outputs: Vec::new(),
        outputs_editable: false,
        player_url: player_url.into(),
    }
}

fn pair(e: &mut Engine, document: PairingDocument) {
    e.dispatch(Command::ServerPair {
        address: ADDRESS.into(),
        document,
    })
    .unwrap();
}

#[test]
fn pairing_moves_the_link_to_the_servers_page_and_unpairing_moves_it_back() {
    let mut e = Engine::open_in_memory().unwrap();
    let unpaired = radio_ui_engine::listen_link(e.state(), &started_link());
    assert!(
        unpaired.starts_with(radio_stream::LISTEN_SITE),
        "unpaired, the listener site: {unpaired}"
    );
    assert!(unpaired.ends_with(&format!("#{ID}")));

    pair(&mut e, document(&format!("{ADDRESS}/night-owls/")));
    let paired = radio_ui_engine::listen_link(e.state(), &started_link());
    assert!(
        paired.starts_with(&format!("{ADDRESS}/night-owls/?stream=")),
        "paired, the server's page: {paired}"
    );
    assert!(
        paired.contains("night-owls%2Fstream"),
        "with the server's stream for when WebRTC does not connect: {paired}"
    );
    assert!(paired.ends_with(&format!("#{ID}")), "the station by its ID");

    e.dispatch(Command::ServerUnpair).unwrap();
    let back = radio_ui_engine::listen_link(e.state(), &started_link());
    assert!(
        back.starts_with(radio_stream::LISTEN_SITE),
        "unpaired again, the listener site: {back}"
    );
    assert!(
        !back.contains("night-owls"),
        "nothing of the server: {back}"
    );
}

#[test]
fn a_server_that_serves_no_page_keeps_the_listener_site() {
    let mut e = Engine::open_in_memory().unwrap();
    // A server older than the page sends no player_url.
    pair(&mut e, document(""));
    let link = radio_ui_engine::listen_link(e.state(), &started_link());
    assert!(
        link.starts_with(radio_stream::LISTEN_SITE),
        "the listener site: {link}"
    );
    assert!(
        link.contains("night-owls%2Fstream"),
        "with the paired stream as the public one: {link}"
    );
}

#[test]
fn a_page_on_another_host_is_never_linked() {
    for elsewhere in [
        "https://elsewhere.example.invalid/night-owls/",
        "http://radio.example.invalid/night-owls/",
        "https://radio.example.invalid.elsewhere.invalid/night-owls/",
        "https://radio.example.invalid/night-owls/?go=elsewhere",
    ] {
        let mut e = Engine::open_in_memory().unwrap();
        pair(&mut e, document(elsewhere));
        let link = radio_ui_engine::listen_link(e.state(), &started_link());
        assert!(
            link.starts_with(radio_stream::LISTEN_SITE),
            "{elsewhere}: the listener site, not {link}"
        );
    }
}

#[test]
fn a_page_setting_rewritten_to_another_host_falls_back_to_the_listener_site() {
    let mut e = Engine::open_in_memory().unwrap();
    pair(&mut e, document(&format!("{ADDRESS}/night-owls/")));
    e.dispatch(Command::SettingsSet {
        part: radio_model::SERVER_PART.into(),
        key: radio_model::SERVER_PLAYER_URL_KEY.into(),
        value: "https://elsewhere.example.invalid/".into(),
    })
    .unwrap();
    let link = radio_ui_engine::listen_link(e.state(), &started_link());
    assert!(
        link.starts_with(radio_stream::LISTEN_SITE),
        "the listener site: {link}"
    );
}
