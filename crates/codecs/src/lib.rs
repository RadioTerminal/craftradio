//! The audio codecs the station decodes with: Symphonia's, plus Opus through craft-audio's
//! native decoder (storytold/craft-libs, MIT OR Apache-2.0). Playback and library analysis use
//! the same [`registry`], so a file that analyses also plays.
//!
//! The Opus decoder follows the shape of symphonia-adapter-libopus (MIT OR Apache-2.0), with
//! craft-audio's decoder in place of libopus.
//!
//! The encoders are in [`encode`], behind the features `encode` (Opus in Ogg) and `encode-mp3`.

#[cfg(any(feature = "encode", feature = "encode-mp3"))]
pub mod encode;

use craft_audio::opus::StreamDecoder;
use symphonia::core::audio::{
    AsGenericAudioBufferRef, AudioBuffer, AudioMut, AudioSpec, Channels, GenericAudioBufferRef,
    layouts,
};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{
    CodecRegistry, RegisterableAudioDecoder, SupportedAudioCodec,
};
use symphonia::core::errors::{Error, Result, unsupported_error};
use symphonia::core::packet::PacketRef;

/// Symphonia's codecs and Opus.
pub fn registry() -> CodecRegistry {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry.register_audio_decoder::<OpusDecoder>();
    registry
}

/// Opus always codes at 48 kHz; a stream may ask to be decoded at a lower rate.
const OPUS_RATE: u32 = 48_000;
/// The rates the decoder can write.
const RATES: [u32; 5] = [8_000, 12_000, 16_000, 24_000, 48_000];
/// 20 ms at 48 kHz: the usual packet, the buffer's first size.
const USUAL_FRAMES: usize = 960;

/// Opus for Symphonia, decoded by craft-audio.
pub struct OpusDecoder {
    params: AudioCodecParameters,
    decoder: StreamDecoder,
    buf: AudioBuffer<f32>,
    pcm: Vec<f32>,
    rate: u32,
    channels: usize,
    /// Samples at the decoder's rate still to drop from the start (the stream's pre-skip).
    pre_skip: usize,
}

impl std::fmt::Debug for OpusDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpusDecoder")
            .field("rate", &self.rate)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// The pre-skip of an `OpusHead` (RFC 7845 5.1): bytes 10 and 11, little-endian.
fn pre_skip(head: &[u8]) -> usize {
    match head {
        [
            b'O',
            b'p',
            b'u',
            b's',
            b'H',
            b'e',
            b'a',
            b'd',
            _,
            _,
            lo,
            hi,
            ..,
        ] => usize::from(u16::from_le_bytes([*lo, *hi])),
        _ => 0,
    }
}

fn layout(channels: usize) -> Option<Channels> {
    match channels {
        1 => Some(layouts::CHANNEL_LAYOUT_MONO),
        2 => Some(layouts::CHANNEL_LAYOUT_STEREO),
        _ => None,
    }
}

fn buffer(rate: u32, channels: &Channels, frames: usize) -> AudioBuffer<f32> {
    AudioBuffer::new(AudioSpec::new(rate, channels.clone()), frames)
}

impl OpusDecoder {
    fn try_new(params: &AudioCodecParameters) -> Result<Self> {
        let Some(count) = params.channels.as_ref().map(Channels::count) else {
            return unsupported_error("opus: the stream names no channels");
        };
        let Some(layout) = layout(count) else {
            return unsupported_error("opus: only mono and stereo are supported");
        };
        let rate = params
            .sample_rate
            .filter(|r| RATES.contains(r))
            .unwrap_or(OPUS_RATE);
        let decoder = StreamDecoder::new(rate, count)
            .map_err(|_| Error::DecodeError("opus: the decoder cannot start"))?;
        let skip = params.extra_data.as_deref().map(pre_skip).unwrap_or(0);
        Ok(Self {
            params: params.clone(),
            decoder,
            buf: buffer(rate, &layout, USUAL_FRAMES),
            pcm: Vec::with_capacity(USUAL_FRAMES * count),
            rate,
            channels: count,
            pre_skip: skip * rate as usize / OPUS_RATE as usize,
        })
    }
}

impl AudioDecoder for OpusDecoder {
    fn codec_info(&self) -> &CodecInfo {
        &Self::supported_codecs()[0].info
    }

    fn reset(&mut self) {
        self.decoder.reset();
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        self.pcm.clear();
        // An empty packet is a lost one: the decoder conceals it.
        let data = (!packet.data.is_empty()).then_some(packet.data);
        let frames = self
            .decoder
            .decode_interleaved(data, &mut self.pcm)
            .map_err(|_| Error::DecodeError("opus: a packet does not decode"))?;
        if frames != self.buf.capacity() {
            let layout = layout(self.channels).unwrap_or(layouts::CHANNEL_LAYOUT_STEREO);
            self.buf = buffer(self.rate, &layout, frames);
        }
        self.buf.clear();
        self.buf.render_uninit(None);
        let pcm = &self.pcm[..frames * self.channels];
        self.buf.copy_from_slice_interleaved(&pcm);
        // The pre-skip is dropped from the start of the stream only, over as many packets as
        // it spans.
        let skip = self.pre_skip.min(frames);
        self.pre_skip -= skip;
        self.buf.trim(
            packet.trim_start.get() as usize + skip,
            packet.trim_end.get() as usize,
        );
        Ok(self.buf.as_generic_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.buf.as_generic_audio_buffer_ref()
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(
        params: &AudioCodecParameters,
        _opts: &AudioDecoderOptions,
    ) -> Result<Box<dyn AudioDecoder>> {
        Ok(Box::new(OpusDecoder::try_new(params)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        const OPUS: [SupportedAudioCodec; 1] = [SupportedAudioCodec {
            id: CODEC_ID_OPUS,
            info: CodecInfo {
                short_name: "opus",
                long_name: "Opus",
                profiles: &[],
            },
        }];
        &OPUS
    }
}
