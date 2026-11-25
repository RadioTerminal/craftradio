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
use crate::{alarms, cartwall, clock_window, decks, history, library, mixer, server_page};

/// One page of Preferences, in the order of the rail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Decks,
    Cartwall,
    Devices,
    Processing,
    Meters,
    Icecast,
    WebRtc,
    Server,
    Branding,
    Clock,
    History,
    Library,
    Alarms,
    Shortcuts,
}

impl Page {
    /// The pages in rail order: each group's pages together.
    const ALL: [Self; 14] = [
        Self::Decks,
        Self::Cartwall,
        Self::Devices,
        Self::Processing,
        Self::Meters,
        Self::Icecast,
        Self::WebRtc,
        Self::Server,
        Self::Branding,
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
            Self::Server => "Server",
            Self::Branding => "Branding",
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
            Self::Processing => Icon::SlidersHorizontal,
            Self::Meters => Icon::Gauge,
            Self::Icecast => Icon::RadioTower,
            Self::WebRtc => Icon::Podcast,
            Self::Server => Icon::Link,
            Self::Branding => Icon::Image,
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
            Self::Devices | Self::Processing | Self::Meters => "Audio",
            Self::Icecast | Self::WebRtc | Self::Server | Self::Branding => "Streaming",
            Self::Clock | Self::History | Self::Library | Self::Alarms => "Windows",
            Self::Shortcuts => "Input",
        }
    }

    /// One line under the page heading.
    fn description(self) -> &'static str {
        match self {
            Self::Decks => "Fades, and how a cut of the item on air is confirmed.",
            Self::Cartwall => "The cart pages and the grid of the cartwall.",
            Self::Devices => {
                "The Program and monitor devices, the live input channels and the sample rate."
            }
            Self::Processing => "Loudness normalisation, the compressor and the limiter.",
            Self::Meters => "How long the meters hold a peak.",
            Self::Icecast => "The Icecast server that the station streams to.",
            Self::WebRtc => "The stream that listeners open in a browser, with no server between.",
            Self::Server => {
                "The station server that this station pairs with by its address and key."
            }
            Self::Branding => "The name and logo that listeners see when no song shows.",
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
                "program device output out none monitor device snd cue pfl route routing \
                 live input device channels sample rate"
            }
            Self::Processing => {
                "loudness normalisation target lufs max boost compressor threshold ratio \
                 attack compressor release makeup gain limiter ceiling limiter release dynamics"
            }
            Self::Meters => "peak hold",
            Self::WebRtc => "webrtc listeners browser link stream port bitrate kbps",
            Self::Server => "station server address key pair pairing connect disconnect unpair",
            Self::Branding => "station name logo picture brand listeners player",
            Self::Icecast => {
                "status now streaming server host port tls encrypt mount user password format \
                 opus mp3 bitrate public address listeners fallback"
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

    /// Ok when Restore page defaults resets this page, or why it cannot. The Icecast and
    /// Server pages depend on whether `view` is paired with a server.
    fn restore(self, view: &StationView) -> Result<(), &'static str> {
        let paired = view.pairing.paired;
        match self {
            Self::Decks
            | Self::Processing
            | Self::Meters
            | Self::Clock
            | Self::History
            | Self::Alarms
            | Self::Library
            | Self::WebRtc
            | Self::Branding => Ok(()),
            Self::Devices => Err("The devices of this computer have no defaults to go back to."),
            Self::Icecast if paired => Err(crate::view::ICECAST_PAIRED),
            Self::Icecast => Ok(()),
            Self::Cartwall => Err("The cart pages are the station's own and have no defaults."),
            // As Disconnect: the pairing goes, and so do the settings it wrote and the key.
            Self::Server if paired => Ok(()),
            Self::Server => Err("The station is not paired with a server."),
            Self::Shortcuts => {
                Err("Set shortcuts back to their defaults in the Keyboard Shortcuts dialog.")
            }
        }
    }

    /// The mixer's settings tab that this page shows, if it is one.
    fn mixer(self) -> Option<SettingsTab> {
        match self {
            Self::Devices => Some(SettingsTab::Devices),
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
    branding: BrandingDraft,
    server: server_page::Draft,
}

impl State {
    /// Show `page`, with no search left over from the last visit.
    pub(crate) fn open_at(&mut self, page: Page) {
        self.page = page;
        self.search.clear();
    }

    /// Saves a draft of the Branding page that is still being typed. It runs when
    /// Preferences closes or the person moves to another page, where the field is no longer
    /// drawn and so never sees itself lose the focus.
    pub(crate) fn flush_drafts(&mut self, view: &StationView, actions: &mut Vec<Action>) {
        use crate::parts;
        // A typed station key is never kept past the page.
        self.server.forget();
        let draft = &mut self.branding;
        let Some(key) = draft.editing.take() else {
            return;
        };
        let typed = if key == radio_model::BRANDING_NAME_KEY {
            &draft.name
        } else {
            &draft.logo
        };
        if typed.as_str() != parts::text(view, parts::BRANDING, key) {
            actions.push(parts::set(parts::BRANDING, key, typed.clone()));
        }
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
        dot: match p {
            Page::Icecast => Some(mixer::icecast_dot(view, t)),
            Page::WebRtc => Some(view.output.webrtc_page_status()),
            Page::Server => view.pairing.page_status(),
            _ => None,
        },
        restore: p.restore(view),
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
        // `searching` is true while there is search text: a page that the search selects
        // does not take the focus from the search field.
        |ui, i, searching| {
            let branding = &mut state.branding;
            let server = &mut state.server;
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
                Page::Server => server_page::show(ui, view, server, !searching, actions, t),
                Page::Branding => branding_page(ui, view, branding, !searching, actions, t),
                _ => {}
            }
        },
    );
    state.search = nav.search;
    if let Some(&p) = Page::ALL.get(nav.selected) {
        if p != state.page {
            state.flush_drafts(view, actions);
        }
        state.page = p;
    }
    if out.restore && state.page.restore(view).is_ok() {
        restore_defaults(state.page, view, mixer_state, actions);
    }
    out.done
}

