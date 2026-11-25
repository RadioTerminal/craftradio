use radio_model::{MarkerSource, PlayMode, Playlist, Timing};
use std::time::Duration;

#[test]
fn old_playlist_loads_with_defaults() {
    let mut p = radio_testkit::playlist(1, "Jingles", vec![radio_testkit::jingle(2, 5)]);
    p.items[0].timing = Some(Timing {
        cue_in: Duration::ZERO,
        segue: Duration::from_secs(4),
        cue_out: Duration::from_secs(5),
        intro_end: None,
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
        outro_start: None,
        markers: MarkerSource::Detected,
    });
    let mut text = toml::to_string(&p).expect("serialize");
    // A file written before this change has none of the new keys.
    text = text
        .lines()
        .filter(|l| {
            !l.starts_with("plays") && !l.starts_with("outro_start") && !l.starts_with("markers")
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("plays") && !text.contains("markers"));
    let back: Playlist = toml::from_str(&text).expect("old file loads");
    assert_eq!(back.plays, PlayMode::InSequence);
    let timing = back.items[0].timing.as_ref().expect("timing kept");
    assert_eq!(timing.outro_start, None);
    assert_eq!(timing.markers, MarkerSource::NotAnalysed);
}

#[test]
fn play_mode_round_trips() {
    let mut p = radio_testkit::playlist(1, "Jingles", vec![]);
    p.plays = PlayMode::Over { duck_db: -9 };
    let back: Playlist = toml::from_str(&toml::to_string(&p).expect("ser")).expect("de");
    assert_eq!(back.plays, PlayMode::Over { duck_db: -9 });
}
