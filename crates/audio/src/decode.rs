//! File decoding with symphonia, and conversion to the engine format with rubato.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, Fft, FixedAsync, FixedSync, PolynomialDegree, Resampler};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

use crate::{AudioError, CHANNELS, SAMPLE_RATE};

/// The lowest sample rate that the decoder accepts, in Hz.
pub const MIN_RATE: u32 = 8_000;
/// The highest sample rate that the decoder accepts, in Hz.
pub const MAX_RATE: u32 = 384_000;
/// The highest channel count that the decoder accepts.
pub const MAX_CHANNELS: usize = 8;

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Facts about a file, from its container.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The playable length.
    pub len: Duration,
    /// The sample rate of the file, before any resampling.
    pub sample_rate: u32,
    /// The number of channels in the file.
    pub channels: u16,
}

/// Reads the length, rate and channel count of a file. If the container gives no length,
/// `probe` decodes the file to count its frames.
pub fn probe(path: impl AsRef<Path>) -> Result<Probe, AudioError> {
    let path = path.as_ref();
    let source = Source::open(path)?;
    let rate = source.sample_rate;
    let channels = u16::try_from(source.channels).unwrap_or(u16::MAX);
    let len = match source.len_frames() {
        Some(frames) => frames_to_duration(frames, rate),
        None => {
            let mut decoder = Decoder::from_source(path, source, Duration::ZERO, false)?;
            let mut frames = 0u64;
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let more = decoder.read(&mut buf)?;
                frames += (buf.len() / CHANNELS) as u64;
                if !more {
                    break;
                }
            }
            frames_to_duration(frames, rate)
        }
    };
    Ok(Probe {
        len,
        sample_rate: rate,
        channels,
    })
}

fn frames_to_duration(frames: u64, rate: u32) -> Duration {
    let rate = u64::from(rate.max(1));
    let secs = frames / rate;
    let rest = frames % rate;
    Duration::from_secs(secs) + Duration::from_nanos(rest * 1_000_000_000 / rate)
}

/// The demuxer and codec of one audio track.
struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    time_base: Option<TimeBase>,
    num_frames: Option<u64>,
    duration: Option<symphonia::core::units::Duration>,
}

impl Source {
    fn open(path: &Path) -> Result<Self, AudioError> {
        let unsupported = |reason: String| AudioError::Unsupported {
            path: path.to_path_buf(),
            reason,
        };
        let file = File::open(path).map_err(|source| AudioError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| unsupported(e.to_string()))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| unsupported("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| unsupported("no audio codec parameters".into()))?;
        let sample_rate = params
            .sample_rate
            .ok_or_else(|| unsupported("no sample rate".into()))?;
        // Treat each media file as untrusted input. A forged header must not size a buffer.
        if !(MIN_RATE..=MAX_RATE).contains(&sample_rate) {
            return Err(unsupported(format!(
                "sample rate {sample_rate} Hz is out of range"
            )));
        }
        let channels = params.channels.as_ref().map_or(0, |c| c.count());
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(unsupported(format!("{channels} channels is out of range")));
        }
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| unsupported(e.to_string()))?;
        let track_id = track.id;
        let time_base = track.time_base;
        let num_frames = track.num_frames;
        let duration = track.duration;
        Ok(Self {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            time_base,
            num_frames,
            duration,
        })
    }

    fn len_frames(&self) -> Option<u64> {
        if let Some(n) = self.num_frames {
            return Some(n);
        }
        let time = self.time_base?.calc_duration(self.duration?)?;
        let nanos = u128::try_from(time.as_nanos()).ok()?;
        u64::try_from(nanos * u128::from(self.sample_rate) / 1_000_000_000).ok()
    }

    /// Seeks to `cue_in` and returns how many source frames to drop after the seek.
    fn seek(&mut self, cue_in: Duration) -> Result<u64, AudioError> {
        let time = Time::try_from_secs_f64(cue_in.as_secs_f64())
            .ok_or_else(|| AudioError::Decode("cue point out of range".into()))?;
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| AudioError::Decode(format!("seek: {e}")))?;
        self.decoder.reset();
        let gap = seeked
            .required_ts
            .get()
            .saturating_sub(seeked.actual_ts.get())
            .max(0);
        let frames = match self.time_base {
            Some(tb) => {
                let t = tb.calc_time(symphonia::core::units::Timestamp::new(gap));
                let nanos = t.map_or(0, |t| t.as_nanos()).max(0);
                u64::try_from(nanos * i128::from(self.sample_rate) / 1_000_000_000).unwrap_or(0)
            }
            None => u64::try_from(gap).unwrap_or(0),
        };
        Ok(frames)
    }
}

