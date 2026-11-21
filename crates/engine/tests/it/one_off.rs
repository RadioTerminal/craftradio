//! One-off items in Upcoming: `schedule.add`, `schedule.update` and `schedule.remove`.

use jiff::civil::{DateTime, date};
use radio_engine::{Command, Engine};
use radio_model::{FixMode, Item, ItemId, PlaylistRole, Repeat, Source};
use std::time::Duration;

use super::temp_db;

fn file(title: &str) -> Item {
    Item {
        id: ItemId(0),
        title: title.into(),
        artist: None,
        source: Source::File {
            path: "/music/station-id.wav".into(),
        },
        timing: None,
        planned_len: Some(Duration::from_secs(30)),
        movable: true,
        trimmable: true,
        locked: false,
        valid_from: None,
        valid_to: None,
        max_per_day: None,
        meta: Default::default(),
    }
}

fn add(e: &mut Engine, item: Item, start: DateTime) -> Result<u64, radio_engine::EngineError> {
    let out = e.dispatch(Command::ScheduleAdd {
        one_off: Box::new(item),
        start,
        fix: FixMode::Hard,
        tolerance: Duration::from_secs(1),
        repeat: Repeat::Once,
    })?;
    Ok(out
        .data
        .and_then(|d| d.as_u64())
        .expect("schedule.add returns the id"))
}

fn ljubljana(e: &mut Engine) {
    let mut station = e.state().station.clone();
    station.tz = "Europe/Ljubljana".into();
    e.dispatch(Command::StationImport { station }).unwrap();
}

#[test]
fn a_one_off_round_trips_through_persistence_and_its_script_matches_its_rules() {
    let path = temp_db("one-off");
    let id = {
        let mut e = Engine::open(&path).unwrap();
        let id = add(
            &mut e,
            file("Station ID"),
            date(2026, 11, 22).at(9, 0, 0, 0),
        )
        .unwrap();
        e.flush().unwrap();
        id
    };
    let mut e = Engine::open(&path).unwrap();
    let ones = e.state().one_offs();
    assert_eq!(ones.len(), 1);
    assert_eq!(ones[0].item.id, ItemId(id));
    assert_eq!(ones[0].start, date(2026, 11, 22).at(9, 0, 0, 0));
    let pl = e
        .state()
        .station
        .playlists
        .iter()
        .find(|p| p.role == PlaylistRole::OneOff)
        .expect("the One-off playlist exists once there is a one-off")
        .clone();
    let names: Vec<_> = e
        .state()
        .station
        .playlists
        .iter()
        .map(|p| (p.name.clone(), p.id))
        .collect();
    let script = e.state().script(pl.id).expect("the rules are a script too");
    let set = radio_script::eval_rules(script, &names, &radio_script::Limits::default()).unwrap();
    assert_eq!(
        set.rules, pl.rules,
        "the generated script holds the same rules"
    );

    e.dispatch(Command::ScheduleRemove { id: ItemId(id) })
        .unwrap();
    e.flush().unwrap();
    drop(e);
    let e = Engine::open(&path).unwrap();
    assert!(e.state().one_offs().is_empty());
    let pl = e
        .state()
        .station
        .playlists
        .iter()
        .find(|p| p.role == PlaylistRole::OneOff)
        .unwrap();
    assert!(pl.rules.is_empty(), "the rule goes with its item");
}

#[test]
fn a_start_in_a_dst_gap_is_rejected_and_changes_nothing() {
    let mut e = Engine::open_in_memory().unwrap();
    ljubljana(&mut e);
    let before = e.state().station.clone();
    let err = add(&mut e, file("Spring"), date(2027, 3, 28).at(2, 30, 0, 0)).unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
    assert_eq!(e.state().station, before);
}

#[test]
fn a_start_in_a_dst_fold_airs_at_the_first_pass_and_says_so() {
    let mut e = Engine::open_in_memory().unwrap();
    ljubljana(&mut e);
    let out = e
        .dispatch(Command::ScheduleAdd {
            one_off: Box::new(file("Autumn")),
            start: date(2026, 10, 25).at(2, 30, 0, 0),
            fix: FixMode::Soft,
            tolerance: Duration::from_secs(2),
            repeat: Repeat::Once,
        })
        .unwrap();
    assert!(out.note.is_some_and(|n| n.contains("twice")));
    assert_eq!(e.state().one_offs().len(), 1);
}

