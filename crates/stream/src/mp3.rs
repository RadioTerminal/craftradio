//! MP3 encoding for the Icecast output, with craft-audio's native encoder.
use crate::EncodeError;
use craft_audio::mp3::{Config, Encoder, bitrates};

/// The stream's sample rate; the output thread feeds interleaved stereo at 48 kHz.
const SAMPLE_RATE: u32 = 48_000;

pub struct Mp3Encoder {
    /// Boxed: the encoder's state is large and the output enum holds it inline.
    encoder: Box<Encoder>,
    left: Vec<f32>,
    right: Vec<f32>,
}

fn error(value: impl std::fmt::Display) -> EncodeError {
    EncodeError::Invalid(format!("MP3 encoder: {value}"))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_bitrate_mpeg1_cannot_carry_at_48_khz() {
        assert!(Mp3Encoder::new(24).is_err());
        assert!(Mp3Encoder::new(128).is_ok());
    }

    /// Decodes an MP3 byte stream to interleaved stereo with symphonia.
    fn decode(bytes: Vec<u8>) -> Vec<f32> {
        use symphonia::core::{
            codecs::audio::AudioDecoderOptions,
            formats::{FormatOptions, TrackType, probe::Hint},
            io::MediaSourceStream,
            meta::MetadataOptions,
        };
        let source =
            MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                source,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .unwrap();
        let track = format.default_track(TrackType::Audio).unwrap();
        let params = track
            .codec_params
            .as_ref()
            .unwrap()
            .audio()
            .unwrap()
            .clone();
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .unwrap();
        let (mut out, mut samples) = (Vec::new(), Vec::new());
        while let Ok(Some(packet)) = format.next_packet() {
            decoder
                .decode(&packet)
                .unwrap()
                .copy_to_vec_interleaved(&mut samples);
            out.extend_from_slice(&samples);
        }
        out
    }

    #[test]
    fn a_tone_decodes_back_close_to_what_went_in() {
        let frames = 48_000;
        let input: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                let left = 0.5 * (std::f32::consts::TAU * 1_000.0 * t).sin();
                let right = 0.5 * (std::f32::consts::TAU * 440.0 * t).sin();
                [left, right]
            })
            .collect();
        let mut encoder = Mp3Encoder::new(128).unwrap();
        let mut bytes = Vec::new();
        for chunk in input.chunks(1_920) {
            bytes.extend(encoder.encode(chunk).unwrap());
        }
        let output = decode(bytes);
        // The decoder lags the input; find the lag where the two line up best.
        let compare = 24_000 * 2;
        let snr = (0..4_000)
            .map(|lag| {
                let (mut signal, mut noise) = (0.0f64, 0.0f64);
                for (i, &x) in input[..compare].iter().enumerate() {
                    let y = output.get(i + lag * 2).copied().unwrap_or(0.0);
                    signal += f64::from(x * x);
                    noise += f64::from((x - y) * (x - y));
                }
                10.0 * (signal / noise.max(1e-12)).log10()
            })
            .fold(f64::MIN, f64::max);
        assert!(snr > 20.0, "signal to noise {snr:.1} dB");
    }
}
