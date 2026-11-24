use radio_server::*;

const TWO: &str = r#"
listen = "127.0.0.1:8080"
public_url = "https://radio.example.org"
[[station]]
slug = "morning"
name = "Morning Station"
key_hash = "sha256:00"
[[station]]
slug = "night"
name = "Night Station"
key_hash = "sha256:11"
max_stream_listeners = 5
"#;

fn one_station(slug: &str) -> String {
    format!(
        r#"
listen = "127.0.0.1:8080"
public_url = "https://radio.example.org"
[[station]]
slug = "{slug}"
name = "Morning Station"
key_hash = "sha256:00"
"#
    )
}

#[test]
fn parses_stations_with_default_cap() {
    let c = Config::parse(TWO).unwrap();
    assert_eq!(c.stations.len(), 2);
    assert_eq!(c.station("morning").unwrap().max_stream_listeners, 200);
    assert_eq!(c.station("night").unwrap().max_stream_listeners, 5);
    assert!(c.station("noon").is_none());
}

#[test]
fn rejects_duplicate_slug() {
    let text = TWO.replace("slug = \"night\"", "slug = \"morning\"");
    match Config::parse(&text) {
        Err(ServerError::Config(message)) => assert!(message.contains("morning"), "{message}"),
        other => panic!("expected a config error, got {other:?}"),
    }
}

#[test]
fn rejects_slug_that_is_not_a_path_segment() {
    assert!(Config::parse(&one_station("morning-2")).is_ok());
    for slug in ["", "a/b", "..", "A b", "admin", "api", "status-json.xsl"] {
        assert!(
            matches!(
                Config::parse(&one_station(slug)),
                Err(ServerError::Config(_))
            ),
            "slug {slug:?} was accepted"
        );
    }
}

#[test]
fn rejects_unknown_field() {
    let on_station = TWO.replace(
        "key_hash = \"sha256:00\"",
        "key_hash = \"sha256:00\"\nkey = \"plain\"",
    );
    assert!(matches!(
        Config::parse(&on_station),
        Err(ServerError::Config(_))
    ));
    let at_top = TWO.replacen("[[station]]", "media = \"x\"\n[[station]]", 1);
    assert!(matches!(
        Config::parse(&at_top),
        Err(ServerError::Config(_))
    ));
}

#[test]
fn rejects_one_tls_path_without_the_other() {
    let with = |lines: &str| TWO.replacen("[[station]]", &format!("{lines}\n[[station]]"), 1);
    assert!(Config::parse(&with("tls_cert = \"cert.pem\"\ntls_key = \"key.pem\"")).is_ok());
    assert!(matches!(
        Config::parse(&with("tls_cert = \"cert.pem\"")),
        Err(ServerError::Config(_))
    ));
    assert!(matches!(
        Config::parse(&with("tls_key = \"key.pem\"")),
        Err(ServerError::Config(_))
    ));
}

#[test]
fn a_key_matches_only_its_own_hash() {
    let (a, b) = (new_key().unwrap(), new_key().unwrap());
    assert_ne!(a, b);
    assert!(key_matches(&a, &hash_key(&a)));
    assert!(!key_matches(&b, &hash_key(&a)));
    assert!(!key_matches(&a, "not a hash"));
}

/// The example config shipped in `packaging/server` parses, so it can't drift from what the
/// server accepts; with its commented-out TLS lines turned on it still parses.
#[test]
fn the_packaged_example_config_loads() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/server/craftradio-server.example.toml");
    let config = Config::load(&path).unwrap();
    assert!(
        config.stations.len() >= 2,
        "the example shows more than one station"
    );
    assert_eq!(config.tls_cert, None);

    let text = std::fs::read_to_string(&path).unwrap();
    let with_tls = text.replace("# tls_", "tls_");
    let config = Config::parse(&with_tls).unwrap();
    assert!(config.tls_cert.is_some() && config.tls_key.is_some());
}