/// Sets the settings of `page` back to their defaults, as commands.
fn restore_defaults(
    page: Page,
    view: &StationView,
    mixer_state: &mut mixer::State,
    actions: &mut Vec<Action>,
) {
    match page {
        Page::Decks => decks::restore_defaults(actions),
        Page::Processing | Page::Meters | Page::Icecast => {
            mixer::restore_defaults(view, mixer_state, page.mixer(), actions);
        }
        Page::Clock => clock_window::restore_defaults(actions),
        Page::History => history::restore_defaults(actions),
        Page::Alarms => alarms::restore_defaults(actions),
        Page::Library => library::restore_defaults(actions),
        Page::WebRtc => {
            // Streaming to listeners is off until a person turns it on: what the page's own
            // switch sends, so the source turns it off as it does there.
            actions.push(Action::EditQuiet(crate::view::Edit::SetStreaming(
                STREAMING_DEFAULT,
            )));
            for (key, value) in WEBRTC_DEFAULTS {
                actions.push(crate::parts::set(crate::parts::WEBRTC, key, value));
            }
            actions.push(crate::parts::set(
                crate::parts::WEBRTC,
                "relay",
                RELAY_DEFAULT,
            ));
            actions.push(crate::parts::set(
                crate::parts::WEBRTC,
                "direct_reserve",
                RESERVE_DEFAULT,
            ));
        }
        Page::Branding => {
            for key in [
                radio_model::BRANDING_NAME_KEY,
                radio_model::BRANDING_LOGO_KEY,
            ] {
                actions.push(crate::parts::set(crate::parts::BRANDING, key, ""));
            }
        }
        // What Disconnect does: unpair, which removes the key too.
        Page::Server => actions.push(Action::Edit(crate::view::Edit::Unpair)),
        // Pages without defaults show the button disabled with the reason.
        Page::Devices | Page::Cartwall | Page::Shortcuts => {}
    }
}