/// A streaming decoder that gives stereo interleaved f32 frames at 48 kHz. It takes the
/// first two channels of a file with more, and copies mono to both channels. It resamples
/// other rates with the rubato FFT resampler.
pub struct Decoder {
    path: PathBuf,
    source: Source,
    skip: u64,
    scratch: Vec<f32>,
    stereo: Vec<f32>,
    resampler: Option<Resampling>,
    finished: bool,
}

impl Decoder {
    /// Opens a file and starts at its beginning.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        Self::open_at(path, Duration::ZERO)
    }

    /// Opens a file and starts at `cue_in`, with sample accuracy.
    pub fn open_at(path: impl AsRef<Path>, cue_in: Duration) -> Result<Self, AudioError> {
        let path = path.as_ref();
        let source = Source::open(path)?;
        Self::from_source(path, source, cue_in, true)
    }

    fn from_source(
        path: &Path,
        mut source: Source,
        cue_in: Duration,
        resample: bool,
    ) -> Result<Self, AudioError> {
        let skip = if cue_in.is_zero() {
            0
        } else {
            source.seek(cue_in)?
        };
        let resampler = if resample && source.sample_rate != SAMPLE_RATE {
            Some(Resampling::new(source.sample_rate)?)
        } else {
            None
        };
        Ok(Self {
            path: path.to_path_buf(),
            source,
            skip,
            scratch: Vec::new(),
            stereo: Vec::new(),
            resampler,
            finished: false,
        })
    }

    /// Returns the sample rate of the file.
    pub fn source_rate(&self) -> u32 {
        self.source.sample_rate
    }

    /// Returns the file that this decoder reads.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decodes one or more packets and appends their frames to `out`. It returns `false` at
    /// the end of the stream, and that last call can still append frames.
    pub fn read(&mut self, out: &mut Vec<f32>) -> Result<bool, AudioError> {
        if self.finished {
            return Ok(false);
        }
        let start = out.len();
        while out.len() == start {
            let packet = match self.source.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(SymError::ResetRequired) => return self.finish(out),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return self.finish(out);
                }
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            };
            if packet.track_id != self.source.track_id {
                continue;
            }
            match self.source.decoder.decode(&packet) {
                Ok(buf) => buf.copy_to_vec_interleaved(&mut self.scratch),
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            }
            self.convert_to_stereo();
            match &mut self.resampler {
                Some(r) => r.push(&self.stereo, out)?,
                None => out.extend_from_slice(&self.stereo),
            }
        }
        Ok(true)
    }

    /// Decodes the rest of the file into one buffer.
    pub fn read_to_end(mut self) -> Result<Vec<f32>, AudioError> {
        let mut out = Vec::new();
        while self.read(&mut out)? {}
        Ok(out)
    }

    fn finish(&mut self, out: &mut Vec<f32>) -> Result<bool, AudioError> {
        self.finished = true;
        if let Some(r) = &mut self.resampler {
            r.finish(out)?;
        }
        Ok(false)
    }

    /// Converts `scratch` (source channels) to `stereo`, and drops the cue frames.
    fn convert_to_stereo(&mut self) {
        let ch = self.source.channels.max(1);
        self.stereo.clear();
        for frame in self.scratch.chunks_exact(ch) {
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            let l = frame.first().copied().unwrap_or(0.0);
            let r = frame.get(1).copied().unwrap_or(l);
            self.stereo.push(l);
            self.stereo.push(r);
        }
    }
}

/// A fixed-ratio stream resampler to 48 kHz that trims the resampler delay.
struct Resampling {
    fft: Box<dyn Resampler<f32>>,
    rate: u32,
    pending: Vec<f32>,
    out_buf: Vec<f32>,
    to_trim: usize,
    in_frames: u64,
    out_frames: u64,
}

impl Resampling {
    const CHUNK: usize = 1024;
    /// The FFT resampler sizes its blocks from `rate / gcd(rate, 48000)`. Above this
    /// value the resampler uses a polynomial resampler with a fixed chunk.
    const MAX_FFT_BLOCK: usize = 2048;

