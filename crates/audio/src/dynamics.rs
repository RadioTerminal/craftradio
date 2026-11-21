//! Program dynamics: an optional compressor and a lookahead limiter, from bisque.
//!
//! Only this module knows about bisque. The mixer gives it blocks of stereo frames and
//! gets them back processed, [`Chain::latency`] frames late.

use bisque::dynamics::{Compressor, CompressorSettings};
use bisque::host::PreparedProcessor;
use bisque::mastering::{Limiter, LimiterSettings};
use bisque::processor::{DspError, KernelProcessor, ProcessSpec};

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
    /// A gentle broadcast setting for material at about −18 LUFS.
    fn default() -> Self {
        Self {
            threshold_db: -20.0,
            ratio: 2.5,
            attack_ms: 10.0,
            release_ms: 200.0,
            makeup_db: 0.0,
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

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            ceiling_db: -1.0,
            lookahead_ms: 1.5,
            release_ms: 50.0,
        }
    }
}

/// The processing on Program: a compressor if it has one, then the limiter.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ProgramDynamics {
    /// The compressor, or `None` for no compressor.
    pub compressor: Option<CompressorConfig>,
    /// The limiter.
    pub limiter: LimiterConfig,
}

type Prepared<K> = PreparedProcessor<KernelProcessor<K, f32>, f32>;

fn dsp_error(e: DspError) -> AudioError {
    AudioError::Dynamics(e.to_string())
}

/// The prepared processors and their scratch planes. Only [`Chain::new`] allocates.
pub(crate) struct Chain {
    compressor: Option<Prepared<Compressor>>,
    limiter: Prepared<Limiter>,
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
        let l = config.limiter;
        let settings = LimiterSettings::new()
            .threshold_db(f64::from(l.ceiling_db))
            .lookahead_ms(f64::from(l.lookahead_ms))
            .release_ms(f64::from(l.release_ms));
        let limiter = PreparedProcessor::prepare_kernel(Limiter::with_settings(settings), spec)
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
        self.compressor.as_ref().map_or(0, Prepared::latency) + self.limiter.latency()
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
            self.limiter
                .process_in_place(&mut [&mut *left, &mut *right], &[]);
            for ((l, r), f) in left.iter().zip(right.iter()).zip(block.iter_mut()) {
                *f = [*l, *r];
            }
        }
    }
}