/// The defaults of the WebRTC stream, as the playout has them: port, bitrate and listeners.
const WEBRTC_DEFAULTS: [(&str, u32); 3] = [("port", 8000), ("kbps", 96), ("max_listeners", 50)];
/// Streaming to listeners over WebRTC, the `enabled` key, is off until a person turns it on,
/// as the playout reads it.
const STREAMING_DEFAULT: bool = false;
/// Listeners pass the audio on unless the station turns it off.
const RELAY_DEFAULT: bool = true;
/// Of the most listeners, the slots kept for listeners who find no one to play through.
const RESERVE_DEFAULT: u32 = 10;

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
        // The same words as the dot on the rail and the status bar.
        settings::status_line(ui, &view.output.webrtc_page_status(), t);
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
    settings::section(ui, "Passing it on", t, |ui| {
        let on = parts::bool(view, parts::WEBRTC, "relay", RELAY_DEFAULT);
        let mut want = on;
        settings::form_row(ui, "Relay", t, |ui| {
            ui.checkbox(&mut want, "Let listeners pass the stream on")
        });
        if want != on {
            actions.push(parts::set(parts::WEBRTC, "relay", want));
        }
        let kept = parts::u32(view, parts::WEBRTC, "direct_reserve", RESERVE_DEFAULT);
        let mut reserve = kept;
        settings::form_row(ui, "Kept back", t, |ui| {
            ui.add_enabled_ui(on, |ui| {
                let field = crate::numbers::field(
                    ui,
                    DragValue::new(&mut reserve)
                        .range(0..=1000)
                        .suffix(" listeners"),
                );
                ui.add_sized([ui.available_width(), size::CONTROL_SM], field)
            })
            .inner
        });
        if reserve != kept {
            actions.push(parts::set(parts::WEBRTC, "direct_reserve", reserve));
        }
        settings::form_note(
            ui,
            RichText::new(
                "Each listener who opens the station link may pass the audio on to a few \
                 others, so more people can listen than the station's upload allows. The \
                 station signs the audio, so no one can change it on the way. Listeners who \
                 pass it on see each other's IP addresses, and each can turn it off on the \
                 page. Of the most listeners, the station keeps some back for those who find \
                 no one to play through, or who turned it off; at least one place stays open \
                 for everyone. Taken up at the next start of the app.",
            )
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
        );
    });
}

/// What the Branding page is typing. A field keeps its text while it has the focus, and the
/// setting is saved when the field is left, so the station does not read each keystroke.
#[derive(Default)]
struct BrandingDraft {
    name: String,
    logo: String,
    /// The field being typed in, by key.
    editing: Option<&'static str>,
    /// The frame in which the page last drew, to tell when it opens.
    drawn: u64,
    /// The logo that egui holds, so a logo that left is forgotten.
    picture: Option<String>,
}

/// A text field of the Branding page. `saved` is the setting; the draft follows it until the
/// field has the focus. It returns the text when the person left the field with a change.
fn branding_field(
    ui: &mut egui::Ui,
    key: &'static str,
    draft: &mut String,
    editing: &mut Option<&'static str>,
    saved: &str,
    hint: &str,
    width: f32,
) -> (egui::Response, Option<String>) {
    use crate::theme::size;
    if *editing != Some(key) {
        draft.clear();
        draft.push_str(saved);
    }
    let field = egui::TextEdit::singleline(draft).hint_text(hint);
    let r = ui.add_sized([width, size::CONTROL_SM], field);
    if r.has_focus() {
        *editing = Some(key);
    }
    let mut out = None;
    if r.lost_focus() {
        *editing = None;
        if draft.as_str() != saved {
            out = Some(draft.clone());
        }
    }
    (r, out)
}