    fn new(rate: u32) -> Result<Self, AudioError> {
        let rate_in = rate as usize;
        let rate_out = SAMPLE_RATE as usize;
        let block = rate_in / gcd(rate_in, rate_out).max(1);
        let fft: Box<dyn Resampler<f32>> = if block <= Self::MAX_FFT_BLOCK {
            Box::new(
                Fft::<f32>::new(rate_in, rate_out, Self::CHUNK, CHANNELS, FixedSync::Input)
                    .map_err(|e| AudioError::Resample(e.to_string()))?,
            )
        } else {
            Box::new(
                Async::<f32>::new_poly(
                    rate_out as f64 / rate_in as f64,
                    1.0,
                    PolynomialDegree::Cubic,
                    Self::CHUNK,
                    CHANNELS,
                    FixedAsync::Input,
                )
                .map_err(|e| AudioError::Resample(e.to_string()))?,
            )
        };
        let to_trim = fft.output_delay();
        let out_buf = vec![0.0; fft.output_frames_max() * CHANNELS];
        Ok(Self {
            fft,
            rate,
            pending: Vec::new(),
            out_buf,
            to_trim,
            in_frames: 0,
            out_frames: 0,
        })
    }

    fn push(&mut self, stereo: &[f32], out: &mut Vec<f32>) -> Result<(), AudioError> {
        self.pending.extend_from_slice(stereo);
        self.in_frames += (stereo.len() / CHANNELS) as u64;
        while self.pending.len() / CHANNELS >= self.fft.input_frames_next() {
            self.process(u64::MAX, out)?;
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), AudioError> {
        let expected = (u128::from(self.in_frames) * u128::from(SAMPLE_RATE))
            .div_ceil(u128::from(self.rate.max(1)));
        let expected = u64::try_from(expected).unwrap_or(u64::MAX);
        // The loop has a bound, so a resampler fault cannot loop for ever.
        for _ in 0..64 {
            if self.out_frames >= expected {
                break;
            }
            self.process(expected, out)?;
        }
        Ok(())
    }

    /// Resamples one chunk from `pending` with silence as padding. The total output stays
    /// at `limit` frames or less.
    fn process(&mut self, limit: u64, out: &mut Vec<f32>) -> Result<(), AudioError> {
        let need = self.fft.input_frames_next();
        let have = (self.pending.len() / CHANNELS).min(need);
        self.pending
            .resize(self.pending.len().max(need * CHANNELS), 0.0);
        let cap = self.out_buf.len() / CHANNELS;
        let input = InterleavedSlice::new(&self.pending[..need * CHANNELS], CHANNELS, need)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        let mut output = InterleavedSlice::new_mut(&mut self.out_buf, CHANNELS, cap)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        let (_, written) = self
            .fft
            .process_into_buffer(&input, &mut output, None)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        self.pending.drain(..need * CHANNELS);
        if have < need {
            // The padding was silence, not input.
            self.pending.clear();
        }
        let trim = self.to_trim.min(written);
        self.to_trim -= trim;
        let room = usize::try_from(limit.saturating_sub(self.out_frames)).unwrap_or(usize::MAX);
        let keep = (written - trim).min(room);
        if let Some(frames) = self.out_buf.get(trim * CHANNELS..(trim + keep) * CHANNELS) {
            out.extend_from_slice(frames);
        }
        self.out_frames += keep as u64;
        Ok(())
    }
}

/// An in-memory clip for the cartwall: stereo interleaved f32 at 48 kHz.
#[derive(Clone, Debug)]
pub struct Clip(Arc<[f32]>);

impl Clip {
    /// Decodes a whole file into a clip.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        Ok(Self::from_interleaved(Decoder::open(path)?.read_to_end()?))
    }

    /// Makes a clip from stereo interleaved samples at 48 kHz. It drops an odd last sample.
    pub fn from_interleaved(mut samples: Vec<f32>) -> Self {
        samples.truncate(samples.len() / CHANNELS * CHANNELS);
        Self(samples.into())
    }

    /// Returns the number of frames.
    pub fn frames(&self) -> usize {
        self.0.len() / CHANNELS
    }

    pub(crate) fn samples(&self) -> &[f32] {
        &self.0
    }
}
