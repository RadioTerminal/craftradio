//! The Server page of Preferences: the station server's address, the station key, Connect and
//! Disconnect, and how the pairing stands, in words. The key goes only to the `Pair` edit,
//! which hands it to the keychain; the page forgets it once Connect sends it.

use crate::app::Action;
use crate::icons::{self, Icon};
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{
    Edit, OUTPUTS_READ_ONLY, OutputRow, OutputsState, PairState, PairingView, Secret, StationView,
    StreamFormat,
};
use crate::widgets;
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
    /// The outputs as the page edits them.
    outputs: Vec<OutputRow>,
    /// The outputs of the view that `outputs` was last made from. A new view replaces the
    /// edited rows only while they are still these (nothing typed would be lost). `None`
    /// until the page has drawn them once.
    outputs_seen: Option<Vec<OutputRow>>,
}

impl Draft {
    /// Forgets the typed key, and lets the address follow the station again.
    pub(crate) fn forget(&mut self) {
        self.key.clear();
        self.edited = false;
        self.outputs_seen = None;
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
    sync_outputs(draft, &pairing.outputs);
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
        // One try at a time: a second click would be one more key toward the lockout.
        let asking = pairing.state == PairState::Pairing;
        let can_connect =
            !asking && !address.is_empty() && (!draft.key.is_empty() || pairing.paired);
        let mut connect = false;
        ui.horizontal(|ui| {
            let why = if asking {
                "The server is being asked; wait for its answer."
            } else if address.is_empty() {
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
    // The outputs live on the server, so they are offered only to a pairing it took.
    if pairing.paired && pairing.state == PairState::Paired {
        settings::section(ui, "Outputs", t, |ui| {
            outputs(ui, pairing, draft, actions, t);
        });
    }
}

/// Makes the edited rows follow the view: at once while nothing was edited, and never over
/// an edit.
fn sync_outputs(draft: &mut Draft, from_view: &[OutputRow]) {
    let unedited = draft.outputs_seen.as_deref() == Some(draft.outputs.as_slice());
    let changed = draft.outputs_seen.as_deref() != Some(from_view);
    if draft.outputs_seen.is_none() || (changed && unedited) {
        draft.outputs = from_view.to_vec();
    }
    if changed {
        draft.outputs_seen = Some(from_view.to_vec());
    }
}

const OUTPUT_KBPS_DEFAULT: u32 = 64;
/// The bitrates the server takes: Opus any whole kbit/s in this range, MP3 only these steps.
const OPUS_KBPS_RANGE: std::ops::RangeInclusive<u32> = 6..=510;
const MP3_KBPS: [u32; 14] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];

/// What a new row starts as.
fn new_output() -> OutputRow {
    OutputRow {
        mount: String::new(),
        format: StreamFormat::Opus,
        kbps: OUTPUT_KBPS_DEFAULT,
    }
}

/// The icon, the colour and the words of the last save, or nothing when there was none. The
/// icon goes with the colour, so the colour is never the only signal.
fn outputs_line(state: &OutputsState, t: &Tokens) -> Option<(Icon, Color32, String)> {
    let (icon, ink) = match state {
        OutputsState::Idle => return None,
        OutputsState::Saving => (Icon::RefreshCw, t.text_muted),
        OutputsState::Saved => (Icon::CircleDot, t.ok),
        OutputsState::Failed(_) => (Icon::OctagonAlert, t.danger),
    };
    Some((icon, ink, state.label().to_owned()))
}

/// The Outputs section: one row per re-encoded output, Add output and Save, and how the last
/// save went. Read-only, with the reason, when the server keeps its outputs in its config.
fn outputs(
    ui: &mut egui::Ui,
    pairing: &PairingView,
    draft: &mut Draft,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let caption = |text: &str| {
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    if !pairing.outputs_editable {
        for r in &pairing.outputs {
            let words = format!("{} · {} kbit/s · {}", r.format.label(), r.kbps, r.mount);
            settings::form_note(
                ui,
                RichText::new(words)
                    .font(theme::text(theme::LABEL))
                    .color(t.text),
            );
        }
        settings::form_note(ui, caption(OUTPUTS_READ_ONLY));
        return;
    }
    settings::form_note(
        ui,
        caption(
            "Each output is the stream re-encoded and served under the station's address. \
             Save sends the whole list to the server.",
        ),
    );
    if draft.outputs.is_empty() {
        settings::form_note(ui, caption("No outputs yet."));
    }
    let mut remove = None;
    for (i, r) in draft.outputs.iter_mut().enumerate() {
        if output_row(ui, i, r) {
            remove = Some(i);
        }
    }
    if let Some(i) = remove {
        draft.outputs.remove(i);
    }
    let saving = pairing.outputs_state == OutputsState::Saving;
    let failed = matches!(pairing.outputs_state, OutputsState::Failed(_));
    // After a failed save the view holds the rows that were sent, so they match the draft;
    // trying again is still offered.
    let can_save = !saving && (failed || draft.outputs != pairing.outputs);
    ui.horizontal(|ui| {
        if ui.button("Add output").clicked() {
            draft.outputs.push(new_output());
        }
        let why = if saving {
            "The outputs are being sent; wait for the server's answer."
        } else {
            "Nothing changed since the server's list."
        };
        if ui
            .add_enabled(can_save, egui::Button::new("Save"))
            .on_disabled_hover_text(why)
            .clicked()
        {
            actions.push(Action::Edit(Edit::SaveServerOutputs(draft.outputs.clone())));
        }
    });
    if let Some((icon, ink, words)) = outputs_line(&pairing.outputs_state, t) {
        settings::form_note(
            ui,
            RichText::new(icons::lead(icon, &words))
                .font(theme::text(theme::LABEL))
                .color(ink),
        );
    }
}

/// One editable row: format, bitrate, mount and Remove. Returns true when Remove was used.
/// The columns have no heading, so each control is named by the word of its job and the
/// row's number.
fn output_row(ui: &mut egui::Ui, i: usize, row: &mut OutputRow) -> bool {
    let n = i + 1;
    let mut remove = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let combo = craft_ui::ComboBox::from_id_salt(("output_format", i))
            .width(size::OUTPUT_FORMAT_W)
            .selected_text(row.format.label())
            .show_ui(ui, |ui| {
                for f in StreamFormat::ALL {
                    widgets::selectable_value(ui, &mut row.format, f, f.label());
                }
            })
            .response;
        combo.accessible_name(format!("Format of output {n}"));
        let kbps = match row.format {
            StreamFormat::Opus => ui.add_sized(
                egui::vec2(size::OUTPUT_KBPS_W, size::CONTROL_SM),
                egui::DragValue::new(&mut row.kbps)
                    .range(OPUS_KBPS_RANGE)
                    // A value the server refused stays as it was sent; only an edit clamps it.
                    .clamp_existing_to_range(false)
                    .suffix(" kbit/s"),
            ),
            // A value that is not a step stays shown until one is picked.
            StreamFormat::Mp3 => {
                craft_ui::ComboBox::from_id_salt(("output_kbps", i))
                    .width(size::OUTPUT_KBPS_W)
                    .selected_text(format!("{} kbit/s", row.kbps))
                    .show_ui(ui, |ui| {
                        for b in MP3_KBPS {
                            widgets::selectable_value(ui, &mut row.kbps, b, format!("{b} kbit/s"));
                        }
                    })
                    .response
            }
        };
        kbps.accessible_name(format!("Bitrate of output {n}"));
        let mount_w = (ui.available_width() - size::OUTPUT_REMOVE_W - space::S).max(0.0);
        let mount = ui.add(
            egui::TextEdit::singleline(&mut row.mount)
                .hint_text("low.opus")
                .desired_width(mount_w)
                .min_size(egui::vec2(0.0, size::CONTROL_SM))
                .align(egui::Align2::LEFT_CENTER),
        );
        mount.accessible_name(format!("Mount of output {n}"));
        let button = ui.add_sized(
            egui::vec2(size::OUTPUT_REMOVE_W, size::CONTROL_SM),
            egui::Button::new("Remove"),
        );
        remove = button
            .accessible_name(format!("Remove output {n}"))
            .clicked();
    });
    remove
}
