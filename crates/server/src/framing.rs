//! Splitting a source's bytes into whole Ogg pages or whole MP3 frames.
//!
//! A listener can only start decoding at a page or frame boundary, and an Ogg/Opus listener
//! also needs the header pages of the current logical stream first. The framer finds the
//! boundaries and keeps those header pages.

use std::sync::Arc;

/// Bytes kept waiting for the rest of a page or frame before they count as garbage.
const MAX_BUFFER: usize = 256 << 10;
/// Header pages kept for new listeners. A mount sends them ahead of the stream into a
/// bounded queue, so the count stays well under that queue's size.
const MAX_HEADER_PAGES: usize = 16;
/// The largest comment packet read for the title and artist.
const MAX_TAGS: usize = 64 << 10;

/// The two kinds of stream a source can push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ogg,
    Mp3,
}

impl Format {
    /// The format a source's `Content-Type` names, ignoring parameters and case.
    pub fn from_content_type(value: &str) -> Option<Format> {
        let essence = value.split(';').next().unwrap_or_default().trim();
        if essence.eq_ignore_ascii_case("audio/ogg")
            || essence.eq_ignore_ascii_case("application/ogg")
        {
            Some(Format::Ogg)
        } else if essence.eq_ignore_ascii_case("audio/mpeg") {
            Some(Format::Mp3)
        } else {
            None
        }
    }

    /// The `Content-Type` listeners get.
    pub fn content_type(self) -> &'static str {
        match self {
            Format::Ogg => "audio/ogg",
            Format::Mp3 => "audio/mpeg",
        }
    }
}

/// What the bytes at the front of the buffer are.
enum Step {
    /// A whole page or frame of this many bytes.
    Chunk(usize),
    /// This many bytes are not the start of one.
    Skip(usize),
    /// More bytes are needed to tell.
    Wait,
}

/// Cuts a byte stream into whole pages or frames, and keeps the Ogg header pages and tags.
#[derive(Debug)]
pub struct Framer {
    format: Format,
    buf: Vec<u8>,
    /// MP3: the last frame was followed by another, so the next one is trusted without
    /// looking past it.
    locked: bool,
    headers: Vec<Arc<[u8]>>,
    /// The serial number of the logical stream whose header pages are still arriving.
    collecting: Option<u32>,
    /// Which packet of the logical stream the next segment belongs to.
    packet_index: usize,
    /// The comment packet (the stream's second) as it arrives.
    comment: Vec<u8>,
    tags: Option<(String, Option<String>)>,
    /// Changes whenever the header pages or the tags change.
    version: u64,
}

impl Framer {
    pub fn new(format: Format) -> Self {
        Self {
            format,
            buf: Vec::new(),
            locked: false,
            headers: Vec::new(),
            collecting: None,
            packet_index: 0,
            comment: Vec::new(),
            tags: None,
            version: 0,
        }
    }

