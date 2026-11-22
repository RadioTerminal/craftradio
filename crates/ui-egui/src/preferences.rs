//! Preferences: every setting of the app in one window, laid out as SolidCraft's Preferences
//! (adapted with the SolidCraft maintainers' permission). A search field on top, a rail of
//! pages in groups on the left, the page's heading and form on the right, and Restore page
//! defaults and Done below. Each window's ⚙ opens it on that window's page. A setting takes
//! effect as it changes, through the view source's edits.

use crate::app::Action;
use crate::icons::Icon;
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
    Cartwall,
    Devices,
    Routing,
    Processing,
    Meters,
    Icecast,
    WebRtc,
    Clock,
    History,
    Library,
    Alarms,
    Shortcuts,
}

impl Page {
    /// The pages in rail order: each group's pages together.
    const ALL: [Self; 13] = [
        Self::Decks,
        Self::Cartwall,
        Self::Devices,
        Self::Routing,
        Self::Processing,
        Self::Meters,
        Self::Icecast,
        Self::WebRtc,
        Self::Clock,
        Self::History,
        Self::Library,
        Self::Alarms,
        Self::Shortcuts,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Decks => "Decks",
            Self::WebRtc => "WebRTC",
            Self::Cartwall => "Cartwall",
            Self::Clock => "Clock",
            Self::History => "History",
            Self::Library => "Library",
            Self::Alarms => "Alarms",
            Self::Shortcuts => "Shortcuts",
            other => other.mixer().map_or("", SettingsTab::label),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Decks => Icon::Disc3,
            Self::Devices => Icon::Speaker,
            Self::Routing => Icon::Route,
            Self::Processing => Icon::SlidersHorizontal,
            Self::Meters => Icon::Gauge,
            Self::Icecast => Icon::RadioTower,
            Self::WebRtc => Icon::Podcast,
            Self::Cartwall => Icon::LayoutGrid,
            Self::Clock => Icon::Clock,
            Self::History => Icon::History,
            Self::Library => Icon::Library,
            Self::Alarms => Icon::Bell,
            Self::Shortcuts => Icon::Keyboard,
        }
    }

    fn group(self) -> &'static str {
        match self {
            Self::Decks | Self::Cartwall => "Playout",
            Self::Devices | Self::Routing | Self::Processing | Self::Meters => "Audio",
            Self::Icecast | Self::WebRtc => "Streaming",
            Self::Clock | Self::History | Self::Library | Self::Alarms => "Windows",
            Self::Shortcuts => "Input",
        }
    }

    /// One line under the page heading.
    fn description(self) -> &'static str {
        match self {
            Self::Decks => "Fades, and how a cut of the item on air is confirmed.",
            Self::Cartwall => "The cart pages and the grid of the cartwall.",
            Self::Devices => "The audio devices, the live input channels and the sample rate.",
            Self::Routing => "The bus each strip plays to.",
            Self::Processing => "Loudness normalisation, the compressor and the limiter.",
            Self::Meters => "How long the meters hold a peak.",
            Self::Icecast => "The Icecast server that the station streams to.",
            Self::WebRtc => "The stream that listeners open in a browser, with no server between.",
            Self::Clock => "What the clock window shows.",
            Self::History => "The columns of the as-played log and how far back it goes.",
            Self::Library => "The folders that the library reads, and its categories.",
            Self::Alarms => "Which alarms the list shows.",
            Self::Shortcuts => "The keyboard shortcut of every command.",
        }
    }

    /// More words a search finds the page by: the labels of its rows and sections.
    fn keywords(self) -> &'static str {
        match self {
            Self::Decks => "on air confirm cut play fade-in fade-out deck a b c d",
            Self::Cartwall => "carts pages grid jingles",
            Self::Devices => {
                "output device out cue device pfl live input device channels sample rate"
            }
            Self::Routing => "route bus strips",
            Self::Processing => {
                "loudness normalisation target lufs max boost compressor threshold ratio \
                 attack compressor release makeup gain limiter ceiling limiter release dynamics"
            }
            Self::Meters => "peak hold",
            Self::WebRtc => "webrtc listeners browser link stream port bitrate kbps",
            Self::Icecast => {
                "status now streaming server host port tls encrypt mount user password format \
                 opus mp3 bitrate"
            }
            Self::Clock => {
                "face 24-hour time show the date seconds ring countdowns next fixed event \
                 end of the current item"
            }
            Self::History => {
                "columns aired at playlist title length origin range show the last hours"
            }
            Self::Library => "folders categories files",
            Self::Alarms => "list show acknowledged alarms",
            Self::Shortcuts => "keys keyboard shortcuts",
        }
    }

    /// Ok when Restore page defaults resets this page, or why it cannot.
    fn restore(self) -> Result<(), &'static str> {
        match self {
            Self::Decks
            | Self::Processing
            | Self::Meters
            | Self::Clock
            | Self::History
            | Self::Alarms
            | Self::Library
            | Self::WebRtc => Ok(()),
            Self::Devices | Self::Routing => {
                Err("The devices and routes of this computer have no defaults to go back to.")
            }
            Self::Icecast => Err("The Icecast server has no default; change it field by field."),
            Self::Cartwall => Err("The cart pages are the station's own and have no defaults."),
            Self::Shortcuts => {
                Err("Set shortcuts back to their defaults in the Keyboard Shortcuts dialog.")
            }
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

/// The page that Preferences shows, and the search.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) page: Page,
    pub(crate) search: String,
    /// The Search Preferences shortcut was pressed this frame.
    pub(crate) find: bool,
}

