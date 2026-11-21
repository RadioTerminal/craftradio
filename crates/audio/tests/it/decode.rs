use std::time::Duration;

use radio_audio::{AudioError, Decoder, SAMPLE_RATE, probe};

use crate::fixtures::{header_wav, tone_wav};

fn assert_near(frames: usize, expected: usize, what: &str) {
    let diff = frames.abs_diff(expected);
    assert!(
        diff <= expected / 200,
        "{what}: got {frames} frames, expected about {expected}"
    );
}

/// `probe` gives the rate of a 44.1 kHz file. The decoder gives the same length at
/// 48 kHz, and a cue point removes the right number of frames.
#[test]
fn a_44k1_file_decodes_to_the_same_length_at_48k() {
    let path = tone_wav("resample", 44_100, 2.0, 440.0, -6.0);

    let info = probe(&path).expect("probe");
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert!(
        (info.len.as_secs_f64() - 2.0).abs() < 0.01,
        "len {:?}",
        info.len
    );

    let whole = Decoder::open(&path)
        .expect("open")
        .read_to_end()
        .expect("decode");
    let rate = SAMPLE_RATE as usize;
    assert_near(whole.len() / 2, 2 * rate, "whole file");

    let cue = Duration::from_millis(700);
    let cued = Decoder::open_at(&path, cue)
        .expect("open at cue")
        .read_to_end()
        .expect("decode");
    assert_near(cued.len() / 2, 13 * rate / 10, "from the cue point");
}

/// A file with a forged rate or channel count fails as unsupported before the decoder
/// sizes a buffer.
#[test]
fn a_forged_header_is_rejected() {
    let forged = [
        header_wav("forged-rate", 10_000_000, 2),
        header_wav("forged-channels", 48_000, 9),
    ];
    for path in &forged {
        let probed = probe(path);
        assert!(
            matches!(probed, Err(AudioError::Unsupported { .. })),
            "{path:?}: {probed:?}"
        );
        assert!(matches!(
            Decoder::open(path),
            Err(AudioError::Unsupported { .. })
        ));
    }
}

/// Opus imports must have a playback decoder, even without an external encoder.
#[test]
fn opus_import_has_a_playback_decoder() {
    use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
    assert!(radio_model::AUDIO_EXTENSIONS.contains(&"opus"));
    assert!(
        radio_audio::codecs()
            .get_audio_decoder(CODEC_ID_OPUS)
            .is_some()
    );
}
