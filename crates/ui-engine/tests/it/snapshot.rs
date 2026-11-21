use crate::fixtures::*;
use radio_ui_egui::view::{PublishMode, Status, ViewSource};

/// Without an Icecast mount the station is never live, whatever the local output does.
#[test]
fn the_output_is_not_live_without_icecast() {
    let d = dir();
    let song = track(1, "Song", &wav(&d, "song", 3.0), 3.0);
    let (mut source, clock) = source(vec![song]);
    let view = wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    assert!(!view.rows.is_empty(), "the song is in the timeline");
    assert_ne!(view.output.icecast, Status::Connected);
    assert_ne!(
        view.output.local,
        Status::Preview,
        "a null output is real playout, not the preview"
    );
}

/// A custom stream title fills its placeholders from the tags of the row.
#[test]
fn a_custom_stream_title_fills_its_placeholders() {
    let d = dir();
    let mut song = track(1, "Lanterns", &wav(&d, "lanterns", 3.0), 3.0);
    song.meta = radio_model::ItemMeta {
        title: "Lanterns".into(),
        artist: "Mira Solenne".into(),
        genre: "Folk".into(),
        publish: radio_model::PublishMode::Custom,
        custom: "{artist} – {title} [{genre}] on air".into(),
        ..Default::default()
    };
    let (mut source, clock) = source(vec![song]);
    let view = wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    let row = view.rows.first().expect("a row");
    assert_eq!(row.meta.publish, PublishMode::Custom);
    let title = source.stream_title(row);
    assert_eq!(title, "Mira Solenne – Lanterns [Folk] on air");
}

/// A cart keeps the duck depth and colour that a person set, and the edit says that the
/// cart player of this build does not use the duck yet.
#[test]
fn cart_settings_come_back_in_the_snapshot() {
    use radio_ui_egui::view::{CartChange, Edit};
    let d = dir();
    let song = track(1, "Song", &wav(&d, "song", 3.0), 3.0);
    let (mut source, clock) = source(vec![song]);
    let view = wait_for(&mut source, &clock, |v| !v.library.is_empty());
    let item = view.library[0].id;
    assert_eq!(
        source.edit(Edit::AssignCart {
            cart: 0,
            item: Some(item)
        }),
        Ok(1)
    );
    let colour = source.edit(Edit::SetCart {
        cart: 0,
        change: CartChange::Colour(Some(2)),
    });
    assert_eq!(colour, Ok(1));
    let _ = source.edit(Edit::SetCart {
        cart: 0,
        change: CartChange::Duck(-9.0),
    });
    let view = wait_for(&mut source, &clock, |v| v.cartwall.slots[0].duck_db < -1.0);
    assert_eq!(view.cartwall.slots[0].colour, Some(2));
    assert!((view.cartwall.slots[0].duck_db + 9.0).abs() < 0.01);
    assert_eq!(view.carts[0], "Song");
}

#[test]
fn failed_checkpoints_show_a_persistent_warning() {
    use radio_engine::{Command, Engine};
    use radio_model::PlaylistId;
    use radio_playout::{ManualClock, OutputChoice, Playout, PlayoutConfig};
    use radio_ui_egui::ViewSource;
    use radio_ui_engine::{EngineSource, SourceOptions};
    use std::sync::Arc;
    let dir = super::fixtures::dir();
    let path = dir.join("station.db");
    let mut engine = Engine::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON playlists BEGIN SELECT RAISE(FAIL, 'write denied'); END;").unwrap();
    engine
        .dispatch(Command::PlaylistRename {
            playlist: PlaylistId(1),
            name: "Unsaved playlist".into(),
        })
        .unwrap();
    assert!(engine.flush().is_err());
    let clock = ManualClock::new(super::fixtures::t0());
    let playout = Playout::start(
        engine,
        PlayoutConfig::new(Arc::new(clock), OutputChoice::Null),
    )
    .unwrap();
    let mut source = EngineSource::new(
        playout,
        SourceOptions {
            tz: jiff::tz::TimeZone::UTC,
            station_name: "Test Radio".into(),
        },
    );
    let view = source.snapshot(super::fixtures::t0());
    assert!(view.warnings > 0);
    assert!(view.mode.contains("NOT SAVED"));
    assert!(view.mode.contains("write denied"));
}
