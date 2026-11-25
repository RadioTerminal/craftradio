//! What a station played before: the server keeps it, so a listener who opens the page
//! sees it at once, and the page reads it at `GET /<slug>/history`.

use crate::cover::{config, put};
use crate::server::{MORNING_KEY, Pusher, Tone, on_air, push_when_free, set_title};
use crate::support::{Head, PATIENCE, client, read_head, read_to_end_within};
use radio_server::{Cover, HISTORY_MAX, HISTORY_SPAN, History, LEADING_COVER, Server};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const NAME: &str = "Morning Station";

fn cover(n: u64) -> Arc<Cover> {
    Arc::new(Cover::new(
        "image/png",
        format!("invented-cover-{n}").into_bytes(),
        n,
    ))
}

fn titles(history: &History, at: Instant) -> Vec<String> {
    history.played(at).map(|p| p.title.clone()).collect()
}

fn covered(history: &History, at: Instant, title: &str) -> Option<Arc<Cover>> {
    history
        .played(at)
        .find(|p| p.title == title)
        .and_then(|p| p.cover.clone())
}

#[test]
fn a_title_joins_the_history_once_another_takes_its_place() {
    let start = Instant::now();
    let mut history = History::default();
    history.title("Paper Lanterns", "Invented Band", NAME, start);
    assert!(
        titles(&history, start).is_empty(),
        "what plays now is not history"
    );
    let later = start + Duration::from_secs(200);
    history.title("Slow River", "Other Band", NAME, later);
    history.title("Glass Hours", "", NAME, later + Duration::from_secs(180));
    assert_eq!(
        titles(&history, later + Duration::from_secs(181)),
        ["Slow River", "Paper Lanterns"],
        "newest first"
    );
}

#[test]
fn a_dropped_push_coming_back_with_the_same_title_adds_nothing() {
    let start = Instant::now();
    let mut history = History::default();
    history.title("Paper Lanterns", "Invented Band", NAME, start);
    // The source ended: the mount names no title, which ends nothing.
    history.title("", "", NAME, start + Duration::from_secs(10));
    history.title(
        "Paper Lanterns",
        "Invented Band",
        NAME,
        start + Duration::from_secs(20),
    );
    assert!(titles(&history, start + Duration::from_secs(21)).is_empty());
}

#[test]
fn a_title_that_is_the_station_name_stays_out() {
    let start = Instant::now();
    let mut history = History::default();
    history.title(NAME, "", NAME, start);
    history.title("Paper Lanterns", "", NAME, start + Duration::from_secs(30));
    history.title("Slow River", "", NAME, start + Duration::from_secs(60));
    assert_eq!(
        titles(&history, start + Duration::from_secs(61)),
        ["Paper Lanterns"]
    );
}

#[test]
fn a_cover_after_its_title_and_one_just_before_it_both_go_to_that_title() {
    let start = Instant::now();
    let mut history = History::default();
    // The cover comes after the title.
    history.title("Paper Lanterns", "", NAME, start);
    history.cover(Some(cover(1)), start + Duration::from_millis(300));
    // The next cover comes just before its title.
    let next = start + Duration::from_secs(200);
    history.cover(Some(cover(2)), next);
    history.title("Slow River", "", NAME, next + LEADING_COVER / 2);
    history.title("Glass Hours", "", NAME, next + Duration::from_secs(150));
    let at = next + Duration::from_secs(151);
    assert_eq!(covered(&history, at, "Paper Lanterns"), Some(cover(1)));
    assert_eq!(covered(&history, at, "Slow River"), Some(cover(2)));
    // An old cover stays with its own title.
    let mut history = History::default();
    history.title("Paper Lanterns", "", NAME, start);
    history.cover(Some(cover(1)), start + Duration::from_millis(300));
    history.title("Slow River", "", NAME, start + LEADING_COVER * 4);
    history.title("Glass Hours", "", NAME, start + LEADING_COVER * 8);
    let at = start + LEADING_COVER * 9;
    assert_eq!(covered(&history, at, "Paper Lanterns"), Some(cover(1)));
    assert_eq!(covered(&history, at, "Slow River"), None);
}

