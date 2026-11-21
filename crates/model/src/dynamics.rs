//! The settings of the processing on Program: a compressor, then a lookahead limiter.
//!
//! This is the one home of the defaults and of the ranges that the engine accepts. The
//! audio crate turns these settings into its processors. The ranges sit inside the ranges
//! that the processors take.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The compressor on Program. It detects both channels together, with a hard knee.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CompressorSettings {
    /// True when the compressor runs.
    pub enabled: bool,
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

impl Default for CompressorSettings {
    /// Off, with a gentle broadcast setting for material at about −18 LUFS.
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -20.0,
            ratio: 2.5,
            attack_ms: 10.0,
            release_ms: 200.0,
            makeup_db: 0.0,
        }
    }
}

/// The lookahead limiter on Program. The lookahead does not change, so Program has one
/// latency.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LimiterSettings {
    /// True when the limiter runs. Without it, a soft clip keeps Program in range.
    pub enabled: bool,
    /// The highest true peak on Program, in dBFS.
    pub ceiling_db: f32,
    /// The release time, in ms.
    pub release_ms: f32,
}

impl Default for LimiterSettings {
    /// On, with the ceiling at −1 dBFS true peak.
    fn default() -> Self {
        Self {
            enabled: true,
            ceiling_db: -1.0,
            release_ms: 50.0,
        }
    }
}

/// The processing on Program.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DynamicsSettings {
    pub compressor: CompressorSettings,
    pub limiter: LimiterSettings,
}

/// The ranges that the engine accepts. Both ends are in the range.
pub mod range {
    pub const THRESHOLD_DB: (f32, f32) = (-60.0, 0.0);
    pub const RATIO: (f32, f32) = (1.0, 20.0);
    pub const ATTACK_MS: (f32, f32) = (0.0, 500.0);
    pub const COMP_RELEASE_MS: (f32, f32) = (1.0, 5000.0);
    pub const MAKEUP_DB: (f32, f32) = (0.0, 24.0);
    pub const CEILING_DB: (f32, f32) = (-30.0, 0.0);
    pub const LIMIT_RELEASE_MS: (f32, f32) = (1.0, 5000.0);
}

fn check(name: &str, unit: &str, v: f32, (lo, hi): (f32, f32)) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{name} takes {lo} to {hi} {unit}, not {v}"))
    }
}

impl DynamicsSettings {
    /// Checks each value against its range. The message names the first bad value.
    pub fn validate(&self) -> Result<(), String> {
        let c = &self.compressor;
        check(
            "the compressor threshold",
            "dBFS",
            c.threshold_db,
            range::THRESHOLD_DB,
        )?;
        check("the compressor ratio", ":1", c.ratio, range::RATIO)?;
        check("the compressor attack", "ms", c.attack_ms, range::ATTACK_MS)?;
        check(
            "the compressor release",
            "ms",
            c.release_ms,
            range::COMP_RELEASE_MS,
        )?;
        check(
            "the compressor makeup gain",
            "dB",
            c.makeup_db,
            range::MAKEUP_DB,
        )?;
        let l = &self.limiter;
        check(
            "the limiter ceiling",
            "dBFS",
            l.ceiling_db,
            range::CEILING_DB,
        )?;
        check(
            "the limiter release",
            "ms",
            l.release_ms,
            range::LIMIT_RELEASE_MS,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_valid() {
        assert_eq!(DynamicsSettings::default().validate(), Ok(()));
    }

    #[test]
    fn values_out_of_range_or_not_finite_are_refused() {
        let mut d = DynamicsSettings::default();
        d.compressor.ratio = 0.5;
        assert!(d.validate().is_err(), "a ratio under 1");
        let mut d = DynamicsSettings::default();
        d.limiter.ceiling_db = 3.0;
        assert!(d.validate().is_err(), "a ceiling over full scale");
        let mut d = DynamicsSettings::default();
        d.compressor.attack_ms = f32::NAN;
        assert!(d.validate().is_err(), "a NaN attack");
    }
}
