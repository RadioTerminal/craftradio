//! Loudness normalisation of deck items (spec §6.5, Loudness). This module holds the
//! station setting and the loudness of a file. It also holds the rule for the item gain.
//!
//! This is the one home of the defaults, the ranges and the gain rule. Playout applies the
//! gain on the deck, and the UI shows the same numbers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The highest true peak that a boost may reach, in dBTP. A boost never lifts the peak of a
/// file over this value. Thus our own gain does not drive the limiter.
pub const PEAK_CEILING_DB: f32 = -1.0;
/// The deepest cut, in dB.
pub const MAX_CUT_DB: f32 = -24.0;

/// The ranges that the engine accepts. Both ends are in the range.
pub mod range {
    pub const TARGET_LUFS: (f32, f32) = (-30.0, -10.0);
    pub const MAX_BOOST_DB: (f32, f32) = (0.0, 12.0);
    /// The manual trim of one item, on top of its measured gain.
    pub const TRIM_DB: (f32, f32) = (-12.0, 12.0);
}

/// Loudness normalisation on the decks, for the whole station.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LoudnessSettings {
    /// True when each deck item gets the gain that brings it to the target.
    pub enabled: bool,
    /// The integrated loudness that each item gets to, in LUFS.
    pub target_lufs: f32,
    /// The largest boost, in dB.
    pub max_boost_db: f32,
}

impl Default for LoudnessSettings {
    /// On, at −18 LUFS, with at most +12 dB of boost.
    fn default() -> Self {
        Self {
            enabled: true,
            target_lufs: -18.0,
            max_boost_db: 12.0,
        }
    }
}

fn check(name: &str, unit: &str, v: f32, (lo, hi): (f32, f32)) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{name} takes {lo} to {hi} {unit}, not {v}"))
    }
}

/// Checks a manual item trim against [`range::TRIM_DB`].
pub fn check_trim(db: f32) -> Result<(), String> {
    check("the item trim", "dB", db, range::TRIM_DB)
}

impl LoudnessSettings {
    /// Checks each value against its range. The message names the first bad value.
    pub fn validate(&self) -> Result<(), String> {
        check(
            "the loudness target",
            "LUFS",
            self.target_lufs,
            range::TARGET_LUFS,
        )?;
        check(
            "the loudness max boost",
            "dB",
            self.max_boost_db,
            range::MAX_BOOST_DB,
        )
    }

    /// The gain of an item. It is the step from its loudness to the target, plus the trim.
    /// Then the clamps apply. An item with no known loudness plays at unity. With
    /// normalisation off, each item plays at unity.
    pub fn gain(&self, known: Option<(Loudness, GainSource)>, trim_db: f32) -> ItemGain {
        let unity = |source| ItemGain {
            db: 0.0,
            source,
            loudness: known.map(|(l, _)| l),
            target_lufs: self.target_lufs,
            trim_db,
            clamp: None,
        };
        if !self.enabled {
            return unity(GainSource::Off);
        }
        let Some((loudness, source)) = known else {
            return unity(GainSource::NotMeasured);
        };
        let mut db = self.target_lufs - loudness.lufs + trim_db;
        let mut clamp = None;
        if db > 0.0 {
            // The peak guard limits a boost only. It never turns a boost into a cut.
            match loudness.peak_db.map(|p| (PEAK_CEILING_DB - p).max(0.0)) {
                Some(room) if db > room => {
                    db = room;
                    clamp = Some(Clamp::Peak);
                }
                Some(_) => {}
                None => {
                    db = 0.0;
                    clamp = Some(Clamp::NoPeak);
                }
            }
            if db > self.max_boost_db {
                db = self.max_boost_db;
                clamp = Some(Clamp::MaxBoost);
            }
        }
        if db < MAX_CUT_DB {
            db = MAX_CUT_DB;
            clamp = Some(Clamp::MaxCut);
        }
        ItemGain {
            db,
            source,
            loudness: Some(loudness),
            target_lufs: self.target_lufs,
            trim_db,
            clamp,
        }
    }
}

/// The loudness of a file: an EBU R128 measurement, or the value of its tags.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Loudness {
    /// The integrated loudness, in LUFS.
    pub lufs: f32,
    /// The true peak in dBTP (or the tagged peak). `None` when a tag gives no peak.
    pub peak_db: Option<f32>,
}

/// Where the loudness of an item comes from.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum GainSource {
    /// An EBU R128 measurement of this station.
    Measured,
    /// A ReplayGain or R128 tag of the file. It applies until a measurement comes.
    Tag,
    /// Nothing known yet: the item plays at unity.
    #[default]
    NotMeasured,
    /// Normalisation is off: the item plays at unity.
    Off,
}

/// Why the gain is less than the loudness asks for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Clamp {
    /// A larger boost would lift the true peak over [`PEAK_CEILING_DB`].
    Peak,
    /// The tag gives no peak, so the item gets no boost.
    NoPeak,
    /// The boost is at the max boost of the settings.
    MaxBoost,
    /// The cut is at [`MAX_CUT_DB`].
    MaxCut,
}

