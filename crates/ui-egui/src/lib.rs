//! The egui desktop UI: the view-model, a mock source for the preview, and the panels.

mod app;
mod cartwall;
mod clock_window;
mod context_menu;
mod decks;
mod history;
mod library;
mod main_pane;
mod meta_editor;
mod mixer;
pub mod mock;
mod parts;
mod rules_editor;
pub mod rules_script;
mod scheduled_pane;
mod settings;
pub mod theme;
pub mod view;
mod widgets;
pub mod windows;

pub use app::RadioApp;
pub use mock::MockSource;
pub use view::{DeckState, DeckView, FitView, PlaylistTab, Row, StationView, ViewSource};
pub use windows::ToolWindow;
