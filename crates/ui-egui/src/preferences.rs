//! Preferences: every setting of the app in one window, as in the Craft apps. A rail of pages
//! on the left and the page's form on the right. Each window's ⚙ opens it on that window's
//! page. Station settings still change through the view source's edits, as before.

use crate::app::Action;
use crate::mixer::SettingsTab;
use crate::settings::{self, Tab};
use crate::theme::Tokens;
use crate::view::StationView;
use crate::{alarms, cartwall, clock_window, decks, history, library, mixer};

/// One page of Preferences, in the order of the rail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Decks,
    Devices,
    Routing,
    Processing,
    Meters,
    Icecast,
    Cartwall,
    Clock,
    History,
    Library,
    Alarms,
    Shortcuts,
}

impl Page {
    const ALL: [Self; 12] = [
        Self::Decks,
        Self::Devices,
        Self::Routing,
        Self::Processing,
        Self::Meters,
        Self::Icecast,
        Self::Cartwall,
        Self::Clock,
        Self::History,
        Self::Library,
        Self::Alarms,
        Self::Shortcuts,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Decks => "Decks",
            Self::Cartwall => "Cartwall",
            Self::Clock => "Clock",
            Self::History => "History",
            Self::Library => "Library",
            Self::Alarms => "Alarms",
            Self::Shortcuts => "Shortcuts",
            other => other.mixer().map_or("", SettingsTab::label),
        }
    }

    /// The mixer's settings tab that this page shows, if it is one.
    fn mixer(self) -> Option<SettingsTab> {
        match self {
            Self::Devices => Some(SettingsTab::Devices),
            Self::Routing => Some(SettingsTab::Routing),
            Self::Processing => Some(SettingsTab::Processing),
            Self::Meters => Some(SettingsTab::Meters),
            Self::Icecast => Some(SettingsTab::Icecast),
            _ => None,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}

impl From<SettingsTab> for Page {
    fn from(tab: SettingsTab) -> Self {
        match tab {
            SettingsTab::Devices => Self::Devices,
            SettingsTab::Routing => Self::Routing,
            SettingsTab::Processing => Self::Processing,
            SettingsTab::Meters => Self::Meters,
            SettingsTab::Icecast => Self::Icecast,
        }
    }
}

/// The page that Preferences shows.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) page: Page,
}

/// The windows whose drafts the pages edit.
pub(crate) struct Parts<'a> {
    pub mixer: &'a mut mixer::State,
    pub carts: &'a mut cartwall::State,
    pub library: &'a mut library::State,
}

/// Draws Preferences. It returns true when a person is done with it (Done or Escape).
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    parts: Parts<'_>,
    streaming: &str,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> bool {
    let tabs = Page::ALL.map(|p| match p {
        Page::Icecast => Tab {
            label: p.label(),
            dot: Some(mixer::icecast_dot(view, t)),
        },
        _ => Tab::plain(p.label()),
    });
    let mut selected = state.page.index();
    let Parts {
        mixer: mixer_state,
        carts,
        library: library_state,
    } = parts;
    let out = settings::page(
        ui,
        "Preferences",
        view.preview,
        &tabs,
        &mut selected,
        t,
        |ui, i| {
            let page = Page::ALL.get(i).copied().unwrap_or_default();
            if let Some(tab) = page.mixer() {
                mixer::preferences(ui, view, mixer_state, tab, streaming, actions, t);
                return;
            }
            match page {
                Page::Decks => decks::preferences(ui, view, actions, t),
                Page::Cartwall => cartwall::preferences(ui, view, carts, actions, t),
                Page::Clock => clock_window::preferences(ui, view, actions, t),
                Page::History => history::preferences(ui, view, actions, t),
                Page::Library => library::preferences(ui, view, library_state, actions, t),
                Page::Alarms => alarms::preferences(ui, view, actions, t),
                Page::Shortcuts => shortcuts_page(ui, actions, t),
                _ => {}
            }
        },
    );
    if let Some(&p) = Page::ALL.get(selected) {
        state.page = p;
    }
    out.done
}

/// The Shortcuts page: the keyboard shortcuts are edited in their own dialog, which needs
/// the room of a table.
fn shortcuts_page(ui: &mut egui::Ui, actions: &mut Vec<Action>, t: &Tokens) {
    settings::form_note(
        ui,
        egui::RichText::new(
            "Every command's keyboard shortcut can be changed, cleared or set back to its \
             default. The menus show the new keys at once.",
        )
        .color(t.text_muted),
    );
    if ui.button("Keyboard Shortcuts…").clicked() {
        actions.push(Action::OpenShortcuts);
    }
}
