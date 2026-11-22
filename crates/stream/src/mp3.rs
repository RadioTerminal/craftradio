//! All LAME bindings stay in this feature-gated module.
use crate::StreamError;
use radio_lame::{Encoder, Settings};

pub(crate) struct Mp3Encoder(Encoder);

fn error(value: impl std::fmt::Display) -> StreamError {
    StreamError::Invalid(format!("MP3 encoder: {value}"))
}

/// The MPEG-1 Layer III bitrates in kbit/s. LAME would round any other value.
const BITRATES: [u32; 16] = [
    8, 16, 24, 32, 40, 48, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];

impl Mp3Encoder {
    pub(crate) fn new(kbps: u32) -> Result<Self, StreamError> {
        if !BITRATES.contains(&kbps) {
            return Err(error("unsupported bitrate"));
        }
        let settings = Settings {
            channels: 2,
            sample_rate: 48_000,
            kbps,
        };
        Encoder::new(settings).map(Self).map_err(error)
    }

    pub(crate) fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, StreamError> {
        self.0.encode(samples).map_err(error)
    }
}
