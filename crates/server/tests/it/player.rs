//! The listener page: each station's is served at `/<slug>/`, a static page that is checked
//! again on each visit, and an unknown station has none.

use crate::support::{PATIENCE, read_to_end_within, request};
use radio_server::{Config, Server, ServerHandle, hash_key};

fn start() -> ServerHandle {
    let config = format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
         [[station]]\nslug = \"dawn\"\nname = \"Dawn Chorus\"\nkey_hash = \"{}\"\n",
        hash_key("invented-dawn-key-7Kp2"),
    );
    Server::start(Config::parse(&config).unwrap()).unwrap()
}

#[test]
fn a_station_gets_the_listener_page_at_its_slug() {
    let server = start();
    let addr = server.local_addr();
    for path in ["/dawn/", "/dawn"] {
        let (mut stream, head) = request(addr, "GET", path);
        assert_eq!(head.status, 200, "{path}");
        assert_eq!(
            head.header("Content-Type"),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
        assert_eq!(head.header("Cache-Control"), Some("no-cache"), "{path}");
        assert_eq!(head.header("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(head.header("Referrer-Policy"), Some("no-referrer"));
        assert_eq!(head.header("X-Frame-Options"), Some("DENY"), "never framed");
        let body = String::from_utf8(read_to_end_within(&mut stream, PATIENCE)).unwrap();
        assert!(body.starts_with("<!doctype html"), "an HTML page");
        // The same page GitHub Pages publishes, which plays a station by its ID.
        assert!(body.contains("status-json.xsl"), "the listener page");
    }
}

#[test]
fn an_unknown_station_has_no_page() {
    let server = start();
    let addr = server.local_addr();
    for path in ["/noon/", "/noon", "/", "/dawn/stream/"] {
        let (_stream, head) = request(addr, "GET", path);
        assert_eq!(head.status, 404, "{path}");
    }
}

#[test]
fn a_head_gets_the_page_headers_and_no_body() {
    let server = start();
    let addr = server.local_addr();
    let (mut stream, head) = request(addr, "HEAD", "/dawn/");
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("Content-Type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(head.header("X-Content-Type-Options"), Some("nosniff"));
    let (_get, get) = request(addr, "GET", "/dawn/");
    assert_eq!(
        head.header("Content-Length"),
        get.header("Content-Length"),
        "the length of the page a GET gets"
    );
    assert!(
        read_to_end_within(&mut stream, PATIENCE).is_empty(),
        "no body"
    );
}

/// A push to the page's path is 404, never 405, which the app would take as "try SOURCE".
#[test]
fn a_push_to_the_page_is_404() {
    let server = start();
    let addr = server.local_addr();
    for method in ["PUT", "SOURCE"] {
        for path in ["/dawn", "/dawn/"] {
            let (_stream, head) = request(addr, method, path);
            assert_eq!(head.status, 404, "{method} {path}");
        }
    }
}