/// The Branding page: the station name and the logo that listeners see.
fn branding_page(
    ui: &mut egui::Ui,
    view: &StationView,
    draft: &mut BrandingDraft,
    take_focus: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    use crate::theme::{self, size, space};
    use crate::{icons, parts, view::check_picture};
    use egui::RichText;
    let frame = ui.ctx().cumulative_frame_nr();
    let opened = frame > draft.drawn + 1;
    draft.drawn = frame;
    if opened {
        draft.editing = None;
    }
    let name = parts::text(view, parts::BRANDING, radio_model::BRANDING_NAME_KEY);
    let logo = parts::text(view, parts::BRANDING, radio_model::BRANDING_LOGO_KEY);
    let caption = |text: &str| {
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    settings::section(ui, "Station", t, |ui| {
        let BrandingDraft {
            name: name_draft,
            logo: logo_draft,
            editing,
            picture,
            ..
        } = draft;
        settings::form_row(ui, "Station name", t, |ui| {
            let (r, saved) = branding_field(
                ui,
                radio_model::BRANDING_NAME_KEY,
                name_draft,
                editing,
                &name,
                radio_model::DEFAULT_STATION_NAME,
                ui.available_width(),
            );
            if opened && take_focus {
                r.request_focus();
            }
            if let Some(text) = saved {
                actions.push(parts::set(
                    parts::BRANDING,
                    radio_model::BRANDING_NAME_KEY,
                    text,
                ));
            }
            r
        });
        settings::form_note(
            ui,
            caption(view.say(
                "Preview: saved, and taken up by a station.",
                "Listeners see the name at the next item. Icecast reconnects with the new name.",
            )),
        );
        settings::form_row(ui, "Logo", t, |ui| {
            let buttons = 2.0 * (size::DIALOG_BUTTON_W + ui.spacing().item_spacing.x);
            let (r, saved) = branding_field(
                ui,
                radio_model::BRANDING_LOGO_KEY,
                logo_draft,
                editing,
                &logo,
                "path to a PNG, JPEG or WebP picture",
                (ui.available_width() - buttons).max(size::CONTROL_SM),
            );
            if let Some(text) = saved {
                actions.push(parts::set(
                    parts::BRANDING,
                    radio_model::BRANDING_LOGO_KEY,
                    text,
                ));
            }
            let picked = match crate::widgets::picture_buttons(ui, !logo.is_empty()) {
                Some(crate::widgets::PicturePick::File(file)) => Some(file),
                Some(crate::widgets::PicturePick::Clear) => Some(String::new()),
                None => None,
            };
            if let Some(path) = picked {
                actions.push(parts::set(
                    parts::BRANDING,
                    radio_model::BRANDING_LOGO_KEY,
                    path,
                ));
            }
            r
        });
        let problem = |ui: &mut egui::Ui, why: &str| {
            settings::form_note(
                ui,
                RichText::new(icons::lead(icons::Icon::TriangleAlert, why))
                    .font(theme::text(theme::CAPTION))
                    .color(t.danger),
            );
        };
        // The picture, or why there is none. A logo that failed never looks set.
        let mut shown = None;
        match (check_picture(&logo), &view.branding_logo) {
            _ if logo.trim().is_empty() => {
                settings::form_note(ui, caption("No logo: listeners see the app icon."))
            }
            (Err(why), _) => problem(ui, why),
            (_, Some(Err(why))) => problem(ui, why),
            (Ok(()), Some(Ok(cover))) => {
                shown = Some(cover.uri.clone());
                ui.add(
                    egui::Image::from_bytes(cover.uri.clone(), cover.bytes.clone())
                        .fit_to_exact_size(egui::Vec2::splat(size::LOGO_PREVIEW))
                        .corner_radius(t.radius)
                        .alt_text("Logo preview"),
                );
            }
            (Ok(()), None) => settings::form_note(
                ui,
                caption(view.say("Preview: the picture is not read.", "Reading the picture…")),
            ),
        }
        ui.add_space(space::XS);
        if *picture != shown {
            if let Some(gone) = picture.take() {
                ui.ctx().forget_image(&gone);
            }
            *picture = shown;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Edit, ViewSource};

    fn view() -> StationView {
        let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
        let now: jiff::Timestamp = "2026-11-21T13:58:00Z".parse().expect("ts");
        crate::MockSource::new(tz).snapshot(now)
    }

    fn tab(page: Page) -> Tab {
        Tab {
            label: page.label(),
            icon: page.icon(),
            group: page.group(),
            description: page.description(),
            keywords: page.keywords(),
            dot: None,
            restore: page.restore(&view()),
        }
    }

    #[test]
    fn a_search_for_logo_finds_branding() {
        let found: Vec<Page> = Page::ALL
            .into_iter()
            .filter(|p| tab(*p).matches("logo"))
            .collect();
        assert_eq!(found, vec![Page::Branding]);
    }

    #[test]
    fn restoring_branding_clears_the_name_and_the_logo() {
        assert!(Page::Branding.restore(&view()).is_ok());
        let mut actions = Vec::new();
        restore_defaults(
            Page::Branding,
            &view(),
            &mut mixer::State::default(),
            &mut actions,
        );
        let mut keys: Vec<String> = actions
            .iter()
            .filter_map(|a| match a {
                Action::EditQuiet(Edit::SetPartSetting { part, key, value })
                    if part == radio_model::BRANDING_PART && value == "" =>
                {
                    Some(key.clone())
                }
                _ => None,
            })
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                radio_model::BRANDING_LOGO_KEY,
                radio_model::BRANDING_NAME_KEY
            ]
        );
    }
}
