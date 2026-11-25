//! Items of an over playlist play on the cart bus over their song, ducking it.

use std::time::Duration;

use radio_engine::{Command, Engine};
use radio_model::{Item, ItemId, MarkerSource, PlayMode, Timing};
use radio_playout::Playout;

use crate::fixtures::{
    SR, Side, add_playlist, engine_with, file_item, first_audible, offline, rms_db, run, wav,
};

const EVERY_TRACK: &str =
    r#"rule({every: {tracks: 1}, place: "between", take: 1, constraints: []});"#;

/// Four 6 s songs on the left channel, each with an intro to `intro` s and an outro from
/// `outro` s.
fn songs(name: &str, intro: Option<f32>, outro: Option<f32>) -> Vec<Item> {
    let path = wav(name, 6.0, Side::Left);
    (1..=4)
        .map(|i| {
            let mut s = file_item(i, &format!("Song {i}"), &path, 6.0, 6.0, 0.0);
            let t = s.timing.as_mut().unwrap();
            t.intro_end = intro.map(Duration::from_secs_f32);
            t.outro_start = outro.map(Duration::from_secs_f32);
            t.markers = MarkerSource::Detected;
            s
        })
        .collect()
}

/// An engine with `main` and a 1 s station ID on the right channel after every track,
/// playing over the music at −12 dB.
fn station(name: &str, main: Vec<Item>) -> Engine {
    let mut engine = engine_with(main);
    let id = wav(&format!("{name}-id"), 1.0, Side::Right);
    let ids = add_playlist(
        &mut engine,
        "IDs",
        vec![file_item(50, "Station ID", &id, 1.0, 1.0, 0.0)],
        EVERY_TRACK,
    );
    engine
        .dispatch(Command::PlaylistSetPlays {
            playlist: ids,
            plays: PlayMode::Over { duck_db: -12 },
        })
        .unwrap();
    engine
}

fn at(s: f32) -> usize {
    (s * SR as f32) as usize
}

/// When the right channel (the ID) first sounds, in seconds.
fn id_starts(program: &[f32]) -> Option<f32> {
    first_audible(program, 1, 0).map(|f| f as f32 / SR as f32)
}

/// The song keeps sounding while the ID plays: it lies over the music, not between songs.
fn over_the_music(program: &[f32], id_start: f32) -> bool {
    rms_db(program, 0, at(id_start + 0.3), at(id_start + 0.7)) > -40.0
}

fn deck_of(p: &Playout, item: u64) -> u8 {
    p.deck_items()
        .iter()
        .position(|d| *d == Some(ItemId(item)))
        .expect("the song is on a deck") as u8
}

#[test]
fn an_id_plays_over_the_next_songs_intro_and_ducks_it() {
    let (p, clock) = offline(
        station("ov-intro", songs("ov-intro", Some(2.0), None)),
        |_| {},
    );
    let program = run(&p, &clock, 9.0);
    let start = id_starts(&program).expect("the ID sounds");
    assert!(
        (start - 6.0).abs() < 0.05,
        "the ID starts at {start} s, not with song 2"
    );
    assert!(
        over_the_music(&program, start),
        "the ID played between songs"
    );
    let song = rms_db(&program, 0, at(4.0), at(5.5));
    let ducked = rms_db(&program, 0, at(6.3), at(6.9));
    assert!(
        (song - ducked - 12.0).abs() < 2.0,
        "ducked by {} dB",
        song - ducked
    );
    let after = rms_db(&program, 0, at(7.8), at(8.8));
    assert!((song - after).abs() < 1.0, "the song did not come back");
    let rows = p
        .history("2026-01-01T00:00:00Z".parse().unwrap(), 50)
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.entry.title == "Station ID" && r.entry.origin.starts_with("over"))
    );
}

#[test]
fn an_outro_id_follows_its_song_when_the_song_was_paused() {
    let (p, clock) = offline(
        station("ov-pause-ok", songs("ov-pause-ok", None, Some(3.0))),
        |_| {},
    );
    let mut program = run(&p, &clock, 1.0);
    let deck = deck_of(&p, 1);
    p.send(Command::DeckPause {
        deck,
        expect_item: Some(ItemId(1)),
    })
    .unwrap();
    program.extend(run(&p, &clock, 2.0));
    p.send(Command::DeckPlay {
        deck,
        expect_item: Some(ItemId(1)),
        fade_in: Duration::ZERO,
    })
    .unwrap();
    program.extend(run(&p, &clock, 7.0));
    // The outro ID ends at song 1's segue: 6 s into the song, 2 s later for the pause.
    let start = id_starts(&program).expect("the ID sounds");
    assert!((start - 7.0).abs() < 0.1, "the ID starts at {start} s");
    assert!(
        over_the_music(&program, start),
        "the ID played between songs"
    );
}

