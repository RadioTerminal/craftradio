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
    assert!(view.open_alarms().count() > 0);
    assert!(
        view.open_alarms()
            .any(|a| a.text.contains("not saved") && a.text.contains("write denied")),
        "the Alarms window names the failed save"
    );
    assert!(view.mode.contains("NOT SAVED"));
    assert!(view.mode.contains("write denied"));
}

/// A row that hides its title shows the station name from Preferences, not the name the
/// source started with.
#[test]
fn a_hidden_title_shows_the_branding_name_in_the_view() {
    let d = dir();
    let mut song = track(1, "Quiet", &wav(&d, "quiet", 3.0), 3.0);
    song.meta = radio_model::ItemMeta {
        title: "Quiet".into(),
        publish: radio_model::PublishMode::Hide,
        ..Default::default()
    };
    let (mut source, clock) = source(vec![song]);
    wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    source
        .playout()
        .dispatch(radio_engine::Command::SettingsSet {
            part: radio_model::BRANDING_PART.into(),
            key: radio_model::BRANDING_NAME_KEY.into(),
            value: "Radio Lipa".into(),
        })
        .unwrap();
    let view = wait_for(&mut source, &clock, |v| v.station == "Radio Lipa");
    let row = view.rows.first().expect("a row");
    assert_eq!(source.stream_title(row), "Radio Lipa");
}

/// A logo that does not load shows why, a logo that loads shows its picture, and no logo
/// shows nothing.
#[test]
fn a_logo_that_does_not_load_shows_why() {
    use radio_ui_egui::view::Edit;
    // A 1 by 1 PNG.
    const PNG: [u8; 70] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64,
        0x60, 0xf8, 0x5f, 0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let d = dir();
    let (mut source, clock) = source(Vec::new());
    let view = wait_for(&mut source, &clock, |_| true);
    assert!(view.branding_logo.is_none(), "no logo is set");
    let set = |source: &mut radio_ui_engine::EngineSource, path: &std::path::Path| {
        let edit = Edit::SetPartSetting {
            part: radio_model::BRANDING_PART.into(),
            key: radio_model::BRANDING_LOGO_KEY.into(),
            value: path.to_string_lossy().into_owned().into(),
        };
        assert_eq!(source.edit(edit), Ok(1));
    };
    let text = d.join("logo.png");
    std::fs::write(&text, b"this is not a picture").unwrap();
    set(&mut source, &text);
    // Until the lane answers, nothing shows: no picture and no error.
    let view = wait_for(&mut source, &clock, |v| v.branding_logo.is_some());
    assert!(
        matches!(&view.branding_logo, Some(Err(why)) if !why.is_empty()),
        "a text file is no picture: {:?}",
        view.branding_logo
    );
    let real = d.join("real.png");
    std::fs::write(&real, PNG).unwrap();
    set(&mut source, &real);
    let view = wait_for(
        &mut source,
        &clock,
        |v| matches!(&v.branding_logo, Some(Ok(c)) if c.uri.contains("real.png")),
    );
    let Some(Ok(cover)) = &view.branding_logo else {
        panic!("a real PNG loads: {:?}", view.branding_logo);
    };
    assert!(!cover.bytes.is_empty());
    // Back to the text file: once the picture of the other path has left, it never returns,
    // and what settles is the error of the new path.
    set(&mut source, &text);
    let left = std::cell::Cell::new(false);
    let view = wait_for(&mut source, &clock, |v| {
        let old = matches!(&v.branding_logo, Some(Ok(c)) if c.uri.contains("real.png"));
        assert!(!(left.get() && old), "the old path's picture came back");
        left.set(left.get() || !old);
        matches!(v.branding_logo, Some(Err(_)))
    });
    assert!(matches!(view.branding_logo, Some(Err(_))));
    set(&mut source, std::path::Path::new(""));
    let view = wait_for(&mut source, &clock, |v| v.branding_logo.is_none());
    assert!(view.branding_logo.is_none(), "clearing the logo shows none");
}
