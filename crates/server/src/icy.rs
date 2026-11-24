//! ICY metadata (SHOUTcast/Icecast): titles interleaved in an MP3 stream for players that
//! ask for them with `Icy-MetaData: 1`. After every [`METAINT`] audio bytes comes one
//! block: a length byte `n`, then `n * 16` bytes of `StreamTitle='…';` padded with zeros.
//! `n = 0` is an empty block, sent while the title is unchanged.

/// Audio bytes between two metadata blocks, as `icy-metaint` tells the listener.
pub const METAINT: usize = 16_000;

/// The most a block holds: its length byte counts 16-byte units.
const MAX_BLOCK: usize = u8::MAX as usize * 16;
const PREFIX: &str = "StreamTitle='";
const SUFFIX: &str = "';";

/// Interleaves metadata blocks into one listener's stream.
#[derive(Debug, Default)]
pub struct IcyWriter {
    /// Audio bytes written since the last block.
    audio: usize,
    /// The title the last block that carried one carried.
    sent: String,
}

impl IcyWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether writing `len` audio bytes next reaches a metadata block, so the caller
    /// need look up the title only then.
    pub fn due(&self, len: usize) -> bool {
        self.audio + len >= METAINT
    }

    /// Appends `chunk` to `out`, with a metadata block after every `METAINT` audio
    /// bytes: `StreamTitle='<now>';` when `now` differs from the last one sent, else an
    /// empty block.
    pub fn write(&mut self, chunk: &[u8], now: &str, out: &mut Vec<u8>) {
        let mut rest = chunk;
        while !rest.is_empty() {
            let room = METAINT.saturating_sub(self.audio).min(rest.len());
            let (audio, after) = rest.split_at(room);
            out.extend_from_slice(audio);
            self.audio += audio.len();
            rest = after;
            if self.audio >= METAINT {
                self.block(now, out);
                self.audio = 0;
            }
        }
    }

    fn block(&mut self, now: &str, out: &mut Vec<u8>) {
        if now == self.sent {
            out.push(0);
            return;
        }
        // ICY has no escaping: a `'` is sent as it is; players read up to `';`.
        let text = cut(now, MAX_BLOCK - PREFIX.len() - SUFFIX.len());
        let len = PREFIX.len() + text.len() + SUFFIX.len();
        let units = len.div_ceil(16);
        let Ok(length) = u8::try_from(units) else {
            // Can't happen: the text was cut to fit. An empty block keeps the stream whole.
            out.push(0);
            return;
        };
        out.push(length);
        let start = out.len();
        out.extend_from_slice(PREFIX.as_bytes());
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(SUFFIX.as_bytes());
        out.resize(start + units * 16, 0);
        now.clone_into(&mut self.sent);
    }
}

/// `text`, cut to at most `max` bytes at a character boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}
