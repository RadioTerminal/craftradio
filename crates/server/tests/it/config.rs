use radio_server::*;

const TWO: &str = r#"
listen = "127.0.0.1:8080"
public_url = "https://radio.example.org"
[[station]]
slug = "morning"
name = "Morning Station"
key_hash = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
[[station]]
slug = "night"
name = "Night Station"
key_hash = "sha256:1111111111111111111111111111111111111111111111111111111111111111"
max_stream_listeners = 5
"#;

/// A well-formed key hash that is plainly a placeholder.
const PLACEHOLDER_HASH: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn one_station(slug: &str) -> String {
    station_with_hash(slug, PLACEHOLDER_HASH)
}

fn station_with_hash(slug: &str, key_hash: &str) -> String {
    format!(
        r#"
listen = "127.0.0.1:8080"
public_url = "https://radio.example.org"
[[station]]
slug = "{slug}"
name = "Morning Station"
key_hash = "{key_hash}"
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
fn rejects_a_key_hash_that_is_not_a_sha256_and_never_echoes_it() {
    let good = hash_key("invented-morning-key-4Hq8");
    assert!(Config::parse(&station_with_hash("morning", &good)).is_ok());
    let hex = good.strip_prefix("sha256:").unwrap();
    for bad in [
        // Cut short, as a copy that missed the end.
        "sha256:00".to_owned(),
        good[..good.len() - 1].to_owned(),
        // The key itself, pasted in place of its hash.
        "invented-pasted-key-Lm52".to_owned(),
        format!("{good}0"),
        format!("md5:{hex}"),
        format!("sha256:{}", "g".repeat(64)),
        format!("sha256: {hex}"),
    ] {
        match Config::parse(&station_with_hash("morning", &bad)) {
            Err(ServerError::Config(message)) => {
                assert!(message.contains("morning"), "{message}");
                assert!(
                    !message.contains(&bad),
                    "the message shows the value: {message}"
                );
            }
            other => panic!("{bad:?} was accepted: {other:?}"),
        }
    }
}

#[test]
fn rejects_two_stations_with_the_same_key() {
    let hash = hash_key("invented-morning-key-4Hq8");
    let hex = hash.strip_prefix("sha256:").unwrap();
    // The same hash, once in capitals: it is still the same key.
    let text = format!(
        "{}[[station]]\nslug = \"night\"\nname = \"Night Station\"\nkey_hash = \"sha256:{}\"\n",
        station_with_hash("morning", &hash),
        hex.to_uppercase()
    );
    match Config::parse(&text) {
        Err(ServerError::Config(message)) => {
            assert!(
                message.contains("morning") && message.contains("night"),
                "{message}"
            );
            assert!(
                !message.contains(hex),
                "the message shows the hash: {message}"
            );
        }
        other => panic!("two stations with one key were accepted: {other:?}"),
    }
}

#[test]
fn a_slug_in_an_error_is_escaped() {
    // A slug with a line break and a quote in it (written as TOML escapes) cannot start a
    // line of its own in the error, or pass for the end of the quoted slug.
    match Config::parse(&one_station(r#"morning\nnight\""#)) {
        Err(ServerError::Config(message)) => {
            assert!(!message.contains('\n'), "{message}");
            assert!(message.contains(r#""morning\nnight\"""#), "{message}");
        }
        other => panic!("the slug was accepted: {other:?}"),
    }
}

#[test]
fn rejects_unknown_field() {
    let on_station = TWO.replacen(
        "name = \"Morning Station\"",
        "name = \"Morning Station\"\nkey = \"plain\"",
        1,
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
