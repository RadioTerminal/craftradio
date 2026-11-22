//! A level meter that an output feeds with the samples it really sends.

use std::sync::atomic::{AtomicU32, Ordering};

/// The level that a meter reads for silence, in dBFS.
pub const LEVEL_FLOOR_DB: f32 = -120.0;

/// The loudest sample per channel since the last [`LevelMeter::take`]. The output thread
/// adds, the UI takes; neither blocks nor allocates.
#[derive(Debug, Default)]
pub struct LevelMeter {
    peak: [AtomicU32; 2],
}

impl LevelMeter {
    /// Adds interleaved stereo samples.
    pub fn add(&self, interleaved_stereo: &[f32]) {
        let mut loudest = [0.0f32; 2];
        for frame in interleaved_stereo.as_chunks::<2>().0 {
            for (l, x) in loudest.iter_mut().zip(frame) {
                if x.is_finite() {
                    *l = l.max(x.abs());
                }
            }
        }
        // Non-negative floats order like their bits, so the bits can take the maximum.
        for (slot, l) in self.peak.iter().zip(loudest) {
            slot.fetch_max(l.to_bits(), Ordering::Relaxed);
        }
    }

    /// The loudest sample per channel since the last take, in dBFS. It starts over at
    /// silence, so an output that sends nothing reads [`LEVEL_FLOOR_DB`].
    pub fn take(&self) -> [f32; 2] {
        self.peak.each_ref().map(|slot| {
            let lin = f32::from_bits(slot.swap(0, Ordering::Relaxed));
            if lin <= 1e-6 {
                LEVEL_FLOOR_DB
            } else {
                (20.0 * lin.log10()).max(LEVEL_FLOOR_DB)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_reports_the_loudest_sample_per_channel_then_falls_to_silence() {
        let m = LevelMeter::default();
        m.add(&[0.5, 0.25, -1.0, 0.1]);
        let [l, r] = m.take();
        assert!(l.abs() < 0.01, "{l}");
        assert!((r + 12.04).abs() < 0.01, "{r}");
        assert_eq!(m.take(), [LEVEL_FLOOR_DB; 2]);
    }

    #[test]
    fn a_meter_that_got_nothing_reads_silence() {
        assert_eq!(LevelMeter::default().take(), [LEVEL_FLOOR_DB; 2]);
    }
}