#[test]
fn an_id_is_skipped_when_its_song_is_cut_first() {
    let (p, clock) = offline(station("ov-cut", songs("ov-cut", None, Some(3.0))), |_| {});
    let mut program = run(&p, &clock, 4.0);
    let deck = deck_of(&p, 1);
    p.send(Command::DeckSkip {
        deck,
        expect_item: Some(ItemId(1)),
    })
    .unwrap();
    program.extend(run(&p, &clock, 1.5));
    assert!(
        first_audible(&program, 1, at(4.0)).is_none(),
        "the ID sounded over the next song"
    );
    let rows = p
        .history("2026-01-01T00:00:00Z".parse().unwrap(), 50)
        .unwrap();
    assert!(
        !rows.iter().any(|r| r.entry.title == "Station ID"
            && r.entry.start < clock_start(&p) + jiff::SignedDuration::from_secs(6)),
        "a skipped ID is logged"
    );
}

fn clock_start(p: &Playout) -> jiff::Timestamp {
    p.snapshot().now - jiff::SignedDuration::from_millis(5500)
}

#[test]
fn an_id_is_skipped_while_its_song_is_paused() {
    let (p, clock) = offline(
        station("ov-pause", songs("ov-pause", None, Some(3.0))),
        |_| {},
    );
    let mut program = run(&p, &clock, 4.0);
    let deck = deck_of(&p, 1);
    p.send(Command::DeckPause {
        deck,
        expect_item: Some(ItemId(1)),
    })
    .unwrap();
    program.extend(run(&p, &clock, 3.0));
    assert!(
        first_audible(&program, 1, 0).is_none(),
        "the ID sounded over a paused song"
    );
}

/// Markers that only the analysis found still place an ID.
fn intro_analysis(_: &std::path::Path) -> Option<Timing> {
    Some(Timing {
        cue_in: Duration::ZERO,
        segue: Duration::from_secs(6),
        cue_out: Duration::from_secs(6),
        intro_end: Some(Duration::from_secs(2)),
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
        outro_start: None,
        markers: MarkerSource::Detected,
    })
}

#[test]
fn markers_from_the_analysis_place_an_id() {
    let mut main = songs("ov-analysed", None, None);
    for s in &mut main {
        s.timing.as_mut().unwrap().markers = MarkerSource::NotAnalysed;
    }
    let (p, clock) = offline(station("ov-analysed", main), |cfg| {
        cfg.analyse = intro_analysis
    });
    // The IDs already on the decks when the analysis lands stay in sequence; a later one
    // lies over a song.
    let program = run(&p, &clock, 60.0);
    // Both channels sound together for 100 ms in a row, which no hand-over between songs
    // does.
    let step = SR / 100;
    let mut run_len = 0;
    let mut both = false;
    for w in 0..program.len() / 2 / step {
        let (a, b) = (w * step, w * step + step);
        let together = rms_db(&program, 0, a, b) > -40.0 && rms_db(&program, 1, a, b) > -40.0;
        run_len = if together { run_len + 1 } else { 0 };
        both |= run_len >= 10;
    }
    assert!(both, "no ID sounded over a song");
}

#[test]
fn a_full_cartwall_leaves_the_slot_of_over_items() {
    let mut engine = station("ov-wall", songs("ov-wall", Some(2.0), None));
    // Silent carts: if one took the slot of over items, the ID would not sound.
    let cart = crate::fixtures::wav_at("ov-wall-cart", 1.0, 0.0, None);
    add_playlist(
        &mut engine,
        "Carts",
        (60..64)
            .map(|i| file_item(i, "Horn", &cart, 1.0, 1.0, 0.0))
            .collect(),
        "",
    );
    for (slot, item) in (60..63).enumerate() {
        engine
            .dispatch(Command::CartAssign {
                page: 0,
                slot: slot as u32,
                item: ItemId(item),
            })
            .unwrap();
    }
    // Three carts on a wall of three slots; over items have a slot of their own.
    let (p, clock) = offline(engine, |cfg| cfg.mixer.cart_slots = 3);
    let mut program = run(&p, &clock, 1.0);
    // A new cart on the third place after the ID loaded: it must not replace the ID.
    p.send(Command::CartAssign {
        page: 0,
        slot: 2,
        item: ItemId(63),
    })
    .unwrap();
    program.extend(run(&p, &clock, 8.0));
    let start = id_starts(&program).expect("the ID sounds");
    assert!((start - 6.0).abs() < 0.05, "the ID starts at {start} s");
    assert!(
        over_the_music(&program, start),
        "the ID played between songs"
    );
}

