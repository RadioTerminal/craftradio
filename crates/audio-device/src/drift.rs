use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Adjustable, Async, FixedAsync, PolynomialDegree, Resampler as _};

use crate::DeviceError;

/// Largest relative deviation from the nominal ratio the controller asks for (0.5 %, far more
/// than any sound card drifts).
const MAX_DEVIATION: f64 = 0.005;
const KP: f64 = 0.002;
const KI: f64 = 0.0001;

/// Resampling ratio (output rate / input rate, around 1.0) for a secondary device whose ring
/// buffer is `buffer_fill` full against a `target` fill (both as fractions or frame counts in
/// the same unit). Below target the ratio goes above 1 (stretch to consume less input), above
/// target below 1. This is the proportional part; [`PiController`] adds the integral part.
pub fn ratio_for(buffer_fill: f32, target: f32) -> f64 {
    PiController::new().update(buffer_fill, target)
}

/// PI controller turning buffer fill error into a resampling ratio.
#[derive(Debug, Clone, Default)]
pub struct PiController {
    integral: f64,
}

impl PiController {
    /// A controller with no accumulated error.
    pub fn new() -> Self {
        Self::default()
    }

    /// One control step. Returns the ratio to set on the [`Resampler`].
    pub fn update(&mut self, buffer_fill: f32, target: f32) -> f64 {
        let target = f64::from(target);
        if !(target.is_finite() && target > 0.0 && buffer_fill.is_finite()) {
            return 1.0;
        }
        let error = (target - f64::from(buffer_fill)) / target;
        self.integral = (self.integral + error).clamp(-10.0, 10.0);
        (1.0 + KP * error + KI * self.integral).clamp(1.0 - MAX_DEVIATION, 1.0 + MAX_DEVIATION)
    }
}

/// A resampler with an adjustable ratio, for matching a secondary device's clock.
pub struct Resampler {
    inner: Async<f32>,
    channels: usize,
    chunk: usize,
    nominal: f64,
}

impl Resampler {
    /// `nominal_ratio` is output rate / input rate; `chunk_frames` is the input block size.
    pub fn new(
        nominal_ratio: f64,
        channels: usize,
        chunk_frames: usize,
    ) -> Result<Self, DeviceError> {
        let inner = Async::<f32>::new_poly(
            nominal_ratio,
            1.1,
            PolynomialDegree::Cubic,
            chunk_frames,
            channels,
            FixedAsync::Input,
        )
        .map_err(|e| DeviceError::Resample(e.to_string()))?;
        Ok(Self {
            inner,
            channels,
            chunk: chunk_frames,
            nominal: nominal_ratio,
        })
    }

    /// Adjust the ratio relative to nominal (`1.0` = nominal, e.g. from [`ratio_for`]).
    pub fn set_relative_ratio(&mut self, relative: f64) -> Result<(), DeviceError> {
        self.inner
            .set_resample_ratio(self.nominal * relative.clamp(1.0 / 1.1, 1.1), true)
            .map_err(|e| DeviceError::Resample(e.to_string()))
    }

    /// Input block size in frames; `process` needs exactly this many.
    pub fn chunk_frames(&self) -> usize {
        self.chunk
    }

    /// Resample one interleaved block of `chunk_frames` frames, appending to `output`.
    /// Returns the number of frames produced.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<usize, DeviceError> {
        let err = |e: &dyn std::fmt::Display| DeviceError::Resample(e.to_string());
        let in_buf =
            InterleavedSlice::new(input, self.channels, self.chunk).map_err(|e| err(&e))?;
        let max = self.inner.output_frames_max();
        let mut scratch = vec![0.0f32; max * self.channels];
        let mut out_buf =
            InterleavedSlice::new_mut(&mut scratch, self.channels, max).map_err(|e| err(&e))?;
        let (_, produced) = self
            .inner
            .process_into_buffer(&in_buf, &mut out_buf, None)
            .map_err(|e| err(&e))?;
        output.extend_from_slice(&scratch[..produced * self.channels]);
        Ok(produced)
    }
}