#[test]
fn undo_brings_back_a_removed_one_off_and_redo_removes_it_again() {
    let mut e = Engine::open_in_memory().unwrap();
    let id = add(&mut e, file("News"), date(2026, 11, 22).at(12, 0, 0, 0)).unwrap();
    e.dispatch(Command::ScheduleRemove { id: ItemId(id) })
        .unwrap();
    assert!(e.state().one_offs().is_empty());
    e.dispatch(Command::HistoryUndo).unwrap();
    assert_eq!(e.state().one_offs()[0].item.id, ItemId(id));
    e.dispatch(Command::HistoryRedo).unwrap();
    assert!(e.state().one_offs().is_empty());
}

#[test]
fn update_changes_the_time_and_keeps_the_id() {
    let mut e = Engine::open_in_memory().unwrap();
    let id = add(&mut e, file("Promo"), date(2026, 11, 22).at(12, 0, 0, 0)).unwrap();
    e.dispatch(Command::ScheduleUpdate {
        id: ItemId(id),
        one_off: Box::new(file("Promo, new cut")),
        start: date(2026, 11, 22).at(15, 30, 0, 0),
        fix: FixMode::Soft,
        tolerance: Duration::from_secs(3),
        repeat: Repeat::Daily,
    })
    .unwrap();
    let ones = e.state().one_offs();
    assert_eq!(ones.len(), 1);
    assert_eq!(ones[0].item.id, ItemId(id));
    assert_eq!(ones[0].item.title, "Promo, new cut");
    assert_eq!(ones[0].repeat, Repeat::Daily);
    assert!(ones[0].start > date(2026, 11, 22).at(12, 0, 0, 0));
}

#[test]
fn a_stream_without_a_length_and_an_unknown_id_are_rejected() {
    let mut e = Engine::open_in_memory().unwrap();
    let mut stream = file("Relay");
    stream.source = Source::Stream {
        url: "https://relay.invalid/live".into(),
        keep_alive: true,
    };
    stream.planned_len = None;
    assert!(add(&mut e, stream, date(2026, 11, 22).at(8, 0, 0, 0)).is_err());
    assert!(
        e.dispatch(Command::ScheduleRemove { id: ItemId(77) })
            .is_err()
    );
}

#[test]
fn the_one_off_playlist_cannot_be_deleted_renamed_or_switched_off() {
    let mut e = Engine::open_in_memory().unwrap();
    add(&mut e, file("ID"), date(2026, 11, 22).at(8, 0, 0, 0)).unwrap();
    let pl = e
        .state()
        .station
        .playlists
        .iter()
        .find(|p| p.role == PlaylistRole::OneOff)
        .unwrap()
        .id;
    for cmd in [
        Command::PlaylistDelete { playlist: pl },
        Command::PlaylistRename {
            playlist: pl,
            name: "Mine".into(),
        },
        Command::PlaylistEnable {
            playlist: pl,
            enabled: false,
        },
        Command::PlaylistSetMain { playlist: pl },
        Command::PlaylistDuplicate {
            playlist: pl,
            name: "Copy".into(),
        },
    ] {
        let id = cmd.id();
        assert!(e.dispatch(cmd).is_err(), "{id} on One-off is refused");
    }
    assert_eq!(e.state().one_offs().len(), 1);
}

#[test]
fn a_second_one_off_reuses_the_playlist() {
    let mut e = Engine::open_in_memory().unwrap();
    let a = add(&mut e, file("A"), date(2026, 11, 22).at(8, 0, 0, 0)).unwrap();
    let b = add(&mut e, file("B"), date(2026, 11, 22).at(9, 0, 0, 0)).unwrap();
    assert_ne!(a, b);
    let count = e
        .state()
        .station
        .playlists
        .iter()
        .filter(|p| p.role == PlaylistRole::OneOff)
        .count();
    assert_eq!(count, 1);
    assert_eq!(e.state().one_offs().len(), 2);
}