impl State {
    /// Show `page`, with no search left over from the last visit.
    pub(crate) fn open_at(&mut self, page: Page) {
        self.page = page;
        self.search.clear();
    }
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
    let tabs = Page::ALL.map(|p| Tab {
        label: p.label(),
        icon: p.icon(),
        group: p.group(),
        description: p.description(),
        keywords: p.keywords(),
        dot: (p == Page::Icecast).then(|| mixer::icecast_dot(view, t)),
        restore: p.restore(),
    });
    let mut nav = settings::Nav {
        selected: state.page.index(),
        search: std::mem::take(&mut state.search),
        find: std::mem::take(&mut state.find),
    };
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
        &mut nav,
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
                Page::WebRtc => webrtc_page(ui, view, actions, t),
                _ => {}
            }
        },
    );
    state.search = nav.search;
    if let Some(&p) = Page::ALL.get(nav.selected) {
        state.page = p;
    }
    if out.restore {
        restore_defaults(state.page, mixer_state, actions);
    }
    out.done
}

/// Sets the settings of `page` back to their defaults, as commands.
fn restore_defaults(page: Page, mixer_state: &mut mixer::State, actions: &mut Vec<Action>) {
    match page {
        Page::Decks => decks::restore_defaults(actions),
        Page::Processing | Page::Meters => {
            mixer::restore_defaults(mixer_state, page.mixer(), actions);
        }
        Page::Clock => clock_window::restore_defaults(actions),
        Page::History => history::restore_defaults(actions),
        Page::Alarms => alarms::restore_defaults(actions),
        Page::Library => library::restore_defaults(actions),
        Page::WebRtc => {
            for (key, value) in WEBRTC_DEFAULTS {
                actions.push(crate::parts::set(crate::parts::WEBRTC, key, value));
            }
        }
        // Pages without defaults show the button disabled with the reason.
        Page::Devices | Page::Routing | Page::Icecast | Page::Cartwall | Page::Shortcuts => {}
    }
}

/// The defaults of the WebRTC stream, as the playout has them: port, bitrate and listeners.
const WEBRTC_DEFAULTS: [(&str, u32); 3] = [("port", 8000), ("kbps", 96), ("max_listeners", 50)];

/// The WebRTC page: the switch of the status line, and the port, bitrate and listener cap that
/// the stream takes at its next start.
fn webrtc_page(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, t: &Tokens) {
    use crate::parts;
    use crate::theme::{self, size};
    use egui::{DragValue, RichText};
    settings::section(ui, "Stream", t, |ui| {
        match view.output.streaming {
            Some(on) => {
                let mut want = on;
                settings::form_row(ui, "Streaming", t, |ui| {
                    ui.checkbox(&mut want, "Stream to listeners")
                });
                if want != on {
                    actions.push(Action::Edit(crate::view::Edit::SetStreaming(want)));
                }
            }
            None => settings::form_note(
                ui,
                RichText::new(view.say(
                    "Preview: there is no WebRTC stream to turn on.",
                    "This station has no WebRTC output.",
                ))
                .font(theme::text(theme::CAPTION))
                .color(t.text_muted),
            ),
        }
        if let Some(link) = &view.output.listen_link {
            settings::form_row(ui, "Listen link", t, |ui| {
                ui.add(egui::Label::new(RichText::new(link).color(t.accent_ink)).truncate())
            });
        }
    });
    settings::section(ui, "Connection", t, |ui| {
        let [(_, port_d), (_, kbps_d), (_, max_d)] = WEBRTC_DEFAULTS;
        let old = (
            parts::u32(view, parts::WEBRTC, "port", port_d),
            parts::u32(view, parts::WEBRTC, "kbps", kbps_d),
            parts::u32(view, parts::WEBRTC, "max_listeners", max_d),
        );
        let (mut port, mut kbps, mut max) = old;
        settings::form_row(ui, "Port", t, |ui| {
            // A port is an address, not a quantity: 8000, never 8,000.
            let field = crate::numbers::field(ui, DragValue::new(&mut port).range(1..=u16::MAX))
                .ungrouped();
            ui.add_sized([ui.available_width(), size::CONTROL_SM], field)
        });
        settings::form_row(ui, "Bitrate", t, |ui| {
            let field = crate::numbers::field(
                ui,
                DragValue::new(&mut kbps).range(32..=320).suffix(" kbit/s"),
            );
            ui.add_sized([ui.available_width(), size::CONTROL_SM], field)
        });
        settings::form_row(ui, "Most listeners", t, |ui| {
            let field = crate::numbers::field(ui, DragValue::new(&mut max).range(1..=1000));
            ui.add_sized([ui.available_width(), size::CONTROL_SM], field)
        });
        settings::form_note(
            ui,
            RichText::new(view.say(
                "Preview: saved, and taken up by a station at its next start.",
                "Taken up at the next start of the app. The port is used for TCP and UDP.",
            ))
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
        );
        for (key, new, was) in [
            ("port", port, old.0),
            ("kbps", kbps, old.1),
            ("max_listeners", max, old.2),
        ] {
            if new != was {
                actions.push(parts::set(parts::WEBRTC, key, new));
            }
        }
    });
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
