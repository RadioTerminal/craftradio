use crate::framing::OggSource;
use craft_audio::mp3::{Config, Encoder};
use radio_server::{DecodeError, Decoded, Format, ListenerCap, Mount, SourceDecoder, SourceGuard};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long a decoder may take to see something that should happen at once.
const PATIENCE: Duration = Duration::from_secs(5);
/// Packets in each Ogg chain: three chains stay well inside a tap's queue.
const PACKETS: usize = 60;
/// Interleaved stereo samples one 20 ms Opus packet decodes to.
const PACKET_SAMPLES: usize = 1920;

/// Writes `bytes` to the mount in 4 KiB pieces from a thread, then ends the source.
fn feed(mut guard: SourceGuard, bytes: Vec<u8>) -> JoinHandle<()> {
    thread::spawn(move || {
        for piece in bytes.chunks(4096) {
            guard.write(piece);
        }
    })
}

/// Decodes until the decoder says the stream is over, within [`PATIENCE`].
pub(crate) fn decode_all(decoder: &mut SourceDecoder) -> (Vec<f32>, Decoded) {
    let deadline = Instant::now() + PATIENCE;
    let mut samples = Vec::new();
    while Instant::now() < deadline {
        match decoder.next(&mut samples).expect("the stream decodes") {
            Decoded::Audio => {}
            other => return (samples, other),
        }
    }
    panic!("the decoder did not reach the end within {PATIENCE:?}");
}

pub(crate) fn heard(samples: &[f32]) -> bool {
    samples.iter().any(|sample| sample.abs() > 0.01)
}

/// `ms` milliseconds of an MP3 tone at `rate`, stereo, 128 kbit/s.
fn mp3_tone(rate: u32, ms: usize) -> Vec<u8> {
    let mut encoder = Encoder::new(Config::cbr(rate, 2, 128)).unwrap();
    let frames = rate as usize * ms / 1000;
    let tone: Vec<f32> = (0..frames)
        .map(|i| (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.2)
        .collect();
    let mut bytes = encoder.encode(&[&tone, &tone]);
    bytes.extend(encoder.flush());
    bytes
}

pub(crate) fn stop() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[test]
fn opus_is_decoded_across_title_changes() {
    let mount = Mount::new(ListenerCap::new(10));
    let guard = mount
        .begin(Format::Ogg, "Morning Station".into(), None)
        .unwrap();
    let tap = mount.tap().unwrap();
    let mut source = OggSource::new("Cloud Signal", Some("Echo"));
    source.audio(PACKETS * 20);
    source.restart("Paper Harbour", None);
    source.audio(PACKETS * 20);
    source.restart("Dawn", None);
    source.audio(PACKETS * 20);
    let feeder = feed(guard, source.take());

    let mut decoder = SourceDecoder::open(tap, stop()).expect("the stream opens");
    let (samples, end) = decode_all(&mut decoder);
    feeder.join().unwrap();
    assert!(matches!(end, Decoded::Ended));
    let chain = PACKETS * PACKET_SAMPLES;
    assert!(
        samples.len() > chain * 5 / 2,
        "the third chain was decoded: {} samples, a chain holds {chain}",
        samples.len()
    );
    // Each chain the app ends also carries the closing packet `restart` writes, so the
    // third chain's audio surely starts no earlier than this.
    let third = 2 * (PACKETS + 1) * PACKET_SAMPLES;
    assert!(
        heard(samples.get(third..).unwrap_or_default()),
        "the audio after the second change is not silent"
    );
}

#[test]
fn mp3_is_decoded() {
    let mount = Mount::new(ListenerCap::new(10));
    let guard = mount
        .begin(Format::Mp3, "Morning Station".into(), None)
        .unwrap();
    let tap = mount.tap().unwrap();
    let feeder = feed(guard, mp3_tone(48_000, 1000));

    let mut decoder = SourceDecoder::open(tap, stop()).expect("the stream opens");
    let (samples, end) = decode_all(&mut decoder);
    feeder.join().unwrap();
    assert!(matches!(end, Decoded::Ended));
    assert!(heard(&samples), "the MP3 tone comes out");
}

#[test]
fn a_44_1_khz_source_is_refused() {
    let mount = Mount::new(ListenerCap::new(10));
    let guard = mount
        .begin(Format::Mp3, "Morning Station".into(), None)
        .unwrap();
    let tap = mount.tap().unwrap();
    let feeder = feed(guard, mp3_tone(44_100, 1000));

    let refused = match SourceDecoder::open(tap, stop()) {
        Err(error) => error,
        Ok(mut decoder) => {
            let mut samples = Vec::new();
            match decoder.next(&mut samples) {
                Err(error) => error,
                Ok(decoded) => panic!("the first packet is refused, not {decoded:?}"),
            }
        }
    };
    feeder.join().unwrap();
    assert!(
        matches!(refused, DecodeError::Unsupported { rate: 44_100, .. }),
        "refused for its rate: {refused:?}"
    );
}

/// A mount with a tap whose decoder has opened on the first chain's start.
fn opened() -> (Arc<Mount>, SourceGuard, OggSource, SourceDecoder) {
    let mount = Mount::new(ListenerCap::new(10));
    let mut guard = mount
        .begin(Format::Ogg, "Morning Station".into(), None)
        .unwrap();
    let tap = mount.tap().unwrap();
    let mut source = OggSource::new("Cloud Signal", None);
    source.audio(200);
    for piece in source.take().chunks(4096) {
        guard.write(piece);
    }
    let decoder = SourceDecoder::open(tap, stop()).expect("the stream opens");
    (mount, guard, source, decoder)
}

#[test]
fn an_overrun_is_told_apart_from_the_end() {
    let (_mount, mut guard, mut source, mut decoder) = opened();
    // More pages than a tap's queue holds, with nobody reading.
    source.audio(300 * 20);
    for piece in source.take().chunks(4096) {
        guard.write(piece);
    }
    let (_, end) = decode_all(&mut decoder);
    assert!(matches!(end, Decoded::Overrun), "an overrun, not the end");

    let (_mount, guard, _source, mut decoder) = opened();
    drop(guard);
    let (_, end) = decode_all(&mut decoder);
    assert!(matches!(end, Decoded::Ended), "the end, not an overrun");
}

#[test]
fn stop_ends_a_decoder_waiting_on_a_silent_source() {
    let mount = Mount::new(ListenerCap::new(10));
    let _guard = mount
        .begin(Format::Ogg, "Morning Station".into(), None)
        .unwrap();
    let tap = mount.tap().unwrap();
    let flag = stop();
    let (done, opened) = mpsc::channel();
    let shared = Arc::clone(&flag);
    thread::spawn(move || {
        let _ = done.send(SourceDecoder::open(tap, shared).is_ok());
    });
    // Gives `open` time to start waiting; what the test proves is the deadline below.
    thread::sleep(Duration::from_millis(300));
    flag.store(true, Ordering::Release);
    let opened = opened
        .recv_timeout(Duration::from_secs(2))
        .expect("open returned once stop was set");
    assert!(!opened, "a source that sent nothing did not open");
}
