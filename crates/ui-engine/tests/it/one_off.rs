//! One-offs of Upcoming through the real engine: the dialog edit, the row and its removal.

use crate::fixtures::*;
use jiff::civil::date;
use radio_ui_egui::view::{Edit, OneOffRepeat, OneOffSource, OneOffView, ViewSource};

fn file_one_off(path: String, minute: i8) -> OneOffView {
    OneOffView {
        id: 0,
        source: OneOffSource::File { path },
        start: date(2026, 11, 21).at(13, minute, 0, 0),
        hard: false,
        tolerance_s: 2.0,
        repeat: OneOffRepeat::Once,
        picture: String::new(),
    }
}

#[test]
fn a_one_off_file_shows_in_upcoming_without_a_tab_and_remove_takes_it_out() {
    let d = dir();
    let songs = (1..=4)
        .map(|i| {
            track(
                i,
                &format!("Song {i}"),
                &wav(&d, &format!("s{i}"), 60.0),
                60.0,
            )
        })
        .collect();
    let (mut source, clock) = source(songs);
    let _ = wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    let promo = wav(&d, "promo", 3.0);
    assert_eq!(
        source.edit(Edit::ScheduleOneOff(file_one_off(promo, 3))),
        Ok(1)
    );
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| r.one_off.is_some())
    });
    let row = view
        .rows
        .iter()
        .find(|r| r.one_off.is_some())
        .expect("the one-off is a row of the timeline");
    assert_eq!(row.title, "promo");
    assert!(row.fixed.is_some(), "it is a fixed row with a fit");
    let target = date(2026, 11, 21).at(13, 3, 0, 0);
    let off = row.start.datetime().duration_since(target).abs();
    assert!(
        off <= jiff::SignedDuration::from_secs(2),
        "starts {off} from its time"
    );
    assert!(
        view.tabs.iter().all(|t| t.name != "One-off"),
        "the system playlist has no tab"
    );
    assert!(
        view.library.iter().all(|l| l.category != "One-off"),
        "a one-off is no library file"
    );
    let o = row.one_off.clone().unwrap();
    assert_eq!(o.tolerance_s, 2.0);
    assert!(!o.hard);

    assert_eq!(source.edit(Edit::Remove(vec![row.id])), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.iter().all(|r| r.one_off.is_none())
    });
    assert!(view.rows.iter().all(|r| r.one_off.is_none()));
    assert_eq!(source.edit(Edit::Undo), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| r.one_off.is_some())
    });
    assert!(
        view.rows.iter().any(|r| r.one_off.is_some()),
        "Undo brings it back"
    );
}

#[test]
fn a_url_one_off_without_a_title_is_titled_by_its_host() {
    let (mut source, clock) = source(Vec::new());
    let _ = wait_for(&mut source, &clock, |_| true);
    // Past the stream pre-roll, so playout opens no connection while the clock stands still.
    let mut o = file_one_off(String::new(), 3);
    o.source = OneOffSource::Url {
        url: "https://relay.example.org/live".into(),
        title: String::new(),
        keep_alive: true,
        len: std::time::Duration::from_secs(60),
    };
    assert_eq!(source.edit(Edit::ScheduleOneOff(o)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| r.one_off.is_some())
    });
    let row = view.rows.iter().find(|r| r.one_off.is_some()).unwrap();
    assert_eq!(row.title, "relay.example.org");
}

/// The picture of a one-off reaches its item, and an edit shows it again.
#[test]
fn a_one_off_picture_is_kept_and_comes_back_for_an_edit() {
    let (mut source, clock) = source(Vec::new());
    let _ = wait_for(&mut source, &clock, |_| true);
    let mut o = file_one_off(String::new(), 3);
    o.source = OneOffSource::Url {
        url: "https://relay.example.org/live".into(),
        title: "Relay".into(),
        keep_alive: true,
        len: std::time::Duration::from_secs(60),
    };
    o.picture = "/art/relay.png".into();
    assert_eq!(source.edit(Edit::ScheduleOneOff(o)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| r.one_off.is_some())
    });
    let row = view.rows.iter().find(|r| r.one_off.is_some()).unwrap();
    let back = row.one_off.as_ref().unwrap();
    assert_eq!(back.picture, "/art/relay.png");
    assert_eq!(row.meta.picture, "/art/relay.png", "its tags carry it too");
}

/// A one-off of a library item starts with the item's picture. Clearing it for the one-off
/// leaves the library item as it was.
#[test]
fn a_cleared_one_off_picture_stays_cleared_and_the_library_item_keeps_its_own() {
    let d = dir();
    let mut song = track(1, "Song", &wav(&d, "s1", 60.0), 60.0);
    song.meta.picture = "/art/song.png".into();
    let (mut source, clock) = source(vec![song]);
    let view = wait_for(&mut source, &clock, |v| !v.library.is_empty());
    let lib = view.library[0].clone();
    assert_eq!(lib.meta.picture, "/art/song.png");
    let mut o = file_one_off(String::new(), 3);
    o.source = OneOffSource::Library {
        item: lib.id,
        title: lib.title.clone(),
    };
    o.picture = lib.meta.picture.clone();
    assert_eq!(source.edit(Edit::ScheduleOneOff(o.clone())), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows.iter().any(|r| r.one_off.is_some())
    });
    let row = view.rows.iter().find(|r| r.one_off.is_some()).unwrap();
    assert_eq!(row.meta.picture, "/art/song.png");
    let mut edited = row.one_off.clone().unwrap();
    edited.picture.clear();
    assert_eq!(source.edit(Edit::ScheduleOneOff(edited)), Ok(1));
    let view = wait_for(&mut source, &clock, |v| {
        v.rows
            .iter()
            .any(|r| r.one_off.as_ref().is_some_and(|o| o.picture.is_empty()))
    });
    let row = view.rows.iter().find(|r| r.one_off.is_some()).unwrap();
    assert!(row.meta.picture.is_empty(), "the one-off has no picture");
    let lib = view.library.iter().find(|l| l.id == lib.id).unwrap();
    assert_eq!(
        lib.meta.picture, "/art/song.png",
        "the library item keeps its own"
    );
}