/// Two different IDs in a row: loading the second never cuts the first short.
#[test]
fn the_next_id_loads_without_cutting_the_one_on_air() {
    let mut engine = engine_with(songs("ov-two", Some(2.0), None));
    let one = wav("ov-two-a", 1.0, Side::Right);
    let two = wav("ov-two-b", 1.0, Side::Right);
    let ids = add_playlist(
        &mut engine,
        "IDs",
        vec![
            file_item(50, "Station ID", &one, 1.0, 1.0, 0.0),
            file_item(51, "Sweeper", &two, 1.0, 1.0, 0.0),
        ],
        EVERY_TRACK,
    );
    engine
        .dispatch(Command::PlaylistSetPlays {
            playlist: ids,
            plays: PlayMode::Over { duck_db: -12 },
        })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let program = run(&p, &clock, 14.0);
    let start = id_starts(&program).expect("the first ID sounds");
    // The whole first ID sounds, to its last tenth of a second.
    let tail = rms_db(&program, 1, at(start + 0.8), at(start + 0.95));
    assert!(
        tail > -30.0,
        "the first ID was cut: its end is at {tail} dB"
    );
}

/// After a seek, an outro ID still starts where the song reaches its outro.
#[test]
fn an_outro_id_follows_a_seek_in_its_song() {
    let (p, clock) = offline(
        station("ov-seek", songs("ov-seek", None, Some(3.0))),
        |_| {},
    );
    let mut program = run(&p, &clock, 1.0);
    let deck = deck_of(&p, 1);
    // Jump from 1 s to 3 s into song 1: its outro ID, due 5 s into the song, now comes at 3 s.
    p.send(Command::DeckSeek {
        deck,
        position: Duration::from_secs(3),
        expect_item: Some(ItemId(1)),
    })
    .unwrap();
    program.extend(run(&p, &clock, 4.0));
    let start = id_starts(&program).expect("the ID sounds");
    assert!((start - 3.0).abs() < 0.15, "the ID starts at {start} s");
}

/// A hard event that goes on air late moves the songs after it; their IDs move with them.
#[test]
fn ids_follow_the_songs_after_a_late_hard_event() {
    let mut engine = station("ov-late", songs("ov-late", Some(2.0), None));
    let news = wav("ov-late-news", 2.0, Side::Left);
    let mut item = file_item(70, "News", &news, 2.0, 2.0, 0.0);
    item.movable = false;
    item.trimmable = false;
    add_playlist(
        &mut engine,
        "News",
        vec![item],
        r#"rule({at: {date: "2026-11-21", time: "13:00:06", repeat: "once"},
                place: {fixed: "hard", tolerance: 1}, take: 1, constraints: []});"#,
    );
    let (p, clock) = offline(engine, |_| {});
    let mut program = run(&p, &clock, 6.5);
    let deck = deck_of(&p, 70);
    p.send(Command::DeckPause {
        deck,
        expect_item: Some(ItemId(70)),
    })
    .unwrap();
    program.extend(run(&p, &clock, 2.0));
    p.send(Command::DeckPlay {
        deck,
        expect_item: Some(ItemId(70)),
        fade_in: Duration::ZERO,
    })
    .unwrap();
    program.extend(run(&p, &clock, 6.0));
    let tl = p.snapshot().timeline;
    for o in &tl.over {
        assert!(
            tl.entries
                .iter()
                .any(|e| e.item == o.over.host && e.start == o.over.host_start),
            "an ID lost its song: host {} at {}",
            o.over.host.0,
            o.over.host_start
        );
    }
    // The news ends at 10 s instead of 8 s; the song after it carries an ID over its intro.
    let after = first_audible(&program, 1, at(9.0)).expect("an ID sounds after the news");
    let after = after as f32 / SR as f32;
    assert!(
        over_the_music(&program, after),
        "the ID after the news played between songs"
    );
}

/// Over items have a slot of their own: every place of the cartwall still fires its cart.
#[test]
fn every_cart_of_a_full_wall_still_fires() {
    let mut engine = station("ov-full", songs("ov-full", Some(2.0), None));
    let horn = wav("ov-full-horn", 0.5, Side::Right);
    add_playlist(
        &mut engine,
        "Carts",
        (60..63)
            .map(|i| file_item(i, "Horn", &horn, 0.5, 0.5, 0.0))
            .collect(),
        "",
    );
    for (slot, item) in (60..63).enumerate() {
        engine
            .dispatch(Command::CartAssign {
                page: 0,
                slot: slot as u32,
                item: ItemId(item),
            })
            .unwrap();
    }
    let (p, clock) = offline(engine, |cfg| cfg.mixer.cart_slots = 3);
    let mut program = run(&p, &clock, 8.5);
    p.send(Command::CartFire { page: 0, slot: 2 }).unwrap();
    program.extend(run(&p, &clock, 1.0));
    assert!(
        rms_db(&program, 1, at(8.6), at(8.9)) > -30.0,
        "the third cart of a three-cart wall did not fire"
    );
}
