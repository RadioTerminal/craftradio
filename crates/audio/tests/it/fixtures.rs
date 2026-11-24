//! Audio fixtures that the tests make at run time, and level helpers.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use radio_audio::{DeckId, DeckState, MixerHandle};

/// Writes a stereo float WAV with a cosine tone into the temp dir. Its first sample has
/// full amplitude.
pub fn tone_wav(name: &str, rate: u32, secs: f64, freq: f64, dbfs: f64) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "radio-audio-{}-{}-{name}.wav",
        std::process::id(),
        rate
    ));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create fixture wav");
    let amp = 10f64.powf(dbfs / 20.0);
    let frames = (secs * f64::from(rate)) as usize;
    for i in 0..frames {
        let t = i as f64 / f64::from(rate);
        let s = (amp * (2.0 * std::f64::consts::PI * freq * t).cos()) as f32;
        writer.write_sample(s).expect("write");
        writer.write_sample(s).expect("write");
    }
    writer.finalize().expect("finalize fixture wav");
    path
}

/// Writes a short silent 16-bit WAV with any rate and channel count, for forged headers.
pub fn header_wav(name: &str, rate: u32, channels: u16) -> PathBuf {
    let path = std::env::temp_dir().join(format!("radio-audio-{}-{name}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create fixture wav");
    for _ in 0..usize::from(channels) * 64 {
        writer.write_sample(0i16).expect("write");
    }
    writer.finalize().expect("finalize fixture wav");
    path
}

/// Returns stereo interleaved cosine samples at 48 kHz, for cart clips in memory.
pub fn tone_samples(secs: f64, freq: f64, dbfs: f64) -> Vec<f32> {
    let amp = 10f64.powf(dbfs / 20.0);
    let frames = (secs * 48_000.0) as usize;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f64 / 48_000.0;
        let s = (amp * (2.0 * std::f64::consts::PI * freq * t).cos()) as f32;
        out.push(s);
        out.push(s);
    }
    out
}

/// Returns the peak level of interleaved samples in dBFS.
pub fn peak_db(samples: &[f32]) -> f32 {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    20.0 * peak.max(1e-12).log10()
}

/// Returns the RMS level of interleaved samples in dBFS.
pub fn rms_db(samples: &[f32]) -> f32 {
    let ms = samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64;
    (10.0 * ms.max(1e-24).log10()) as f32
}

/// Returns the interleaved stereo samples of frames `from..to`.
pub fn frames(out: &[f32], from: usize, to: usize) -> &[f32] {
    &out[from * 2..to * 2]
}

/// Waits until the feeder of the deck has enough audio to play.
pub fn wait_ready(handle: &MixerHandle, deck: DeckId) {
    let start = Instant::now();
    while handle.deck_state(deck) != DeckState::Ready {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "deck {deck:?} never became ready (state {:?})",
            handle.deck_state(deck)
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A file in memory whose reads fail once they pass `fail_at` bytes: a disk that goes
/// away in the middle of a file.
pub struct FailingFile {
    bytes: std::io::Cursor<Vec<u8>>,
    fail_at: u64,
}

impl FailingFile {
    pub fn new(path: &std::path::Path, fail_at: u64) -> Self {
        let bytes = std::fs::read(path).expect("read fixture");
        Self {
            bytes: std::io::Cursor::new(bytes),
            fail_at,
        }
    }
}

impl std::io::Read for FailingFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // A probe looks at the last bytes of a file for tags, so those still read.
        let tail = (self.bytes.get_ref().len() as u64).saturating_sub(64 * 1024);
        if (self.fail_at..tail).contains(&self.bytes.position()) {
            return Err(std::io::Error::other("the disk went away"));
        }
        self.bytes.read(buf)
    }
}

impl std::io::Seek for FailingFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.bytes.seek(pos)
    }
}
