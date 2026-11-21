use crate::fixtures::*;
use radio_playout::Clock;
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
    let mut source = source.with_secrets(Box::new(Shared(store.clone())));
    let secret = "hunter-test-only";
    let mount = IcecastView {
        host: "stream.example.org".into(),
        port: 8000,
        mount: "/live".into(),
        user: "source".into(),
        password: secret.into(),
        password_set: false,
        format: StreamFormat::Opus,
        kbps: 96,
    };
    assert_eq!(source.edit(Edit::SetIcecast(mount)), Ok(1));
    let view = source.snapshot(clock.now());
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
