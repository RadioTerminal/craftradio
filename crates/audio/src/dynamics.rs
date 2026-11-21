//! Program dynamics: an optional compressor and a lookahead limiter, from bisque.
//!
//! Only this module knows about bisque. The mixer gives it blocks of stereo frames and
//! gets them back processed, [`Chain::latency`] frames late.

use bisque::dynamics::{Compressor, CompressorSettings};
use bisque::host::PreparedProcessor;
use bisque::mastering::{Limiter, LimiterSettings};
use bisque::processor::{DspError, KernelProcessor, ProcessSpec};

use radio_model::DynamicsSettings;

use crate::{AudioError, CHANNELS, SAMPLE_RATE};

/// The settings of the compressor on Program. It detects both channels together.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CompressorConfig {
    /// The level where compression starts, in dBFS.
    pub threshold_db: f32,
    /// The ratio, as `ratio:1`. 1 is no compression.
    pub ratio: f32,
    /// The attack time, in ms.
    pub attack_ms: f32,
    /// The release time, in ms.
    pub release_ms: f32,
    /// The gain after compression, in dB.
    pub makeup_db: f32,
}

impl Default for CompressorConfig {
    /// The compressor of [`radio_model::CompressorSettings::default`], the one home of the
    /// defaults.
    fn default() -> Self {
        let c = radio_model::CompressorSettings::default();
        Self {
            threshold_db: c.threshold_db,
            ratio: c.ratio,
            attack_ms: c.attack_ms,
            release_ms: c.release_ms,
            makeup_db: c.makeup_db,
        }
    }
}

/// The settings of the lookahead limiter on Program.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LimiterConfig {
    /// The highest true peak on Program, in dBFS.
    pub ceiling_db: f32,
    /// The lookahead, in ms. Program comes out this much later.
    pub lookahead_ms: f32,
    /// The release time, in ms.
    pub release_ms: f32,
}

impl LimiterConfig {
    /// The lookahead of the limiter, in ms. It does not change, so Program has one latency.
    pub const LOOKAHEAD_MS: f32 = 1.5;
}

impl Default for LimiterConfig {
    /// The limiter of [`radio_model::LimiterSettings::default`].
    fn default() -> Self {
        let l = radio_model::LimiterSettings::default();
        Self {
            ceiling_db: l.ceiling_db,
            lookahead_ms: Self::LOOKAHEAD_MS,
            release_ms: l.release_ms,
        }
    }
}

/// The processing on Program: a compressor if it has one, then the limiter if it has one.
/// Without a limiter, a soft clip keeps Program in range.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ProgramDynamics {
    /// The compressor, or `None` for no compressor.
    pub compressor: Option<CompressorConfig>,
    /// The limiter, or `None` for the soft clip.
    pub limiter: Option<LimiterConfig>,
}

impl Default for ProgramDynamics {
    /// The limiter alone.
    fn default() -> Self {
        Self {
            compressor: None,
            limiter: Some(LimiterConfig::default()),
        }
    }
}

impl From<&DynamicsSettings> for ProgramDynamics {
    fn from(d: &DynamicsSettings) -> Self {
        let c = d.compressor;
        let l = d.limiter;
        Self {
            compressor: c.enabled.then_some(CompressorConfig {
                threshold_db: c.threshold_db,
                ratio: c.ratio,
                attack_ms: c.attack_ms,
                release_ms: c.release_ms,
                makeup_db: c.makeup_db,
            }),
            limiter: l.enabled.then_some(LimiterConfig {
                ceiling_db: l.ceiling_db,
                lookahead_ms: LimiterConfig::LOOKAHEAD_MS,
                release_ms: l.release_ms,
            }),
        }
    }
}

type Prepared<K> = PreparedProcessor<KernelProcessor<K, f32>, f32>;

fn dsp_error(e: DspError) -> AudioError {
    AudioError::Dynamics(e.to_string())
}

/// The prepared processors and their scratch planes. Only [`Chain::new`] allocates.
pub(crate) struct Chain {
    compressor: Option<Prepared<Compressor>>,
    limiter: Option<Prepared<Limiter>>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Chain {
    /// The most frames that one bisque block holds.
    pub(crate) const MAX_BLOCK: usize = 512;

    /// Prepares the processors. It fails if a setting is out of range.
    pub(crate) fn new(config: ProgramDynamics) -> Result<Self, AudioError> {
        let spec = ProcessSpec {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            max_block: Self::MAX_BLOCK,
            max_memory: None,
        };
        let compressor = config
            .compressor
            .map(|c| {
                let settings = CompressorSettings::new()
                    .threshold_db(f64::from(c.threshold_db))
                    .ratio(f64::from(c.ratio))
                    .attack_ms(f64::from(c.attack_ms))
                    .release_ms(f64::from(c.release_ms))
                    .makeup_db(f64::from(c.makeup_db));
                PreparedProcessor::prepare_kernel(Compressor::with_settings(settings), spec)
            })
            .transpose()
            .map_err(dsp_error)?;
        let limiter = config
            .limiter
            .map(|l| {
                let settings = LimiterSettings::new()
                    .threshold_db(f64::from(l.ceiling_db))
                    .lookahead_ms(f64::from(l.lookahead_ms))
                    .release_ms(f64::from(l.release_ms));
                PreparedProcessor::prepare_kernel(Limiter::with_settings(settings), spec)
            })
            .transpose()
            .map_err(dsp_error)?;
        Ok(Self {
            compressor,
            limiter,
            left: vec![0.0; Self::MAX_BLOCK],
            right: vec![0.0; Self::MAX_BLOCK],
        })
    }

    /// How many frames late Program comes out.
    pub(crate) fn latency(&self) -> usize {
        self.compressor.as_ref().map_or(0, Prepared::latency)
            + self.limiter.as_ref().map_or(0, Prepared::latency)
    }

    /// True when the chain has a limiter. Without one, the mixer soft-clips Program.
    pub(crate) fn has_limiter(&self) -> bool {
        self.limiter.is_some()
    }

    /// Processes stereo frames in place, in parts of at most [`Chain::MAX_BLOCK`]. It
    /// does not allocate.
    pub(crate) fn process(&mut self, frames: &mut [[f32; CHANNELS]]) {
        for block in frames.chunks_mut(Self::MAX_BLOCK) {
            let n = block.len();
            let (Some(left), Some(right)) = (self.left.get_mut(..n), self.right.get_mut(..n))
            else {
                return;
            };
            for ((l, r), f) in left.iter_mut().zip(right.iter_mut()).zip(block.iter()) {
                *l = f[0];
                *r = f[1];
            }
            if let Some(c) = &mut self.compressor {
                c.process_in_place(&mut [&mut *left, &mut *right], &[]);
            }
            if let Some(l) = &mut self.limiter {
                l.process_in_place(&mut [&mut *left, &mut *right], &[]);
            }
            for ((l, r), f) in left.iter().zip(right.iter()).zip(block.iter_mut()) {
                *f = [*l, *r];
            }
        }
    }
}
