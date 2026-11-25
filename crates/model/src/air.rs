//! Whether the station is on air: sending Program to Icecast and WebRTC.

/// The settings part of the On air / Off air switch.
pub const AIR_PART: &str = "air";
/// The key of the switch in [`AIR_PART`], a bool.
pub const ON_AIR_KEY: &str = "on_air";

/// Whether the station is on air, from the stored setting. A station that never chose is
/// on air, so an old station keeps sending after an update.
pub fn on_air(stored: Option<bool>) -> bool {
    stored.unwrap_or(true)
}

/// The settings part of the Icecast push.
pub const ICECAST_PART: &str = "icecast";
/// The key of the Icecast switch in [`ICECAST_PART`], a bool. On air is the switch of the
/// whole station; this one is Icecast's own.
pub const ICECAST_ENABLED_KEY: &str = "enabled";

/// Whether Icecast is switched on, from the stored setting. A station that never chose
/// streams to Icecast, so an old station keeps sending after an update.
pub fn icecast_enabled(stored: Option<bool>) -> bool {
    stored.unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a stored `false` takes the station off air.
    #[test]
    fn only_a_stored_false_is_off_air() {
        assert!(on_air(None));
        assert!(on_air(Some(true)));
        assert!(!on_air(Some(false)));
    }
}