    /// Appends to `out` every page or frame `bytes` completes. A partial one waits for the
    /// next call; bytes before the first sync are dropped.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<Arc<[u8]>>) {
        self.buf.extend_from_slice(bytes);
        let mut pos = 0;
        while let Some(rest) = self.buf.get(pos..) {
            let step = match self.format {
                Format::Ogg => ogg_step(rest),
                Format::Mp3 => mp3_step(rest, self.locked),
            };
            match step {
                Step::Chunk(len) => {
                    let Some(bytes) = rest.get(..len) else { break };
                    let chunk: Arc<[u8]> = Arc::from(bytes);
                    pos += len;
                    match self.format {
                        Format::Ogg => self.observe_page(&chunk),
                        Format::Mp3 => self.locked = true,
                    }
                    out.push(chunk);
                }
                Step::Skip(len) => {
                    pos += len.max(1);
                    self.locked = false;
                }
                Step::Wait => break,
            }
        }
        self.buf.drain(..pos.min(self.buf.len()));
        if self.buf.len() > MAX_BUFFER {
            self.buf.clear();
            self.locked = false;
        }
    }

    /// Ogg: the pages of the current logical stream from its BOS page up to the last page at
    /// granule position 0 (OpusHead and OpusTags). MP3: empty.
    pub fn headers(&self) -> Vec<Arc<[u8]>> {
        self.headers.clone()
    }

    /// `TITLE` and `ARTIST` of the current logical stream's comment packet.
    pub fn tags(&self) -> Option<(String, Option<String>)> {
        self.tags.clone()
    }

    /// Bytes waiting for the rest of a page or frame.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Changes whenever [`Framer::headers`] or [`Framer::tags`] change.
    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    fn observe_page(&mut self, page: &[u8]) {
        let flags = page.get(5).copied().unwrap_or_default();
        let granule = read_u64(page, 6);
        let serial = read_u32(page, 14);
        if flags & 0x02 != 0 {
            self.headers = vec![Arc::from(page)];
            self.collecting = Some(serial);
            self.packet_index = 0;
            self.comment.clear();
            self.tags = None;
            self.version += 1;
            self.read_packets(page);
        } else if self.collecting == Some(serial)
            && granule == 0
            && self.headers.len() < MAX_HEADER_PAGES
        {
            self.headers.push(Arc::from(page));
            self.version += 1;
            self.read_packets(page);
        } else {
            self.collecting = None;
        }
    }

    /// Follows the packets of a header page, to read the comment packet.
    fn read_packets(&mut self, page: &[u8]) {
        let segments = usize::from(page.get(26).copied().unwrap_or_default());
        let Some(lacing) = page.get(27..27 + segments) else {
            return;
        };
        let mut offset = 27 + segments;
        for &len in lacing {
            let len = usize::from(len);
            if self.packet_index == 1
                && self.comment.len() + len <= MAX_TAGS
                && let Some(segment) = page.get(offset..offset + len)
            {
                self.comment.extend_from_slice(segment);
            }
            offset += len;
            if len < 255 {
                if self.packet_index == 1 {
                    self.tags = parse_tags(&self.comment);
                    self.comment = Vec::new();
                    self.version += 1;
                }
                self.packet_index += 1;
            }
        }
    }
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    bytes
        .get(at..at + 8)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u64::from_le_bytes)
}

/// Keeps the last `keep` bytes, which may be the start of a sync split across reads.
fn skip_all_but(rest: &[u8], keep: usize) -> Step {
    match rest.len().saturating_sub(keep) {
        0 => Step::Wait,
        n => Step::Skip(n),
    }
}

fn ogg_step(rest: &[u8]) -> Step {
    let Some(start) = rest.windows(4).position(|w| w == b"OggS") else {
        return skip_all_but(rest, 3);
    };
    if start > 0 {
        return Step::Skip(start);
    }
    let Some(head) = rest.get(..27) else {
        return Step::Wait;
    };
    // Version 0, and no header-type flags beyond continued, BOS and EOS.
    if head.get(4) != Some(&0) || head.get(5).is_none_or(|flags| flags & !0x07 != 0) {
        return Step::Skip(1);
    }
    let segments = usize::from(head.get(26).copied().unwrap_or_default());
    let Some(lacing) = rest.get(27..27 + segments) else {
        return Step::Wait;
    };
    let len = 27 + segments + lacing.iter().map(|&b| usize::from(b)).sum::<usize>();
    let Some(page) = rest.get(..len) else {
        return Step::Wait;
    };
    if ogg_crc(page) == read_u32(page, 22) {
        Step::Chunk(len)
    } else {
        Step::Skip(1)
    }
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            r = if r & 0x8000_0000 != 0 {
                (r << 1) ^ 0x04C1_1DB7
            } else {
                r << 1
            };
            bit += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
};

/// The Ogg page checksum, computed with the checksum field as zeros.
fn ogg_crc(page: &[u8]) -> u32 {
    page.iter().enumerate().fold(0u32, |crc, (i, &byte)| {
        let byte = if (22..26).contains(&i) { 0 } else { byte };
        (crc << 8) ^ CRC_TABLE[usize::from((crc >> 24) as u8 ^ byte)]
    })
}

