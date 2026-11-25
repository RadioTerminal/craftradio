//! Items of an over playlist lie on a song's intro or outro and take no airtime.

use jiff::Timestamp;
use jiff::civil::date;
use radio_model::{
    FixMode, Item, Placement, PlayMode, Playlist, PlaylistId, Repeat, Rule, Station, Trigger,
};
use radio_scheduler::{Entry, PlanInput, PlanOptions, PlayHistory, Side, Timeline, plan};
use radio_testkit::{jingle, playlist, station, track};
use std::time::Duration;

const MAIN: PlaylistId = PlaylistId(1);
const IDS: PlaylistId = PlaylistId(2);

fn now() -> Timestamp {
    "2026-11-21T13:00:00Z".parse().unwrap()
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A 180 s song with an intro to `intro` s and an outro from `outro` s (segue at 178 s).
fn song(id: u64, intro: Option<u64>, outro: Option<u64>) -> Item {
    let mut t = track(id, &format!("song-{id}"), &format!("band-{id}"), 180);
    let timing = t.timing.as_mut().unwrap();
    timing.intro_end = intro.map(secs);
    timing.outro_start = outro.map(secs);
    t
}

fn ids(len: u32, take: u8) -> Playlist {
    let mut p = playlist(2, "ids", (100..110).map(|i| jingle(i, len)).collect());
    p.plays = PlayMode::Over { duck_db: -12 };
    p.rules = vec![Rule {
        trigger: Trigger::EveryNTracks(2),
        placement: Placement::BetweenTracks,
        take,
        constraints: Vec::new(),
    }];
    p
}

fn songs(intro: Option<u64>, outro: Option<u64>) -> Playlist {
    let mut m = playlist(1, "main", (1..=30).map(|i| song(i, intro, outro)).collect());
    m.is_main = true;
    m
}

fn run(st: &Station) -> Timeline {
    let history = PlayHistory::new();
    plan(
        &PlanInput {
            station: st,
            history: &history,
            now: now(),
            horizon: Duration::from_secs(3600),
            pinned: &[],
        },
        &PlanOptions::default(),
        None,
    )
}

fn host<'a>(tl: &'a Timeline, o: &radio_scheduler::OverEntry) -> &'a Entry {
    tl.entries
        .iter()
        .find(|e| e.item == o.over.host && e.start == o.over.host_start)
        .expect("the host is in the plan")
}

fn in_sequence(tl: &Timeline) -> usize {
    tl.entries.iter().filter(|e| e.playlist == IDS).count()
}

#[test]
fn an_id_lies_on_the_next_intro_when_it_fits() {
    let tl = run(&station("UTC", vec![songs(Some(15), Some(160)), ids(8, 1)]));
    assert!(!tl.over.is_empty(), "no id lies over the music");
    assert_eq!(in_sequence(&tl), 0, "an id still takes a slot");
    for o in &tl.over {
        assert_eq!(o.over.side, Side::Intro);
        assert_eq!(o.entry.start, host(&tl, o).start, "it starts with its song");
        assert_eq!(o.over.duck_db, -12);
    }
}

#[test]
fn an_id_lies_on_the_outro_when_only_the_outro_fits() {
    let tl = run(&station("UTC", vec![songs(None, Some(160)), ids(8, 1)]));
    assert!(!tl.over.is_empty());
    for o in &tl.over {
        assert_eq!(o.over.side, Side::Outro);
        let h = host(&tl, o);
        // It ends at the host's segue, when the next song starts.
        assert_eq!(o.entry.start + o.entry.len, h.start + secs(178));
        assert!(
            o.entry.start >= h.start + secs(160),
            "it starts inside the outro"
        );
    }
}

#[test]
fn an_id_plays_in_sequence_when_nothing_fits() {
    let tl = run(&station("UTC", vec![songs(None, None), ids(8, 1)]));
    assert!(tl.over.is_empty());
    assert!(in_sequence(&tl) > 0);
}

#[test]
fn an_id_that_is_longer_than_every_intro_and_outro_plans_as_in_sequence() {
    let over = run(&station(
        "UTC",
        vec![songs(Some(15), Some(160)), ids(30, 1)],
    ));
    let mut plain = ids(30, 1);
    plain.plays = PlayMode::InSequence;
    let seq = run(&station("UTC", vec![songs(Some(15), Some(160)), plain]));
    assert!(over.over.is_empty());
    assert_eq!(over.entries, seq.entries);
}

#[test]
fn an_id_over_the_music_takes_no_airtime() {
    let tl = run(&station("UTC", vec![songs(Some(15), Some(160)), ids(8, 1)]));
    for w in tl.entries.windows(2) {
        assert_eq!(
            w[1].start,
            w[0].start + secs(178),
            "a gap between two songs"
        );
    }
    let mut plain = ids(8, 1);
    plain.plays = PlayMode::InSequence;
    let seq = run(&station("UTC", vec![songs(Some(15), Some(160)), plain]));
    let tenth = |t: &Timeline| {
        t.entries
            .iter()
            .filter(|e| e.playlist == MAIN)
            .nth(9)
            .map(|e| (e.item, e.start))
            .unwrap()
    };
    let ((over_song, over_at), (seq_song, seq_at)) = (tenth(&tl), tenth(&seq));
    assert_eq!(over_song, seq_song);
    assert!(over_at < seq_at, "the tenth song does not start sooner");
}

