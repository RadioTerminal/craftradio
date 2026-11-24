use radio_server::{Format, Framer};
use radio_stream::{OggMuxer, OpusEncoder};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// A writer the test can empty while the muxer still owns it.
#[derive(Clone, Default)]
pub struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    /// The bytes written since the last call.
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An Ogg/Opus stream made the way the app makes it.
pub struct OggSource {
    muxer: OggMuxer<Sink>,
    sink: Sink,
    encoder: OpusEncoder,
    packets: usize,
}

impl OggSource {
    pub fn new(title: &str, artist: Option<&str>) -> Self {
        let sink = Sink::default();
        let mut encoder = OpusEncoder::new(96).unwrap();
        let mut muxer = OggMuxer::new(sink.clone());
        muxer
            .start(title, artist, encoder.pre_skip().unwrap())
            .unwrap();
        Self {
            muxer,
            sink,
            encoder,
            packets: 0,
        }
    }

    /// Encodes `ms` milliseconds of a tone, one page per 20 ms packet.
    pub fn audio(&mut self, ms: usize) {
        for _ in 0..ms / 20 {
            let samples: Vec<f32> = (0..960)
                .flat_map(|i| {
                    let t = (self.packets * 960 + i) as f32 / 48_000.0;
                    let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.2;
                    [s, s]
                })
                .collect();
            let packet = self.encoder.encode(&samples).unwrap();
            self.muxer.packet(packet, false).unwrap();
            self.packets += 1;
        }
    }

    /// Ends the logical stream and starts a new one, as the app does on a title change.
    pub fn restart(&mut self, title: &str, artist: Option<&str>) {
        let packet = self.encoder.encode(&[0.0; 1920]).unwrap();
        self.muxer.packet(packet, true).unwrap();
        self.encoder = OpusEncoder::new(96).unwrap();
        self.muxer
            .start(title, artist, self.encoder.pre_skip().unwrap())
            .unwrap();
    }

    /// The bytes written since the last call.
    pub fn take(&mut self) -> Vec<u8> {
        self.sink.take()
    }
}

/// Splits whole Ogg pages off the front of `bytes`, independently of the framer under test.
/// An incomplete page at the end is left out.
pub fn pages(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while rest.len() >= 27 {
        assert_eq!(&rest[..4], b"OggS", "test input is not a page sequence");
        let segments = rest[26] as usize;
        if rest.len() < 27 + segments {
            break;
        }
        let body: usize = rest[27..27 + segments].iter().map(|&b| b as usize).sum();
        let len = 27 + segments + body;
        if rest.len() < len {
            break;
        }
        out.push(&rest[..len]);
        rest = &rest[len..];
    }
    out
}

pub fn granule(page: &[u8]) -> u64 {
    u64::from_le_bytes(page[6..14].try_into().unwrap())
}

pub fn is_bos(page: &[u8]) -> bool {
    page[5] & 0x02 != 0
}

fn frame(format: Format, bytes: &[u8], piece: usize) -> (Vec<Arc<[u8]>>, Framer) {
    let mut framer = Framer::new(format);
    let mut out = Vec::new();
    for part in bytes.chunks(piece) {
        framer.push(part, &mut out);
    }
    (out, framer)
}

fn as_vecs(chunks: &[Arc<[u8]>]) -> Vec<Vec<u8>> {
    chunks.iter().map(|c| c.to_vec()).collect()
}

#[test]
fn ogg_chunks_are_whole_pages_however_the_bytes_arrive() {
    let mut source = OggSource::new("Cloud Signal", Some("Echo"));
    source.audio(500);
    source.restart("Dawn", None);
    source.audio(500);
    let input = source.take();
    let expected: Vec<Vec<u8>> = pages(&input).iter().map(|p| p.to_vec()).collect();
    for piece in [1, 7, input.len()] {
        let (chunks, _) = frame(Format::Ogg, &input, piece);
        assert!(chunks.iter().all(|c| c.starts_with(b"OggS")));
        assert_eq!(chunks.concat(), input, "{piece}-byte pieces");
        assert_eq!(as_vecs(&chunks), expected, "{piece}-byte pieces");
    }
}

#[test]
fn a_new_title_replaces_the_header_pages() {
    let mut source = OggSource::new("Cloud Signal", Some("Echo"));
    source.audio(200);
    let first = source.take();
    let mut framer = Framer::new(Format::Ogg);
    let mut out = Vec::new();
    framer.push(&first, &mut out);
    assert_eq!(
        framer.tags(),
        Some(("Cloud Signal".to_owned(), Some("Echo".to_owned())))
    );
    let before = as_vecs(&framer.headers());
    // From the BOS page up to the last page at granule 0: OpusHead and OpusTags.
    let source_pages = pages(&first);
    let header_count = source_pages.iter().take_while(|p| granule(p) == 0).count();
    assert!(header_count >= 2);
    assert_eq!(
        before,
        source_pages[..header_count]
            .iter()
            .map(|p| p.to_vec())
            .collect::<Vec<_>>()
    );

    source.restart("Dawn", None);
    source.audio(200);
    framer.push(&source.take(), &mut out);
    assert_eq!(framer.tags(), Some(("Dawn".to_owned(), None)));
    let after = as_vecs(&framer.headers());
    assert_ne!(after, before);
    assert!(is_bos(&after[0]));
    assert!(after.iter().all(|p| granule(p) == 0));
}

