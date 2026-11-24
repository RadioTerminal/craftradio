//! How many of the four decks the station uses.

/// The settings part of the decks.
pub const DECKS_PART: &str = "decks";
/// The key of the number of decks in [`DECKS_PART`].
pub const DECK_COUNT_KEY: &str = "count";
/// The mixer has four decks.
pub const MAX_DECKS: usize = 4;
/// A segue needs the next item loaded while one plays.
pub const MIN_DECKS: usize = 2;
/// The number of decks of a station that never chose one.
pub const DEFAULT_DECKS: usize = 2;

/// The number of decks in use, from the stored setting. A missing value is the default,
/// and a value out of range is the nearest allowed one.
pub fn deck_count(stored: Option<u64>) -> usize {
    stored.map_or(DEFAULT_DECKS, |n| {
        usize::try_from(n)
            .unwrap_or(MAX_DECKS)
            .clamp(MIN_DECKS, MAX_DECKS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The playout divides by the number, and keeps one deck for the item on air. So a
    /// stored value that is out of range never comes back as it is.
    #[test]
    fn a_stored_number_out_of_range_becomes_an_allowed_one() {
        for stored in [0, 1, 5, u64::MAX] {
            let n = deck_count(Some(stored));
            assert!((MIN_DECKS..=MAX_DECKS).contains(&n), "{stored} gave {n}");
        }
        assert_eq!(deck_count(Some(3)), 3);
        assert_eq!(deck_count(None), DEFAULT_DECKS);
    }
}
