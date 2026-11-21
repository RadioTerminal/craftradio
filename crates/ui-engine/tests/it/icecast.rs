use crate::fixtures::*;
use radio_ui_egui::view::{Edit, IcecastView, Status, StreamFormat, ViewSource};
use radio_ui_engine::{MemoryStore, SecretStore, icecast_config};

/// Shares one memory store between the source and the test.
struct Shared(std::sync::Arc<MemoryStore>);

impl SecretStore for Shared {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        self.0.get(key)
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.set(key, secret)
    }
}

/// The Icecast password goes to the secret store. The settings and the snapshot never carry
/// it; the snapshot only says that it is set.
#[test]
fn the_icecast_password_stays_out_of_the_settings_and_the_snapshot() {
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store.clone())));
    let secret = "hunter-test-only";
    let mount = IcecastView {
        host: "stream.example.org".into(),
        port: 8000,
        mount: "/live".into(),
        user: "source".into(),
        password: secret.into(),
        password_set: false,
        keychain_error: None,
        tls: true,
        format: StreamFormat::Opus,
        kbps: 96,
    };
    assert_eq!(source.edit(Edit::SetIcecast(mount)), Ok(1));
    // The keychain answers on the background lane.
    let view = wait_for(&mut source, &clock, |v| v.icecast.password_set);
    assert!(
        view.icecast.password.is_empty(),
        "no password in the snapshot"
    );
    assert!(view.icecast.password_set);
    assert_eq!(view.icecast.host, "stream.example.org");
    let settings = serde_json::to_string(&view.settings).unwrap();
    assert!(!settings.contains(secret), "no password in the settings");
    assert_ne!(
        view.output.icecast,
        Status::Connected,
        "saving does not connect"
    );

    let state = source.playout().snapshot().state;
    let cfg = icecast_config(&state, &Shared(store), "Test")
        .unwrap()
        .expect("a mount");
    assert_eq!(cfg.password, secret, "the start reads it from the store");
}

/// A password belongs to its mount. After another client changes the host, the new host has
/// no password, and the start sends none to it.
#[test]
fn a_changed_host_gets_no_password_of_the_old_mount() {
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store.clone())));
    let mut mount = IcecastView {
        host: "stream.example.org".into(),
        port: 8000,
        mount: "/live".into(),
        user: "source".into(),
        password: "only-for-the-old-host".into(),
        password_set: false,
        keychain_error: None,
        tls: true,
        format: StreamFormat::Opus,
        kbps: 96,
    };
    assert_eq!(source.edit(Edit::SetIcecast(mount.clone())), Ok(1));
    let _ = wait_for(&mut source, &clock, |v| v.icecast.password_set);
    // Another client (the CLI or MCP) changes only the host setting.
    let _ = &mut mount;
    let _ = source
        .playout()
        .dispatch(radio_engine::Command::SettingsSet {
            part: radio_ui_engine::ICECAST_PART.into(),
            key: "host".into(),
            value: serde_json::json!("other.example.net"),
        })
        .unwrap();
    let view = wait_for(&mut source, &clock, |v| {
        v.icecast.host == "other.example.net"
    });
    assert!(!view.icecast.password_set, "the new host has no password");
    let state = source.playout().snapshot().state;
    assert!(
        icecast_config(&state, &Shared(store), "Test").is_err(),
        "the start finds no password for the new host"
    );
}

/// TLS is on for another computer and off for this one, unless a person chose. The start
/// uses the choice.
#[test]
fn tls_defaults_on_for_a_remote_host_and_the_start_honours_the_choice() {
    let store = std::sync::Arc::new(MemoryStore::default());
    let (source, clock) = source(Vec::new());
    let mut source = source.with_secrets(std::sync::Arc::new(Shared(store.clone())));
    let set = |source: &mut radio_ui_engine::EngineSource, key: &str, value| {
        let _ = source
            .playout()
            .dispatch(radio_engine::Command::SettingsSet {
                part: radio_ui_engine::ICECAST_PART.into(),
                key: key.into(),
                value,
            })
            .unwrap();
    };
    set(&mut source, "host", serde_json::json!("stream.example.org"));
    let view = wait_for(&mut source, &clock, |v| {
        v.icecast.host == "stream.example.org"
    });
    assert!(view.icecast.tls, "a remote host gets TLS");

    set(&mut source, "host", serde_json::json!("127.0.0.1"));
    let view = wait_for(&mut source, &clock, |v| v.icecast.host == "127.0.0.1");
    assert!(!view.icecast.tls, "this computer gets no TLS");

    // A person turns TLS off for a remote host, with a password set.
    let mut mount = view.icecast.clone();
    mount.host = "stream.example.org".into();
    mount.mount = "/live".into();
    mount.password = "test-only".into();
    mount.tls = false;
    assert_eq!(source.edit(Edit::SetIcecast(mount)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.icecast.password_set && v.icecast.host == "stream.example.org"
    });
    assert!(!view.icecast.tls, "the choice beats the default");
    assert!(view.icecast.password_in_clear(), "the warning shows");
    let state = source.playout().snapshot().state;
    let cfg = icecast_config(&state, &Shared(store), "Test")
        .unwrap()
        .expect("a mount");
    assert!(!cfg.tls, "the start honours the choice");
}