/// One MPEG audio Layer III frame: a header and a payload of the given total length. The
/// payload carries a frame sync in its middle, so a framer that splits on every sync fails.
fn mp3_frame(b1: u8, b2: u8, len: usize) -> Vec<u8> {
    let mut frame = vec![0xFF, b1, b2, 0xC4];
    frame.resize(len, 0x55);
    frame[len / 2] = 0xFF;
    frame[len / 2 + 1] = 0xFB;
    frame
}

/// Two variable-bitrate streams, one MPEG-1 at 44.1 kHz and one MPEG-2 at 24 kHz, with frame
/// lengths worked out by hand from the MPEG tables rather than by the code under test.
fn mp3_streams() -> [Vec<Vec<u8>>; 2] {
    let mut mpeg1 = Vec::new();
    let mut mpeg2 = Vec::new();
    for _ in 0..20 {
        // 144 * kbps * 1000 / 44100, plus the padding bit: 128 -> 417 or 418, 96 -> 313,
        // 320 -> 1044 or 1045.
        mpeg1.push(mp3_frame(0xFB, 0x90, 417));
        mpeg1.push(mp3_frame(0xFB, 0x92, 418));
        mpeg1.push(mp3_frame(0xFB, 0x70, 313));
        mpeg1.push(mp3_frame(0xFB, 0xE0, 1044));
        mpeg1.push(mp3_frame(0xFB, 0xE2, 1045));
        // 72 * kbps * 1000 / 24000, plus the padding bit: 64 -> 192, 160 -> 480, 8 -> 24 or 25.
        mpeg2.push(mp3_frame(0xF3, 0x84, 192));
        mpeg2.push(mp3_frame(0xF3, 0xE4, 480));
        mpeg2.push(mp3_frame(0xF3, 0x14, 24));
        mpeg2.push(mp3_frame(0xF3, 0x16, 25));
    }
    [mpeg1, mpeg2]
}

#[test]
fn mp3_chunks_start_at_a_frame() {
    // radio_stream's MP3 encoder is reachable only through an Icecast connection, so the
    // frames are built by hand (see mp3_streams).
    for frames in mp3_streams() {
        let mut junk: Vec<u8> = (0..100u8).map(|i| i.wrapping_mul(37)).collect();
        // A header that parses, inside the junk; the frame it claims is not followed by
        // another.
        junk[10..14].copy_from_slice(&[0xFF, 0xFB, 0x90, 0xC4]);
        let input = [junk, frames.concat()].concat();
        for piece in [1, 7, input.len()] {
            let (chunks, framer) = frame(Format::Mp3, &input, piece);
            assert!(
                chunks.iter().all(|c| c[0] == 0xFF && c[1] & 0xE0 == 0xE0),
                "{piece}-byte pieces"
            );
            assert_eq!(as_vecs(&chunks), frames, "{piece}-byte pieces");
            assert!(framer.headers().is_empty());
        }
    }
}

#[test]
fn garbage_input_does_not_grow_the_buffer() {
    for format in [Format::Ogg, Format::Mp3] {
        let mut framer = Framer::new(format);
        let mut out = Vec::new();
        for part in vec![0u8; 2 << 20].chunks(64 << 10) {
            framer.push(part, &mut out);
        }
        assert!(out.is_empty());
        assert!(framer.buffered() < 256 << 10, "{format:?}");
    }

    let mut source = OggSource::new("Cloud Signal", None);
    source.audio(200);
    let valid = source.take();
    // A fake page header claiming the largest page, with a wrong checksum.
    let mut fake = b"OggS\x00\x00".to_vec();
    fake.resize(26, 0x11);
    fake.push(255);
    fake.extend([255; 255]);
    let mut input = vec![0x22u8; 300 << 10];
    input.extend(&fake);
    input.extend(vec![0x33; 70_000]);
    input.extend(&valid);
    let (chunks, framer) = frame(Format::Ogg, &input, input.len());
    let expected: Vec<Vec<u8>> = pages(&valid).iter().map(|p| p.to_vec()).collect();
    assert_eq!(as_vecs(&chunks), expected);
    assert!(framer.buffered() < 256 << 10);
}

#[test]
fn content_types_name_their_format() {
    assert_eq!(Format::from_content_type("audio/ogg"), Some(Format::Ogg));
    assert_eq!(
        Format::from_content_type("application/ogg"),
        Some(Format::Ogg)
    );
    assert_eq!(Format::from_content_type("audio/mpeg"), Some(Format::Mp3));
    assert_eq!(Format::from_content_type("text/html"), None);
    for format in [Format::Ogg, Format::Mp3] {
        assert_eq!(
            Format::from_content_type(format.content_type()),
            Some(format)
        );
    }
}
