//! The egui desktop UI: the view-model, a mock source for the preview, and the panels.
//!
//! # Glossary
//!
//! The UI uses one word per idea:
//!
//! - **Main**: the main playlist, in the left pane. Its tracks run in order.
//! - **Upcoming**: the right list. It shows the items that rules and fixed times weave into
//!   Main, in time order.
//! - **Playlist**: a named list (Jingles, Ads…) with its rules, as a tab beside Upcoming.
//! - **Rule**: when a playlist puts an item into Main ("every 4 tracks", "at :00").
//! - **Fixed**: an item that starts at a set time. Hard ones cut in; soft ones wait for the
//!   end of the track.
//! - **Deck**: one of the four players A–D. **On air**: the item that listeners hear now.
//!   **NEXT 1/2/3**: the order of the ready decks.
//! - **Cart**: a short sound on the Cartwall, fired by hand or by F1–F12.
//! - **Remove** takes an item out of a list. **Delete** destroys a whole object (a rule,
//!   a playlist). Both can be undone.
//! - **Library**: the files the station knows, in categories.

mod add_menu;
mod alarms;
mod app;
mod cartwall;
mod clock_window;
mod context_menu;
mod decks;
mod fonts;
mod history;
mod icons;
mod library;
mod main_pane;
mod meta_editor;
mod mixer;
pub mod mock;
mod parts;
mod prepared;
mod rules_editor;
pub mod rules_script;
mod scheduled_pane;
mod select;
mod settings;
mod shortcuts;
pub mod theme;
pub mod view;
mod widgets;
pub mod windows;

pub use app::RadioApp;
pub use mock::MockSource;
pub use view::{
    AlarmSource, AlarmView, DeckState, DeckView, FitView, PlaylistTab, Row, Severity, StationView,
    ViewSource,
};
pub use windows::ToolWindow;

/// Where the card of deck `i` was drawn in the last frame. For tests.
#[doc(hidden)]
#[must_use]
pub fn deck_card_rect(ctx: &egui::Context, i: usize) -> Option<egui::Rect> {
    ctx.data(|d| d.get_temp::<egui::Rect>(decks::card_rect_id(i)))
}
