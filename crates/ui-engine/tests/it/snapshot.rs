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