/// The length of the MPEG audio Layer III frame this header starts, and the bits that stay
/// the same from frame to frame (version, layer, sample rate).
fn mp3_header(head: &[u8]) -> Option<(usize, (u8, u8))> {
    let &[b0, b1, b2, _] = head.get(..4)? else {
        return None;
    };
    if b0 != 0xFF || b1 & 0xE0 != 0xE0 {
        return None;
    }
    // Version: 0 is MPEG-2.5, 1 is reserved, 2 is MPEG-2, 3 is MPEG-1. Layer III is 1.
    let version = (b1 >> 3) & 0x03;
    if version == 1 || (b1 >> 1) & 0x03 != 1 {
        return None;
    }
    let mpeg1 = version == 3;
    const MPEG1_KBPS: [u32; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const MPEG2_KBPS: [u32; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];
    let table = if mpeg1 { &MPEG1_KBPS } else { &MPEG2_KBPS };
    let kbps = *table.get(usize::from(b2 >> 4))?;
    let rate = match ((b2 >> 2) & 0x03, version) {
        (0, 3) => 44_100,
        (1, 3) => 48_000,
        (2, 3) => 32_000,
        (0, 2) => 22_050,
        (1, 2) => 24_000,
        (2, 2) => 16_000,
        (0, 0) => 11_025,
        (1, 0) => 12_000,
        (2, 0) => 8_000,
        _ => return None,
    };
    // Free format (index 0) has no length in the header.
    if kbps == 0 {
        return None;
    }
    let factor = if mpeg1 { 144 } else { 72 };
    let padding = u32::from((b2 >> 1) & 0x01);
    let len = factor * kbps * 1000 / rate + padding;
    Some((usize::try_from(len).ok()?, (b1 & 0xFE, b2 & 0x0C)))
}

fn mp3_step(rest: &[u8], locked: bool) -> Step {
    let Some(start) = rest
        .windows(2)
        .position(|w| matches!(w, [0xFF, next] if next & 0xE0 == 0xE0))
    else {
        return skip_all_but(rest, 1);
    };
    if start > 0 {
        return Step::Skip(start);
    }
    if rest.len() < 4 {
        return Step::Wait;
    }
    let Some((len, fixed)) = mp3_header(rest) else {
        return Step::Skip(1);
    };
    if locked {
        return if rest.len() >= len {
            Step::Chunk(len)
        } else {
            Step::Wait
        };
    }
    // Out of sync, a header counts only when the next frame's header follows it.
    let Some(next) = rest.get(len..len + 4) else {
        return Step::Wait;
    };
    match mp3_header(next) {
        Some((_, next_fixed)) if next_fixed == fixed => Step::Chunk(len),
        _ => Step::Skip(1),
    }
}

/// `TITLE` and `ARTIST` of an OpusTags (or Vorbis comment) packet.
fn parse_tags(packet: &[u8]) -> Option<(String, Option<String>)> {
    let body = packet
        .strip_prefix(b"OpusTags")
        .or_else(|| packet.strip_prefix(b"\x03vorbis"))?;
    let vendor = usize::try_from(read_u32(body, 0)).ok()?;
    let mut rest = body.get(4usize.checked_add(vendor)?..)?;
    // The declared count is only an upper bound: every comment must also fit in the packet.
    let count = read_u32(rest, 0);
    rest = rest.get(4..)?;
    let (mut title, mut artist) = (None, None);
    for _ in 0..count {
        let Some(len) = rest.get(..4).map(|_| read_u32(rest, 0)) else {
            break;
        };
        let Some(comment) = usize::try_from(len)
            .ok()
            .and_then(|len| rest.get(4..4usize.checked_add(len)?))
        else {
            break;
        };
        rest = rest.get(4 + comment.len()..).unwrap_or_default();
        let Some(eq) = comment.iter().position(|&b| b == b'=') else {
            continue;
        };
        let key = comment.get(..eq).unwrap_or_default();
        let value = comment.get(eq + 1..).unwrap_or_default();
        let value = String::from_utf8_lossy(value).into_owned();
        if key.eq_ignore_ascii_case(b"TITLE") {
            title.get_or_insert(value);
        } else if key.eq_ignore_ascii_case(b"ARTIST") {
            artist.get_or_insert(value);
        }
    }
    Some((title.unwrap_or_default(), artist))
}
