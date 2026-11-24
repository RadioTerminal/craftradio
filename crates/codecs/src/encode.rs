//! The encoders a station streams with: Opus in Ogg (feature `encode`), and MP3 with
//! craft-audio's native encoder (feature `encode-mp3`).
#[cfg(feature = "encode-mp3")]
use craft_audio::mp3::{Config, Encoder, bitrates};
#[cfg(feature = "encode")]
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
#[cfg(feature = "encode")]
use std::io::Write;

/// Why an encoder or the Ogg muxer failed.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("{0}")]
    Invalid(String),
    #[cfg(feature = "encode")]
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[cfg(feature = "encode")]
    #[error(transparent)]
    Opus(#[from] opus::Error),
}

/// Encodes exactly 20 ms of 48 kHz stereo audio per packet.
#[cfg(feature = "encode")]
pub struct OpusEncoder(opus::Encoder);
#[cfg(feature = "encode")]
impl OpusEncoder {
    pub fn new(kbps: u32) -> Result<Self, EncodeError> {
        if !(6..=510).contains(&kbps) {
            return Err(EncodeError::Invalid("bitrate must be 6..510 kbps".into()));
        }
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)?;
        encoder.set_bitrate(opus::Bitrate::Bits((kbps * 1000) as i32))?;
        Ok(Self(encoder))
    }
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, EncodeError> {
        if samples.len() != 1920 {
            return Err(EncodeError::Invalid(
                "expected 1920 interleaved samples".into(),
            ));
        }
        let mut packet = vec![0; 4000];
        let len = self.0.encode_float(samples, &mut packet)?;
        packet.truncate(len);
        Ok(packet)
    }
    pub fn pre_skip(&mut self) -> Result<u16, EncodeError> {
        Ok(self.0.get_lookahead()? as u16)
    }
}

/// Writes Ogg pages, with a distinct serial number for each metadata chain.
#[cfg(feature = "encode")]
pub struct OggMuxer<W: Write> {
    writer: PacketWriter<'static, W>,
    serial: u32,
    granule: u64,
}
#[cfg(feature = "encode")]
impl<W: Write> OggMuxer<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: PacketWriter::new(writer),
            serial: 0,
            granule: 0,
        }
    }
    /// The writer the pages go to.
    pub fn get_mut(&mut self) -> &mut W {
        self.writer.inner_mut()
    }
    pub fn start(
        &mut self,
        title: &str,
        artist: Option<&str>,
        pre_skip: u16,
    ) -> Result<(), EncodeError> {
        self.serial = self.serial.wrapping_add(1);
        self.granule = 0;
        let mut head = b"OpusHead\x01\x02".to_vec();
        head.extend(pre_skip.to_le_bytes());
        head.extend(48_000u32.to_le_bytes());
        head.extend([0, 0, 0]);
        self.writer
            .write_packet(head, self.serial, PacketWriteEndInfo::EndPage, 0)?;
        let mut tags = b"OpusTags".to_vec();
        tags.extend(12u32.to_le_bytes());
        tags.extend(b"radio-stream");
        tags.extend(if artist.is_some() { 2u32 } else { 1u32 }.to_le_bytes());
        for comment in
            std::iter::once(format!("TITLE={title}")).chain(artist.map(|a| format!("ARTIST={a}")))
        {
            tags.extend((comment.len() as u32).to_le_bytes());
            tags.extend(comment.as_bytes());
        }
        self.writer
            .write_packet(tags, self.serial, PacketWriteEndInfo::EndPage, 0)?;
        Ok(())
    }
    pub fn packet(&mut self, packet: Vec<u8>, end: bool) -> Result<(), EncodeError> {
        self.granule += 960;
        self.writer.write_packet(
            packet,
            self.serial,
            if end {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::EndPage
            },
            self.granule,
        )?;
        Ok(())
    }
}

/// The stream's sample rate; the output thread feeds interleaved stereo at 48 kHz.
#[cfg(feature = "encode-mp3")]
const SAMPLE_RATE: u32 = 48_000;

/// MP3 encoding, with craft-audio's native encoder.
#[cfg(feature = "encode-mp3")]
pub struct Mp3Encoder {
    /// Boxed: the encoder's state is large and the output enum holds it inline.
    encoder: Box<Encoder>,
    left: Vec<f32>,
    right: Vec<f32>,
}

#[cfg(feature = "encode-mp3")]
fn error(value: impl std::fmt::Display) -> EncodeError {
    EncodeError::Invalid(format!("MP3 encoder: {value}"))
}

#[cfg(feature = "encode-mp3")]
impl Mp3Encoder {
    /// A constant-bitrate, joint-stereo encoder. At 48 kHz MPEG-1 Layer III carries 32 to 320
    /// kbit/s; any other value is refused rather than rounded.
    pub fn new(kbps: u32) -> Result<Self, EncodeError> {
        if !bitrates(SAMPLE_RATE).contains(&kbps) {
            return Err(error(format!(
                "{kbps} kbit/s is not an MP3 bitrate at 48 kHz (32 to 320)"
            )));
        }
        let encoder = Box::new(Encoder::new(Config::cbr(SAMPLE_RATE, 2, kbps)).map_err(error)?);
        Ok(Self {
            encoder,
            left: Vec::new(),
            right: Vec::new(),
        })
    }

    /// Encodes interleaved stereo samples and returns the frames that are complete.
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, EncodeError> {
        self.left.clear();
        self.right.clear();
        for pair in samples.as_chunks::<2>().0 {
            self.left.push(pair[0]);
            self.right.push(pair[1]);
        }
        Ok(self.encoder.encode(&[&self.left, &self.right]))
    }
}
