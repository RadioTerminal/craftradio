use crate::decode::{decode_all, heard, stop};
use crate::framing::is_bos;
use radio_codecs::encode::EncodeError;
use radio_server::{
    BLOCK, Decoded, Listener, ListenerCap, Mount, OutputEncoder, OutputFormat, Received,
    SourceDecoder,
};
use std::sync::Arc;
use std::time::Duration;

/// Blocks in one second.
const SECOND: usize = 50;

/// Block `n` of a test signal: two tones and a little noise, the same on every run.
fn block(n: usize) -> Vec<f32> {
    let mut noise = (n as u32).wrapping_mul(2_654_435_761) | 1;
    (0..BLOCK / 2)
        .flat_map(|i| {
            let t = (n * BLOCK / 2 + i) as f32 / 48_000.0;
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let hiss = (noise as f32 / u32::MAX as f32 - 0.5) * 0.05;
            let left = (t * 440.0 * std::f32::consts::TAU).sin() * 0.2 + hiss;
            let right = (t * 660.0 * std::f32::consts::TAU).sin() * 0.2 - hiss;
            [left, right]
        })
        .collect()
}

/// An output mount with an encoder writing to it.
fn output(format: OutputFormat, kbps: u32) -> (Arc<Mount>, OutputEncoder) {
    let mount = Mount::new(ListenerCap::new(10));
    let guard = mount
        .begin(format.into(), "Morning Station".into(), Some(kbps))
        .unwrap();
    let encoder = OutputEncoder::new(format, kbps, guard).unwrap();
    (mount, encoder)
}

