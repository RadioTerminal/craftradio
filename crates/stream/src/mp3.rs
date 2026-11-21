//! All LAME bindings stay in this feature-gated module.
use crate::StreamError;
use mp3lame_encoder::{Bitrate, Builder, Encoder, InterleavedPcm};

pub(crate) struct Mp3Encoder(Encoder);

fn error(value: impl std::fmt::Display) -> StreamError {
    StreamError::Invalid(format!("MP3 encoder: {value}"))
}

impl Mp3Encoder {
    pub(crate) fn new(kbps: u32) -> Result<Self, StreamError> {
        let bitrate = match kbps {
            8 => Bitrate::Kbps8,
            16 => Bitrate::Kbps16,
            24 => Bitrate::Kbps24,
            32 => Bitrate::Kbps32,
            40 => Bitrate::Kbps40,
            48 => Bitrate::Kbps48,
            64 => Bitrate::Kbps64,
            80 => Bitrate::Kbps80,
            96 => Bitrate::Kbps96,
            112 => Bitrate::Kbps112,
            128 => Bitrate::Kbps128,
            160 => Bitrate::Kbps160,
            192 => Bitrate::Kbps192,
            224 => Bitrate::Kbps224,
            256 => Bitrate::Kbps256,
            320 => Bitrate::Kbps320,
            _ => return Err(error("unsupported bitrate")),
        };
        let encoder = Builder::new()
            .ok_or_else(|| error("allocation failed"))?
            .with_num_channels(2)
            .map_err(error)?
            .with_sample_rate(48_000)
            .map_err(error)?
            .with_brate(bitrate)
            .map_err(error)?
            .with_to_write_vbr_tag(false)
            .map_err(error)?
            .build()
            .map_err(error)?;
        Ok(Self(encoder))
    }

    pub(crate) fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, StreamError> {
        let mut bytes =
            Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(samples.len() / 2));
        self.0
            .encode_to_vec(InterleavedPcm(samples), &mut bytes)
            .map_err(error)?;
        Ok(bytes)
    }
}
