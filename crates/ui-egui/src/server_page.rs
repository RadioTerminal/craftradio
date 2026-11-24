//! The Server page of Preferences: the station server's address, the station key, Connect and
//! Disconnect, and how the pairing stands, in words. The key goes only to the `Pair` edit,
//! which hands it to the keychain; the page forgets it once Connect sends it.

use crate::app::Action;
use crate::icons::{self, Icon};
use crate::settings;
use crate::theme::{self, Tokens, size};
use crate::view::{Edit, PairState, PairingView, Secret, StationView};
use egui::{Color32, RichText};

/// What the page is typing. The key lives here only while it is typed: Connect takes it, and
/// leaving the page or closing Preferences clears it.
#[derive(Default)]
pub(crate) struct Draft {
    address: String,
    /// The address was typed into; until then it follows the paired one.
    edited: bool,
    key: String,
    /// The frame in which the page last drew, to tell when it opens.
    drawn: u64,
}

impl Draft {
    /// Forgets the typed key, and lets the address follow the station again.
    pub(crate) fn forget(&mut self) {
        self.key.clear();
        self.edited = false;
    }
}

/// The icon and the colour of a pairing state. Only a pairing the server took, in the real
/// app, looks like success; every other state looks like what it is (rule 7). The icon goes
/// with the colour, so the colour is never the only signal.
fn look(pairing: &PairingView, preview: bool, t: &Tokens) -> (Icon, Color32) {
    match &pairing.state {
        PairState::Paired if !preview => (Icon::CircleDot, t.ok),
        PairState::Paired | PairState::NotPaired => (Icon::Circle, t.text_muted),
        PairState::Pairing => (Icon::RefreshCw, t.text_muted),
        PairState::Unreachable(_) => (Icon::TriangleAlert, t.warn),
        PairState::KeyRefused | PairState::Refused(_) => (Icon::OctagonAlert, t.danger),
    }
}

/// The colour that the page gives the state of `view`, for the tests of rule 7.
#[doc(hidden)]
#[must_use]
pub fn state_ink(view: &StationView, t: &Tokens) -> Color32 {
    look(&view.pairing, view.preview, t).1
}

/// Draws the page. `take_focus` is false while a search is typed, so the page does not take
/// the focus from the search field.
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    draft: &mut Draft,
    take_focus: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let frame = ui.ctx().cumulative_frame_nr();
    let opened = frame > draft.drawn + 1;
    draft.drawn = frame;
    if opened {
        draft.forget();
    }
    let pairing = &view.pairing;
    if !draft.edited {
        draft.address.clone_from(&pairing.address);
    }
    let caption = |text: &str| {
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    settings::section(ui, "Pairing", t, |ui| {
        let (icon, ink) = look(pairing, view.preview, t);
        settings::form_note(
            ui,
            RichText::new(icons::lead(icon, &pairing.label()))
                .font(theme::text(theme::LABEL))
                .color(ink),
        );
        settings::form_row(ui, "Server address", t, |ui| {
            let edit = egui::TextEdit::singleline(&mut draft.address)
                .hint_text("https://radio.example.org")
                .desired_width(ui.available_width())
                .min_size(egui::vec2(0.0, size::CONTROL_SM))
                .align(egui::Align2::LEFT_CENTER);
            let r = ui.add(edit);
            if r.changed() {
                draft.edited = true;
            }
            if opened && take_focus {
                r.request_focus();
            }
            r
        });
        // An empty key uses the one in the keychain, which only a pairing put there.
        let hint = if pairing.paired {
            "in the keychain · leave empty to keep it"
        } else {
            "the key the server gave this station"
        };
        let mut enter = false;
        let mut key_id = None;
        settings::form_row(ui, "Station key", t, |ui| {
            let edit = egui::TextEdit::singleline(&mut draft.key)
                .password(true)
                .hint_text(hint)
                .desired_width(ui.available_width())
                .min_size(egui::vec2(0.0, size::CONTROL_SM))
                .align(egui::Align2::LEFT_CENTER);
            let r = ui.add(edit);
            enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            key_id = Some(r.id);
            r
        });
        let address = draft.address.trim();
        let can_connect = !address.is_empty() && (!draft.key.is_empty() || pairing.paired);
        let mut connect = false;
        ui.horizontal(|ui| {
            let why = if address.is_empty() {
                "Type the server address first."
            } else {
                "Type the station key first."
            };
            connect = ui
                .add_enabled(can_connect, egui::Button::new("Connect"))
                .on_disabled_hover_text(why)
                .clicked();
            if ui
                .add_enabled(pairing.paired, egui::Button::new("Disconnect"))
                .on_disabled_hover_text("This station is not paired with a server.")
                .clicked()
            {
                actions.push(Action::Edit(Edit::Unpair));
            }
        });
        if (connect || enter) && can_connect {
            let key = Secret::new(std::mem::take(&mut draft.key));
            actions.push(Action::Edit(Edit::Pair {
                address: address.to_owned(),
                key,
            }));
            // The field's undo history holds what was typed; it goes with the key.
            if let Some(id) = key_id {
                egui::text_edit::TextEditState::default().store(ui.ctx(), id);
            }
        }
        settings::form_note(
            ui,
            caption(view.say(
                "Preview: Connect needs the station app; nothing is sent from here.",
                "The key goes to the keychain only. Connect asks the server for the station's \
                 settings and streams to it.",
            )),
        );
        settings::form_note(
            ui,
            caption(
                "Pairing writes the Icecast server, mount, user, TLS and public address, and \
                 the WebRTC public stream. The app asks the server again at each start, and \
                 that overwrites what was changed by hand in those fields. Disconnect sets \
                 them back to their defaults and removes the key from the keychain.",
            ),
        );
    });
}
