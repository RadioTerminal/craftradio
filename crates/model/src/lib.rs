//! Pure serde data model for the radio station: items, playlists, rules and constraints.

mod error;
mod ids;
mod item;
mod playlist;
mod rule;
mod station;

pub use error::ModelError;
pub use ids::{ItemId, PlaylistId};
pub use item::{Item, Source, Timing};
pub use playlist::{PickOrder, Playlist};
pub use rule::{Constraint, FixMode, Placement, Repeat, Rule, Trigger};
pub use station::Station;