/// The gain of one deck item, and the values that made it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ItemGain {
    /// The gain in dB. 0 is unity.
    pub db: f32,
    pub source: GainSource,
    pub loudness: Option<Loudness>,
    pub target_lufs: f32,
    /// The manual trim of the item, in dB. `db` includes it.
    pub trim_db: f32,
    pub clamp: Option<Clamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(lufs: f32, peak: f32) -> Option<(Loudness, GainSource)> {
        Some((
            Loudness {
                lufs,
                peak_db: Some(peak),
            },
            GainSource::Measured,
        ))
    }

    #[test]
    fn the_defaults_are_valid() {
        assert_eq!(LoudnessSettings::default().validate(), Ok(()));
    }

    #[test]
    fn values_out_of_range_or_not_finite_are_refused() {
        let s = LoudnessSettings {
            target_lufs: -5.0,
            ..LoudnessSettings::default()
        };
        assert!(s.validate().is_err(), "a target over the range");
        let s = LoudnessSettings {
            max_boost_db: f32::NAN,
            ..LoudnessSettings::default()
        };
        assert!(s.validate().is_err(), "a NaN boost");
        assert!(check_trim(30.0).is_err());
        assert!(check_trim(-2.5).is_ok());
    }

    #[test]
    fn a_quiet_item_is_boosted_and_a_loud_one_cut_to_the_target() {
        let s = LoudnessSettings::default();
        let quiet = s.gain(measured(-26.0, -12.0), 0.0);
        let loud = s.gain(measured(-9.0, -0.1), 0.0);
        assert!(quiet.db > 0.0 && loud.db < 0.0);
        assert!((-26.0 + quiet.db - s.target_lufs).abs() < 0.01);
        assert!((-9.0 + loud.db - s.target_lufs).abs() < 0.01);
        assert_eq!(quiet.clamp, None);
        assert_eq!(quiet.source, GainSource::Measured);
    }

    #[test]
    fn a_boost_never_lifts_the_peak_over_the_ceiling() {
        let s = LoudnessSettings::default();
        let g = s.gain(measured(-30.0, -4.0), 0.0);
        assert!(g.db > 0.0, "some boost is left");
        assert!(-4.0 + g.db <= PEAK_CEILING_DB + 1e-4);
        assert_eq!(g.clamp, Some(Clamp::Peak));
        // A file that already peaks over the ceiling gets no boost, and no cut either.
        let g = s.gain(measured(-30.0, 0.0), 0.0);
        assert_eq!(g.db, 0.0);
    }

    #[test]
    fn boost_and_cut_stay_within_their_caps() {
        let s = LoudnessSettings {
            max_boost_db: 6.0,
            ..LoudnessSettings::default()
        };
        let g = s.gain(measured(-40.0, -40.0), 0.0);
        assert!(g.db > 0.0 && g.db <= 6.0 && g.clamp == Some(Clamp::MaxBoost));
        let g = s.gain(measured(10.0, 0.0), 0.0);
        assert!(g.db >= MAX_CUT_DB && g.clamp == Some(Clamp::MaxCut));
    }

    #[test]
    fn the_trim_adds_to_the_gain_and_the_clamps_still_hold() {
        let s = LoudnessSettings::default();
        let plain = s.gain(measured(-20.0, -10.0), 0.0);
        let trimmed = s.gain(measured(-20.0, -10.0), -3.0);
        assert!(trimmed.db < plain.db);
        let pushed = s.gain(measured(-20.0, -10.0), 12.0);
        assert!(pushed.db > plain.db);
        assert!(-10.0 + pushed.db <= PEAK_CEILING_DB + 1e-4);
    }

    #[test]
    fn unknown_or_off_plays_at_unity() {
        let s = LoudnessSettings::default();
        let g = s.gain(None, 4.0);
        assert_eq!((g.db, g.source), (0.0, GainSource::NotMeasured));
        let off = LoudnessSettings {
            enabled: false,
            ..s
        };
        let g = off.gain(measured(-30.0, -20.0), 0.0);
        assert_eq!((g.db, g.source), (0.0, GainSource::Off));
    }

    #[test]
    fn a_tag_without_a_peak_gets_no_boost() {
        let s = LoudnessSettings::default();
        let tag = Some((
            Loudness {
                lufs: -24.0,
                peak_db: None,
            },
            GainSource::Tag,
        ));
        let g = s.gain(tag, 0.0);
        assert_eq!((g.db, g.clamp), (0.0, Some(Clamp::NoPeak)));
        let loud = Some((
            Loudness {
                lufs: -12.0,
                peak_db: None,
            },
            GainSource::Tag,
        ));
        assert!(s.gain(loud, 0.0).db < 0.0, "a cut needs no peak");
    }
}
