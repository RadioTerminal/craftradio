//! The egui desktop UI: the view-model, a mock source for the preview, and the panels.

mod app;
mod cartwall;
mod decks;
mod main_pane;
pub mod mock;
mod scheduled_pane;
pub mod theme;
pub mod view;

pub use app::RadioApp;
pub use mock::MockSource;
pub use view::{DeckState, DeckView, FitView, PlaylistTab, Row, StationView, ViewSource};