#[test]
fn two_ids_due_at_one_handover_never_overlap() {
    let tl = run(&station("UTC", vec![songs(Some(15), Some(160)), ids(8, 2)]));
    let mut windows: Vec<_> = tl
        .over
        .iter()
        .map(|o| (o.entry.start, o.entry.start + o.entry.len))
        .collect();
    windows.sort();
    assert!(windows.len() > 1);
    for w in windows.windows(2) {
        assert!(w[0].1 <= w[1].0, "two ids overlap: {w:?}");
    }
    assert!(
        in_sequence(&tl) > 0,
        "the second id at a handover plays in sequence"
    );
}

fn with_news(mut st: Station, at_minute: i8) -> Station {
    let mut news = playlist(3, "news", vec![jingle(300, 60)]);
    news.rules = vec![Rule {
        trigger: Trigger::At {
            at: date(2026, 11, 21).at(13, at_minute, 0, 0),
            repeat: Repeat::Once,
        },
        placement: Placement::Fixed {
            mode: FixMode::Hard,
            tolerance: secs(1),
        },
        take: 1,
        constraints: Vec::new(),
    }];
    let mut pool = playlist(9, "fill", (0..12).map(|i| jingle(200 + i, 20)).collect());
    pool.is_fill_pool = true;
    st.playlists.push(news);
    st.playlists.push(pool);
    st
}

#[test]
fn a_hard_event_still_starts_on_its_target() {
    let tl = run(&with_news(
        station("UTC", vec![songs(Some(15), Some(160)), ids(8, 1)]),
        20,
    ));
    let target: Timestamp = "2026-11-21T13:20:00Z".parse().unwrap();
    let news = tl
        .entries
        .iter()
        .find(|e| e.item.0 == 300)
        .expect("the news is planned");
    assert_eq!(news.start, target);
    assert!(!tl.over.is_empty());
}

#[test]
fn a_trimmed_song_takes_no_outro_id() {
    let tl = run(&with_news(
        station("UTC", vec![songs(None, Some(160)), ids(8, 1)]),
        20,
    ));
    assert!(
        tl.entries.iter().any(|e| e.trim > Duration::ZERO),
        "no song was trimmed"
    );
    for o in &tl.over {
        assert_eq!(
            host(&tl, o).trim,
            Duration::ZERO,
            "an id lies on a trimmed outro"
        );
    }
}

#[test]
fn a_locked_id_still_lies_over_the_music() {
    let mut locked = ids(8, 1);
    for i in &mut locked.items {
        i.locked = true;
    }
    let tl = run(&station("UTC", vec![songs(Some(15), Some(160)), locked]));
    assert!(!tl.over.is_empty());
    assert_eq!(in_sequence(&tl), 0);
}

#[test]
fn a_fixed_id_plays_in_sequence_at_its_time() {
    let mut fixed = ids(8, 1);
    fixed.rules = vec![Rule {
        trigger: Trigger::At {
            at: date(2026, 11, 21).at(13, 30, 0, 0),
            repeat: Repeat::Once,
        },
        placement: Placement::Fixed {
            mode: FixMode::Hard,
            tolerance: secs(1),
        },
        take: 1,
        constraints: Vec::new(),
    }];
    let tl = run(&station("UTC", vec![songs(Some(15), Some(160)), fixed]));
    assert!(tl.over.is_empty());
    let id = tl
        .entries
        .iter()
        .find(|e| e.playlist == IDS)
        .expect("the id is planned");
    assert!(id.fixed.is_some());
}

/// Songs on the decks are pinned, and nothing is woven between pinned entries. An over
/// item that the previous plan laid on a pinned song stays on it.
#[test]
fn an_id_on_a_pinned_song_stays_in_the_next_plan() {
    let st = station("UTC", vec![songs(Some(15), Some(160)), ids(8, 1)]);
    let first = run(&st);
    let o = first.over.first().expect("an id over the music").clone();
    let host_at = first
        .entries
        .iter()
        .position(|e| e.item == o.over.host && e.start == o.over.host_start)
        .unwrap();
    let pinned: Vec<Entry> = first.entries[..=host_at].to_vec();
    let history = PlayHistory::new();
    let next = plan(
        &PlanInput {
            station: &st,
            history: &history,
            now: now(),
            horizon: Duration::from_secs(3600),
            pinned: &pinned,
        },
        &PlanOptions::default(),
        Some(&first),
    );
    assert!(
        next.over
            .iter()
            .any(|n| n.over.host == o.over.host && n.over.host_start == o.over.host_start),
        "the id on the pinned song is gone"
    );
}