/// Moves what `listener` has queued into `chunks`, without waiting.
fn drain(listener: &Listener, chunks: &mut Vec<Vec<u8>>) {
    loop {
        match listener.recv(Duration::ZERO) {
            Received::Chunk(chunk) => chunks.push(chunk.to_vec()),
            Received::Timeout => return,
            Received::Ended => panic!("the listener was ended"),
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Encodes a second of the test signal and decodes it back from a tap on the mount.
fn round_trip(format: OutputFormat, title: &str, artist: Option<&str>) -> (Vec<f32>, Decoded) {
    let (mount, mut encoder) = output(format, 128);
    let tap = mount.tap().unwrap();
    encoder.start(title, artist).unwrap();
    for n in 0..SECOND {
        encoder.encode(&block(n)).unwrap();
    }
    let stats = mount.stats();
    assert_eq!(
        stats.kbps,
        Some(128),
        "the mount keeps the bitrate it was given"
    );
    if format == OutputFormat::Opus {
        assert_eq!(stats.title, title);
        assert_eq!(stats.artist, artist.unwrap_or_default());
    }
    // Ends the source, so the decoder reaches the end.
    drop(encoder);
    let mut decoder = SourceDecoder::open(tap, stop()).expect("the output opens");
    decode_all(&mut decoder)
}

#[test]
fn an_opus_output_plays_and_carries_its_title() {
    let (samples, end) = round_trip(OutputFormat::Opus, "Cloud Signal", Some("Echo"));
    assert!(matches!(end, Decoded::Ended));
    assert!(heard(&samples), "the tone comes out");
}

#[test]
fn an_mp3_output_plays() {
    let (samples, end) = round_trip(OutputFormat::Mp3, "Cloud Signal", None);
    assert!(matches!(end, Decoded::Ended));
    assert!(heard(&samples), "the tone comes out");
}

/// Bytes a listener gets for five seconds at `kbps`.
fn five_seconds(format: OutputFormat, kbps: u32) -> usize {
    let (mount, mut encoder) = output(format, kbps);
    let listener = mount.listen().unwrap();
    encoder.start("Cloud Signal", None).unwrap();
    let mut chunks = Vec::new();
    for n in 0..5 * SECOND {
        encoder.encode(&block(n)).unwrap();
        drain(&listener, &mut chunks);
    }
    chunks.iter().map(Vec::len).sum()
}

#[test]
fn a_higher_bitrate_makes_more_bytes() {
    for format in [OutputFormat::Opus, OutputFormat::Mp3] {
        let low = five_seconds(format, 64);
        let high = five_seconds(format, 192);
        assert!(high > low, "{format:?}: {high} bytes at 192, {low} at 64");
        for (kbps, bytes) in [(64, low), (192, high)] {
            let expected = kbps as usize * 1000 * 5 / 8;
            assert!(
                bytes.abs_diff(expected) <= expected / 4,
                "{format:?} at {kbps} kbit/s: {bytes} bytes, about {expected} expected"
            );
        }
    }
}

/// An Opus output that changed from one title to another, still live.
struct Changed {
    mount: Arc<Mount>,
    _live: OutputEncoder,
    /// What a listener who joined after the change got.
    late: Vec<Vec<u8>>,
    /// What a listener who was there from the start got.
    early: Vec<Vec<u8>>,
}

fn across_a_change(
    change: fn(&mut OutputEncoder, &str, Option<&str>) -> Result<(), EncodeError>,
) -> Changed {
    let (mount, mut encoder) = output(OutputFormat::Opus, 96);
    let early = mount.listen().unwrap();
    encoder.start("Cloud Signal", Some("Echo")).unwrap();
    for n in 0..10 {
        encoder.encode(&block(n)).unwrap();
    }
    change(&mut encoder, "Paper Harbour", None).unwrap();
    for n in 10..15 {
        encoder.encode(&block(n)).unwrap();
    }
    let late = mount.listen().unwrap();
    for n in 15..20 {
        encoder.encode(&block(n)).unwrap();
    }
    let (mut late_chunks, mut early_chunks) = (Vec::new(), Vec::new());
    drain(&late, &mut late_chunks);
    drain(&early, &mut early_chunks);
    Changed {
        mount,
        _live: encoder,
        late: late_chunks,
        early: early_chunks,
    }
}

/// A late listener starts at the new chain, and the old one was closed before it.
fn starts_at_the_new_chain(changed: Changed) {
    let Changed {
        mount, late, early, ..
    } = changed;
    assert!(is_bos(&late[0]), "the first chunk begins a stream");
    assert!(contains(&late[0], b"OpusHead"));
    assert!(contains(&late[1], b"TITLE=Paper Harbour"), "the new tags");
    assert!(!late.iter().any(|c| contains(c, b"Cloud Signal")));
    assert_eq!(mount.stats().title, "Paper Harbour");
    assert_eq!(mount.stats().artist, "");

    // A listener who was there all along hears the first chain closed before the second.
    let second = early.iter().rposition(|c| is_bos(c)).unwrap();
    assert!(second > 0);
    let end_of_stream = early[second - 1][5] & 0x04 != 0;
    assert!(
        end_of_stream,
        "the first chain ends before the second begins"
    );
}

#[test]
fn a_listener_joining_after_a_restart_starts_at_the_new_chain() {
    starts_at_the_new_chain(across_a_change(OutputEncoder::start));
}

#[test]
fn a_listener_joining_after_an_opus_retitle_starts_at_the_new_chain() {
    starts_at_the_new_chain(across_a_change(OutputEncoder::retitle));
}

#[test]
fn an_opus_output_refuses_a_retitle_before_start() {
    let (_mount, mut encoder) = output(OutputFormat::Opus, 96);
    assert!(encoder.retitle("Cloud Signal", None).is_err());
}

/// The bytes a listener gets for two seconds of MP3, retitled after one if `retitle`.
fn mp3_bytes(retitle: bool) -> Vec<u8> {
    let (mount, mut encoder) = output(OutputFormat::Mp3, 128);
    let listener = mount.listen().unwrap();
    encoder.start("Cloud Signal", None).unwrap();
    let mut chunks = Vec::new();
    for n in 0..2 * SECOND {
        if retitle && n == SECOND {
            encoder.retitle("Paper Harbour", None).unwrap();
        }
        encoder.encode(&block(n)).unwrap();
        drain(&listener, &mut chunks);
    }
    chunks.concat()
}

#[test]
fn a_title_change_does_not_break_an_mp3_stream() {
    let retitled = mp3_bytes(true);
    assert!(!retitled.is_empty());
    assert!(
        retitled == mp3_bytes(false),
        "the retitle changed the stream"
    );
}
