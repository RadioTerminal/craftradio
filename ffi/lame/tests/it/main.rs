//! Tests of the LAME bindings against the installed libmp3lame.

use radio_lame::{Encoder, Error, Settings};

const STEREO_48K: Settings = Settings {
    channels: 2,
    sample_rate: 48_000,
    kbps: 128,
};

/// One second of a 440 Hz tone, interleaved stereo at 48 kHz.
fn tone() -> Vec<f32> {
    (0..48_000)
        .flat_map(|i| {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            [s, s]
        })
        .collect()
}

#[test]
fn a_tone_encodes_to_mpeg1_layer3_frames_at_the_chosen_bitrate() {
    let mut encoder = Encoder::new(STEREO_48K).expect("encoder");
    let mut mp3 = encoder.encode(&tone()).expect("encode");
    mp3.extend(encoder.flush().expect("flush"));
    let header = mp3
        .windows(4)
        .find(|w| w[0] == 0xFF && w[1] & 0xE0 == 0xE0)
        .expect("a frame");
    // MPEG-1 (0b11), Layer III (0b01).
    assert_eq!((header[1] >> 3) & 0b11, 0b11, "MPEG version");
    assert_eq!((header[1] >> 1) & 0b11, 0b01, "layer");
    // In MPEG-1 Layer III, bitrate index 9 is 128 kbit/s, and sample-rate index 1 is 48 kHz.
    assert_eq!(header[2] >> 4, 9, "bitrate index");
    assert_eq!((header[2] >> 2) & 0b11, 1, "sample-rate index");
    // One second at 128 kbit/s is about 16000 bytes.
    assert!((14_000..18_000).contains(&mp3.len()), "{} bytes", mp3.len());
}

#[test]
fn samples_that_do_not_fill_every_channel_are_refused() {
    let mut encoder = Encoder::new(STEREO_48K).expect("encoder");
    assert_eq!(encoder.encode(&[0.0; 3]), Err(Error::PartialFrame));
}

#[test]
fn an_encoder_moves_to_another_thread() {
    let mut encoder = Encoder::new(STEREO_48K).expect("encoder");
    let bytes = std::thread::spawn(move || encoder.encode(&tone()).expect("encode"))
        .join()
        .expect("thread");
    assert!(!bytes.is_empty());
}