#[test]
fn a_cleared_cover_leaves_its_item_without_one() {
    let start = Instant::now();
    let mut history = History::default();
    history.title("Paper Lanterns", "", NAME, start);
    history.cover(Some(cover(1)), start + Duration::from_secs(1));
    history.title("Slow River", "", NAME, start + Duration::from_secs(100));
    history.cover(None, start + Duration::from_secs(101));
    history.title("Glass Hours", "", NAME, start + Duration::from_secs(200));
    let at = start + Duration::from_secs(201);
    assert_eq!(covered(&history, at, "Paper Lanterns"), Some(cover(1)));
    assert_eq!(covered(&history, at, "Slow River"), None);
}

#[test]
fn the_history_keeps_the_last_hour_and_at_most_its_limit() {
    let start = Instant::now();
    let mut history = History::default();
    let count = HISTORY_MAX + 5;
    for i in 0..=count {
        history.title(
            &format!("Song {i}"),
            "",
            NAME,
            start + Duration::from_secs(i as u64),
        );
    }
    let now = start + Duration::from_secs(count as u64);
    let kept = titles(&history, now);
    assert_eq!(kept.len(), HISTORY_MAX);
    assert_eq!(kept[0], format!("Song {}", count - 1), "newest first");
    // An hour on, every item started longer ago than the history reaches.
    assert!(titles(&history, now + HISTORY_SPAN).is_empty());
}

/// `GET <path>` on the server.
fn get(addr: SocketAddr, path: &str) -> (Head, Vec<u8>) {
    let mut stream = client(addr);
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: radio.example.test\r\n\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    let body = read_to_end_within(&mut stream, PATIENCE);
    (head, body)
}

#[test]
fn a_listener_who_opens_the_page_later_gets_what_played_with_its_cover() {
    let server = Server::start(config(None)).unwrap();
    let addr = server.local_addr();
    let pusher = on_air(addr, "morning", MORNING_KEY, Tone::Mp3 { rate: 44_100 });
    // The app sends the cover as the item starts, here just before its title.
    let picture = b"\x89PNG\r\n\x1a\ninvented-history-cover";
    assert_eq!(put(addr, MORNING_KEY, "image/png", picture), 204);
    assert_eq!(
        set_title(addr, "morning", "Paper Lanterns", Some(MORNING_KEY))
            .1
            .status,
        200
    );
    assert_eq!(
        set_title(addr, "morning", "Slow River", Some(MORNING_KEY))
            .1
            .status,
        200
    );
    // The push drops and comes back with the same title: nothing more joins.
    drop(pusher);
    let tone = Tone::Mp3 { rate: 44_100 };
    let _pusher = Pusher::start(
        push_when_free(addr, "morning", MORNING_KEY, "audio/mpeg"),
        tone,
    );
    assert_eq!(
        set_title(addr, "morning", "Slow River", Some(MORNING_KEY))
            .1
            .status,
        200
    );

    let (head, body) = get(addr, "/morning/history");
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(head.header("Cache-Control"), Some("no-store"));
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = doc["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{doc}");
    assert_eq!(items[0]["title"], "Paper Lanterns");
    assert!(items[0]["ago_ms"].as_u64().is_some());
    let path = items[0]["cover"].as_str().expect("its cover's path");
    let (head, bytes) = get(addr, path);
    assert_eq!(head.status, 200);
    assert_eq!(bytes, picture);
    assert_eq!(head.header("Content-Type"), Some("image/png"));

    assert_eq!(
        get(addr, "/morning/cover/00ff").0.status,
        404,
        "no such cover"
    );
    assert_eq!(get(addr, "/noon/history").0.status, 404, "no such station");
}
