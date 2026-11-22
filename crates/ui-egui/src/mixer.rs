//! The mixer: one strip per deck, the cart bus, the live input and the output.
//! Levels come from the view-model. In the preview, the mock simulates them, and nothing plays.
//! Faders, mute, PFL, routing, devices and Icecast go to the view source as edits.
//! The ⚙ opens Preferences on the mixer's pages: Devices, Routing, Processing, Meters and Icecast.

use crate::numbers;
use crate::widgets;
use egui::{Align, DragValue, Layout, RichText, Sense, Stroke};

use crate::app::Action;
use crate::context_menu::{check_item, menu_item};
use crate::icons::{self, Icon};
use crate::parts;
use crate::preferences::Page;
use crate::settings::{self, Dot};
use crate::theme::{self, Tokens, pad, size, space};
use radio_model::{DynamicsSettings, LoudnessSettings, dynamics_range as range, loudness_range};

use crate::view::{
    DeviceRole, Edit, IcecastView, MeterView, Route, StationView, Status, StreamFormat, StripChange,
};

/// The dB marks beside a meter.
const SCALE_DB: [f32; 5] = [0.0, -6.0, -12.0, -20.0, -40.0];
const NOTE: &str = "preview — simulated levels, no audio";
const RATES: [u32; 3] = [44_100, 48_000, 96_000];
const DEFAULT_RATE: u32 = 48_000;
const BITRATES: [u32; 6] = [64, 96, 128, 192, 256, 320];
/// The peak line stays this long by default, in seconds.
const DEFAULT_PEAK_HOLD_S: f32 = 1.5;
const MAX_PEAK_HOLD_S: f32 = 10.0;
const MAX_INPUT_CHANNEL: u32 = 16;
const HZ_PER_KHZ: f64 = 1000.0;

/// The tabs of the settings page, in the order of the rail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    #[default]
    Devices,
    Routing,
    Processing,
    Meters,
    Icecast,
}

impl SettingsTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Devices => "Devices",
            Self::Routing => "Routing",
            Self::Processing => "Processing",
            Self::Meters => "Meters",
            Self::Icecast => "Icecast",
        }
    }
}

/// The view state of the window. The mixer settings live in the view source; the drafts are
/// the mixer's Preferences pages while a person edits them.
#[derive(Default)]
pub(crate) struct State {
    /// The Icecast fields while a person edits them. Apply sends them. They stay while
    /// Preferences is open, across pages.
    icecast: Option<IcecastView>,
    /// The processing on Program while a person edits it. A field sends it when the edit
    /// ends, so a drag does not rebuild the processors at each step.
    dynamics: Option<DynamicsSettings>,
    /// Loudness normalisation while a person edits it. A field sends it when the edit ends.
    loudness: Option<LoudnessSettings>,
}

impl State {
    /// Drops the drafts, as Preferences closes.
    pub(crate) fn drop_drafts(&mut self) {
        self.icecast = None;
        self.dynamics = None;
        self.loudness = None;
    }
}

fn strip_edit(strip: &str, change: StripChange) -> Action {
    Action::EditQuiet(Edit::SetStrip {
        strip: strip.to_owned(),
        change,
    })
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    streaming: &str,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let _ = (state, streaming);
    let mut open = false;
    settings::header(ui, "Mixer", &mut open, |ui| {
        if view.preview {
            ui.label(
                RichText::new(NOTE)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        }
    });
    let mut wanted = open.then_some(SettingsTab::Devices);
    ui.add_space(space::S);
    ui.horizontal_top(|ui| {
        for meter in &view.meters {
            let is_out = meter.label == "Out";
            let cue = view.cue_open && view.devices.contains(&view.device_roles.pfl);
            if let Some(tab) = strip_ui(ui, meter, is_out, cue, actions, t) {
                wanted = Some(tab);
            }
        }
    });
    if let Some(tab) = wanted {
        actions.push(Action::OpenPreferences(Page::from(tab)));
    }
}

/// The status dot of the Icecast page: connected or not, in words too.
pub(crate) fn icecast_dot(view: &StationView, t: &Tokens) -> Dot {
    let ice = icecast_status(view, t);
    if ice.connected {
        Dot::Good(ice.word)
    } else {
        Dot::Bad(ice.word)
    }
}

/// The mixer's Preferences page `tab`.
pub(crate) fn preferences(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    tab: SettingsTab,
    streaming: &str,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    match tab {
        SettingsTab::Devices => devices_tab(ui, view, actions, t),
        SettingsTab::Routing => routing_tab(ui, view, actions, t),
        SettingsTab::Processing => {
            loudness_section(ui, view, &mut state.loudness, actions, t);
            processing_tab(ui, view, &mut state.dynamics, actions, t);
        }
        SettingsTab::Meters => meters_tab(ui, view, actions, t),
        SettingsTab::Icecast => {
            let ice = icecast_status(view, t);
            let draft = state.icecast.get_or_insert_with(|| view.icecast.clone());
            icecast_tab(ui, view, draft, &ice, streaming, actions, t);
        }
    }
}

fn strip_ui(
    ui: &mut egui::Ui,
    meter: &MeterView,
    is_out: bool,
    cue: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> Option<SettingsTab> {
    let mut settings_wanted: Option<SettingsTab> = None;
    let mut icecast = false;
    let mut gain = meter.gain_db;
    let mut mute = meter.mute;

    let fill = if is_out { t.selection } else { t.card };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
        .corner_radius(t.radius_lg)
        .inner_margin(pad::TIGHT)
        .show(ui, |ui| {
            ui.set_width(size::STRIP_W);
            ui.vertical_centered(|ui| {
                // Every name row has the height of a settings button, so all strips line up.
                let row = egui::vec2(ui.available_width(), size::BADGE);
                ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
                    let name = RichText::new(&meter.label).font(theme::mono(theme::CARD));
                    let r = ui.add(egui::Label::new(name.strong()).sense(Sense::click()));
                    r.context_menu(|ui| {
                        if let Some(tab) = strip_menu(ui, meter, is_out, cue, actions) {
                            settings_wanted = Some(tab);
                        }
                    });
                    if is_out {
                        settings::settings_button(ui, "Icecast", &mut icecast);
                    }
                });
                ui.horizontal(|ui| {
                    meter_bars(ui, meter, t).context_menu(|ui| {
                        if let Some(tab) = strip_menu(ui, meter, is_out, cue, actions) {
                            settings_wanted = Some(tab);
                        }
                    });
                    ui.spacing_mut().slider_width = size::METER_H;
                    let range = size::METER_MIN_DB..=size::METER_MAX_DB;
                    let fader = egui::Slider::new(&mut gain, range)
                        .vertical()
                        .show_value(false);
                    ui.add(fader)
                        .on_hover_text(format!("{} fader", meter.label));
                });
                let level = meter.level_db[0].max(meter.level_db[1]);
                let peak = meter.peak_db[0].max(meter.peak_db[1]);
                let small = |s: String| RichText::new(s).font(theme::mono(theme::CAPTION));
                ui.label(RichText::new(db(level)).font(theme::mono(theme::CODE)));
                ui.label(small(format!("pk {}", db(peak))).color(t.text_muted));
                ui.label(small(format!("gain {gain:+.1}")).color(t.text_muted));
                ui.horizontal(|ui| {
                    // Framed toggles: the word and the fill change, so the state does not hang on colour.
                    let word = if mute { "MUTED" } else { "Mute" };
                    let fill = if mute { t.danger_fill } else { t.field };
                    if toggle(ui, word, fill, t).clicked() {
                        mute = !mute;
                    }
                    if ui
                        .add_enabled(
                            cue && !is_out,
                            egui::Button::new(if cue && !is_out && meter.pfl {
                                "PFL ON"
                            } else {
                                "PFL"
                            }),
                        )
                        .on_disabled_hover_text(
                            "PFL needs a present cue device and an open cue output",
                        )
                        .clicked()
                    {
                        actions.push(strip_edit(&meter.label, StripChange::Pfl(!meter.pfl)));
                    }
                });
            });
        });
    if gain != meter.gain_db {
        actions.push(strip_edit(&meter.label, StripChange::Gain(gain)));
    }
    if mute != meter.mute {
        actions.push(mute_action(meter, is_out, mute));
    }

    // The Out ⚙ and the Settings item of Out open the Icecast tab; the others open Devices.
    if icecast {
        Some(SettingsTab::Icecast)
    } else {
        settings_wanted
    }
}

/// A framed toggle button with a fill that shows its state.
fn toggle(ui: &mut egui::Ui, word: &str, fill: egui::Color32, t: &Tokens) -> egui::Response {
    let text = RichText::new(word)
        .font(theme::text(theme::CAPTION))
        .color(t.text);
    let button = egui::Button::new(text)
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, t.field_border));
    ui.add(button)
}

/// The action for a Mute press. Muting Out silences the whole programme, so it asks first.
fn mute_action(meter: &MeterView, is_out: bool, mute: bool) -> Action {
    let edit = strip_edit(&meter.label, StripChange::Mute(mute));
    if is_out && mute {
        Action::Confirm {
            text: "Mute Out? Listeners hear silence until you unmute it.".to_owned(),
            yes: "Mute Out".to_owned(),
            then: Box::new(edit),
        }
    } else {
        edit
    }
}

/// A level as text. Silence shows as "−∞ dB".
fn db(v: f32) -> String {
    if v <= size::METER_MIN_DB {
        "−∞ dB".to_owned()
    } else {
        format!("{v:.1} dB")
    }
}

/// The part of the meter height for a level.
pub(crate) fn frac(v: f32) -> f32 {
    ((v - size::METER_MIN_DB) / (size::METER_MAX_DB - size::METER_MIN_DB)).clamp(0.0, 1.0)
}

/// The right-click menu of a strip. It returns the settings tab that the person asks for.
/// Settings of Out opens Icecast, of another strip Devices; Out also links to Processing.
fn strip_menu(
    ui: &mut egui::Ui,
    meter: &MeterView,
    is_out: bool,
    cue: bool,
    actions: &mut Vec<Action>,
) -> Option<SettingsTab> {
    if check_item(ui, meter.mute, "Mute") {
        actions.push(mute_action(meter, is_out, !meter.mute));
    }
    ui.add_enabled_ui(cue && !is_out, |ui| {
        if check_item(ui, meter.pfl, "PFL") {
            actions.push(strip_edit(&meter.label, StripChange::Pfl(!meter.pfl)));
        }
    })
    .response
    .on_disabled_hover_text("PFL needs a present cue device and an open cue output");
    if menu_item(ui, "Reset gain to 0 dB", None) {
        actions.push(strip_edit(&meter.label, StripChange::Gain(0.0)));
    }
    ui.menu_button("Route to", |ui| {
        for route in Route::ALL {
            if (route == Route::Out || cue) && check_item(ui, meter.route == route, route.label()) {
                actions.push(strip_edit(&meter.label, StripChange::Route(route)));
            }
        }
    });
    ui.separator();
    if is_out
        && menu_item(
            ui,
            &icons::lead(Icon::SlidersHorizontal, "Processing…"),
            None,
        )
    {
        return Some(SettingsTab::Processing);
    }
    let settings = menu_item(ui, &icons::lead(Icon::Settings, "Settings"), None);
    settings.then_some(if is_out {
        SettingsTab::Icecast
    } else {
        SettingsTab::Devices
    })
}

/// Two meter bars (left and right) with green, amber and red zones and a peak line.
fn meter_bars(ui: &mut egui::Ui, meter: &MeterView, t: &Tokens) -> egui::Response {
    let w = size::METER_BAR_W * 2.0 + space::XXS;
    let full = w + size::METER_SCALE_W;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(full, size::METER_H), Sense::click());
    let p = ui.painter();
    for ch in 0..2 {
        let left = rect.left() + ch as f32 * (size::METER_BAR_W + space::XXS);
        let bar = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2(size::METER_BAR_W, rect.height()),
        );
        p.rect_filled(bar, t.radius_sm, t.canvas);
        let y_of = |v: f32| bar.bottom() - bar.height() * frac(v);
        let level = meter.level_db[ch];
        for (lo, hi, colour) in [
            (size::METER_MIN_DB, size::METER_AMBER_DB, t.meter_low),
            (size::METER_AMBER_DB, size::METER_RED_DB, t.meter_mid),
            (size::METER_RED_DB, size::METER_MAX_DB, t.meter_high),
        ] {
            if level > lo {
                let seg =
                    egui::Rect::from_x_y_ranges(bar.x_range(), y_of(level.min(hi))..=y_of(lo));
                p.rect_filled(seg, 0.0, colour);
            }
        }
        let peak = meter.peak_db[ch];
        if peak > size::METER_MIN_DB {
            p.hline(
                bar.x_range(),
                y_of(peak),
                Stroke::new(size::HAIRLINE * 2.0, t.text),
            );
        }
    }
    // The scale: a mark and its dB number at the usual steps.
    for v in SCALE_DB {
        let y = rect.bottom() - rect.height() * frac(v);
        let bars = egui::Rangef::new(rect.left(), rect.left() + w);
        p.hline(bars, y, Stroke::new(size::HAIRLINE, t.field_border));
        p.text(
            egui::pos2(rect.right(), y),
            egui::Align2::RIGHT_CENTER,
            format!("{v:.0}"),
            theme::mono(theme::TINY),
            t.text_muted,
        );
    }
    resp
}

/// A dropdown of devices by name, as wide as its form cell. It returns the new name when a
/// person picks one.
fn device_pick(
    ui: &mut egui::Ui,
    id: &str,
    current: &str,
    devices: &[String],
    picked: &mut Option<String>,
) -> egui::Response {
    craft_ui::ComboBox::from_id_salt(id)
        .width(ui.available_width())
        .truncate()
        .selected_text(if current.is_empty() { "None" } else { current })
        .show_ui(ui, |ui| {
            if widgets::selectable(ui, current.is_empty(), "None").clicked() && !current.is_empty()
            {
                *picked = Some(String::new());
            }
            for d in devices {
                if widgets::selectable(ui, d == current, d).clicked() && d != current {
                    *picked = Some(d.clone());
                }
            }
        })
        .response
}

fn khz(rate: u32) -> String {
    format!("{} kHz", f64::from(rate) / HZ_PER_KHZ)
}

fn devices_tab(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, t: &Tokens) {
    let roles = &view.device_roles;
    for (label, id, role, current) in [
        (
            "Output device (Out)",
            "out_device",
            DeviceRole::Out,
            &roles.out,
        ),
        (
            "Cue device (PFL)",
            "cue_device",
            DeviceRole::Pfl,
            &roles.pfl,
        ),
        (
            "Live input device",
            "live_device",
            DeviceRole::Input,
            &roles.input,
        ),
    ] {
        let devices = if role == DeviceRole::Input {
            &view.input_devices
        } else {
            &view.devices
        };
        let mut picked = None;
        settings::form_row(ui, label, t, |ui| {
            device_pick(ui, id, current, devices, &mut picked)
        });
        if let Some(device) = picked {
            actions.push(Action::EditQuiet(Edit::SetDevice { role, device }));
        }
    }
    let mut left = parts::u32(view, parts::MIXER, "input_left", 1);
    let mut right = parts::u32(view, parts::MIXER, "input_right", 2);
    let (old_left, old_right) = (left, right);
    settings::form_row(ui, "Live input channels", t, |ui| {
        // Two halves of the one cell, with the gap of the theme between them.
        ui.spacing_mut().item_spacing.x = space::S;
        let half = (ui.available_width() - space::S) / 2.0;
        let chans = 1..=MAX_INPUT_CHANNEL;
        let h = size::CONTROL_SM;
        let l = ui.add_sized(
            [half, h],
            numbers::field(
                ui,
                DragValue::new(&mut left).range(chans.clone()).prefix("L "),
            ),
        );
        let r = ui.add_sized(
            [half, h],
            numbers::field(ui, DragValue::new(&mut right).range(chans).prefix("R ")),
        );
        l.union(r)
    });
    if left != old_left {
        actions.push(parts::set(parts::MIXER, "input_left", left));
    }
    if right != old_right {
        actions.push(parts::set(parts::MIXER, "input_right", right));
    }
    let rate = parts::u32(view, parts::MIXER, "sample_rate", DEFAULT_RATE);
    settings::form_row(ui, "Sample rate", t, |ui| {
        craft_ui::ComboBox::from_id_salt("sample_rate")
            .width(ui.available_width())
            .selected_text(khz(rate))
            .show_ui(ui, |ui| {
                for r in RATES {
                    if widgets::selectable(ui, r == rate, khz(r)).clicked() && r != rate {
                        actions.push(parts::set(parts::MIXER, "sample_rate", r));
                    }
                }
            })
            .response
    });
}

fn routing_tab(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, t: &Tokens) {
    for m in &view.meters {
        settings::form_row(ui, &m.label, t, |ui| {
            craft_ui::ComboBox::from_id_salt(("route", &m.label))
                .width(ui.available_width())
                .selected_text(m.route.label())
                .show_ui(ui, |ui| {
                    for r in Route::ALL {
                        if r != Route::OutAndCue
                            && widgets::selectable(ui, r == m.route, r.label()).clicked()
                        {
                            actions.push(strip_edit(&m.label, StripChange::Route(r)));
                        }
                    }
                })
                .response
        });
    }
}

fn meters_tab(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, t: &Tokens) {
    let mut hold = parts::f32(view, parts::MIXER, "peak_hold_s", DEFAULT_PEAK_HOLD_S);
    let old = hold;
    settings::form_row(ui, "Peak hold", t, |ui| {
        let field = numbers::field(
            ui,
            DragValue::new(&mut hold)
                .range(0.0..=MAX_PEAK_HOLD_S)
                .speed(0.1)
                .suffix(" s"),
        );
        ui.add_sized([ui.available_width(), size::CONTROL_SM], field)
    });
    if hold != old {
        actions.push(parts::set(parts::MIXER, "peak_hold_s", f64::from(hold)));
    }
}

/// A number field of the Processing tab. It returns true when an edit ends: a drag stops,
/// or a typed or keyed value changes.
fn dyn_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    (lo, hi): (f32, f32),
    speed: f64,
    suffix: &str,
    t: &Tokens,
) -> bool {
    let mut done = false;
    settings::form_row(ui, label, t, |ui| {
        let field = numbers::field(
            ui,
            DragValue::new(value)
                .range(lo..=hi)
                .speed(speed)
                .max_decimals(1)
                .suffix(suffix)
                // A typed value counts when the typing ends, not at each key.
                .update_while_editing(false),
        );
        let r = ui.add_sized([ui.available_width(), size::CONTROL_SM], field);
        done = r.drag_stopped() || (r.changed() && !r.dragged());
        r
    });
    done
}

/// Loudness normalisation on the decks, above the processing on Program. Each deck item
/// gets the gain that brings it to the target. In the preview, nothing plays.
fn loudness_section(
    ui: &mut egui::Ui,
    view: &StationView,
    draft: &mut Option<LoudnessSettings>,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let l = draft.get_or_insert(view.loudness);
    ui.add_space(space::XS);
    settings::form_note(
        ui,
        RichText::new("Loudness")
            .font(theme::text(theme::LABEL))
            .strong(),
    );
    let note = view.say(
        "Preview: nothing plays, so no item gain applies.",
        "Each deck item gets the gain that brings it to the target. A change on air ramps \
         over 1.5 s.",
    );
    settings::form_note(
        ui,
        RichText::new(note)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
    let mut commit = false;
    settings::form_row(ui, "Loudness normalisation", t, |ui| {
        let r = ui.checkbox(&mut l.enabled, "On");
        commit |= r.changed();
        r
    });
    ui.add_enabled_ui(l.enabled, |ui| {
        commit |= dyn_field(
            ui,
            "Target",
            &mut l.target_lufs,
            loudness_range::TARGET_LUFS,
            0.1,
            " LUFS",
            t,
        );
        commit |= dyn_field(
            ui,
            "Max boost",
            &mut l.max_boost_db,
            loudness_range::MAX_BOOST_DB,
            0.1,
            " dB",
            t,
        );
    });
    settings::form_note(
        ui,
        RichText::new("A boost never lifts a true peak over −1 dBTP. The deepest cut is −24 dB.")
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
    if commit && *l != view.loudness {
        actions.push(Action::EditQuiet(Edit::SetLoudness(*l)));
    }
}

/// The compressor and the limiter on Program. In the preview, nothing plays, so nothing is
/// processed. The meters carry no gain reduction, so the tab shows none.
fn processing_tab(
    ui: &mut egui::Ui,
    view: &StationView,
    draft: &mut Option<DynamicsSettings>,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let d = draft.get_or_insert(view.dynamics);
    let note = view.say(
        "Preview: nothing plays, so nothing is processed.",
        "Changes apply to Program at once, with a 15 ms crossfade.",
    );
    settings::form_note(
        ui,
        RichText::new(note)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
    let heading = |ui: &mut egui::Ui, text: &str| {
        ui.add_space(space::XS);
        settings::form_note(
            ui,
            RichText::new(text).font(theme::text(theme::LABEL)).strong(),
        );
    };
    let mut commit = false;
    heading(ui, "Compressor");
    let c = &mut d.compressor;
    settings::form_row(ui, "Compressor", t, |ui| {
        let r = ui.checkbox(&mut c.enabled, "On");
        commit |= r.changed();
        r
    });
    ui.add_enabled_ui(c.enabled, |ui| {
        commit |= dyn_field(
            ui,
            "Threshold",
            &mut c.threshold_db,
            range::THRESHOLD_DB,
            0.1,
            " dBFS",
            t,
        );
        commit |= dyn_field(ui, "Ratio", &mut c.ratio, range::RATIO, 0.05, " : 1", t);
        commit |= dyn_field(
            ui,
            "Attack",
            &mut c.attack_ms,
            range::ATTACK_MS,
            0.5,
            " ms",
            t,
        );
        commit |= dyn_field(
            ui,
            "Compressor release",
            &mut c.release_ms,
            range::COMP_RELEASE_MS,
            5.0,
            " ms",
            t,
        );
        commit |= dyn_field(
            ui,
            "Makeup gain",
            &mut c.makeup_db,
            range::MAKEUP_DB,
            0.1,
            " dB",
            t,
        );
    });
    heading(ui, "Limiter");
    let l = &mut d.limiter;
    settings::form_row(ui, "Limiter", t, |ui| {
        let r = ui.checkbox(&mut l.enabled, "On");
        commit |= r.changed();
        r
    });
    ui.add_enabled_ui(l.enabled, |ui| {
        commit |= dyn_field(
            ui,
            "Ceiling",
            &mut l.ceiling_db,
            range::CEILING_DB,
            0.1,
            " dBFS",
            t,
        );
        commit |= dyn_field(
            ui,
            "Limiter release",
            &mut l.release_ms,
            range::LIMIT_RELEASE_MS,
            5.0,
            " ms",
            t,
        );
    });
    if !l.enabled {
        settings::form_note(
            ui,
            RichText::new(icons::lead(
                Icon::TriangleAlert,
                "Limiter off: a soft clip keeps Program under −1 dBFS",
            ))
            .font(theme::text(theme::CAPTION))
            .color(t.warn),
        );
    }
    if commit && *d != view.dynamics {
        actions.push(Action::EditQuiet(Edit::SetDynamics(*d)));
    }
}

/// The warning for a password that goes to another computer without TLS.
pub(crate) const PASSWORD_IN_CLEAR: &str = "password sent unencrypted · turn on TLS";

/// The Icecast status in words, its ink, and whether it counts as connected.
struct IceStatus {
    word: String,
    icon: Icon,
    ink: egui::Color32,
    connected: bool,
}

fn icecast_status(view: &StationView, t: &Tokens) -> IceStatus {
    let s = |icon, word: &str, ink, connected| IceStatus {
        word: word.to_owned(),
        icon,
        ink,
        connected,
    };
    match &view.output.icecast {
        _ if view.preview => s(
            Icon::Circle,
            "Icecast not connected — preview",
            t.text_muted,
            false,
        ),
        Status::Connected => s(Icon::CircleDot, "Icecast connected", t.ok, true),
        Status::Reconnecting { attempt } => s(
            Icon::RefreshCw,
            &format!("Icecast connecting, try {attempt}"),
            t.warn,
            false,
        ),
        Status::Failed(e) => s(Icon::Circle, &format!("Icecast down: {e}"), t.danger, false),
        Status::NotConnected | Status::Preview => {
            s(Icon::Circle, "Icecast not connected", t.text_muted, false)
        }
    }
}

fn icecast_tab(
    ui: &mut egui::Ui,
    view: &StationView,
    ice: &mut IcecastView,
    status: &IceStatus,
    streaming: &str,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let word = icons::lead(status.icon, &status.word);
    settings::form_note(
        ui,
        RichText::new(word)
            .font(theme::text(theme::LABEL))
            .color(status.ink),
    );
    // One line, in the font of the deck titles. A long title ends in "…", and the tooltip
    // has all of it.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Now streaming:").color(t.text_muted));
        let full = format!("“{streaming}”");
        let text = RichText::new(full.as_str()).font(theme::text(theme::BODY));
        crate::widgets::truncated_label(ui, text.color(t.accent_ink)).on_hover_text(full);
    });
    let note = view.say(
        "The stream would carry this text. The preview sends nothing.",
        "Changes to the mount apply on the next start of the app.",
    );
    settings::form_note(
        ui,
        RichText::new(note)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
    ui.add_space(space::XS);
    let field = |ui: &mut egui::Ui, text: &mut String| {
        let edit = egui::TextEdit::singleline(text)
            .desired_width(ui.available_width())
            .min_size(egui::vec2(0.0, size::CONTROL_SM))
            .align(egui::Align2::LEFT_CENTER);
        ui.add(edit)
    };
    settings::form_row(ui, "Host", t, |ui| field(ui, &mut ice.host));
    settings::form_row(ui, "Port", t, |ui| {
        // A port is an address, not a quantity: 8000, never 8,000.
        let port =
            numbers::field(ui, DragValue::new(&mut ice.port).range(1..=u16::MAX)).ungrouped();
        ui.add_sized([ui.available_width(), size::CONTROL_SM], port)
    });
    settings::form_row(ui, "TLS", t, |ui| {
        ui.checkbox(&mut ice.tls, "Encrypt (TLS)")
    });
    settings::form_row(ui, "Mount", t, |ui| field(ui, &mut ice.mount));
    settings::form_row(ui, "User", t, |ui| field(ui, &mut ice.user));
    let hint = if view.icecast.password_set {
        "set · leave empty to keep it"
    } else {
        "not set"
    };
    settings::form_row(ui, "Password", t, |ui| {
        let edit = egui::TextEdit::singleline(&mut ice.password)
            .password(true)
            .hint_text(hint)
            .min_size(egui::vec2(0.0, size::CONTROL_SM))
            .align(egui::Align2::LEFT_CENTER);
        ui.add(edit.desired_width(ui.available_width()))
    });
    settings::form_row(ui, "Format", t, |ui| {
        // One button per format, splitting the cell, as the input channels do.
        ui.spacing_mut().item_spacing.x = space::S;
        let n = StreamFormat::ALL.len() as f32;
        let w = (ui.available_width() - space::S * (n - 1.0)) / n;
        let mut all: Option<egui::Response> = None;
        for f in StreamFormat::ALL {
            let button = egui::Button::new(f.label()).min_size(egui::vec2(w, size::CONTROL_SM));
            let r = widgets::steady(ui, ice.format == f, button);
            if r.clicked() {
                ice.format = f;
            }
            all = Some(match all {
                Some(a) => a.union(r),
                None => r,
            });
        }
        all.unwrap_or_else(|| ui.label(""))
    });
    settings::form_row(ui, "Bitrate", t, |ui| {
        craft_ui::ComboBox::from_id_salt("ice_kbps")
            .width(ui.available_width())
            .selected_text(format!("{} kbit/s", ice.kbps))
            .show_ui(ui, |ui| {
                for b in BITRATES {
                    widgets::selectable_value(ui, &mut ice.kbps, b, format!("{b} kbit/s"));
                }
            })
            .response
    });
    // The edited form decides: a typed password counts as a password.
    let typed = IcecastView {
        password_set: view.icecast.password_set || !ice.password.is_empty(),
        ..ice.clone()
    };
    if typed.password_in_clear() {
        settings::form_note(
            ui,
            RichText::new(icons::lead(Icon::TriangleAlert, PASSWORD_IN_CLEAR))
                .font(theme::text(theme::CAPTION))
                .color(t.warn),
        );
    }
    ui.horizontal(|ui| {
        let changed = *ice != view.icecast;
        if ui
            .add_enabled(changed, egui::Button::new("Apply"))
            .clicked()
        {
            actions.push(Action::Edit(Edit::SetIcecast(ice.clone())));
        }
        if ui
            .add_enabled(changed, egui::Button::new("Revert"))
            .clicked()
        {
            *ice = view.icecast.clone();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// A device name too long for its form cell is cut short on the button, and hovering the
    /// button shows the whole name.
    #[test]
    fn a_long_device_name_shows_whole_on_hover() {
        const NAME: &str = "Studio Interface Line Outputs 3 and 4 (Rack B)";
        let devices = vec![NAME.to_owned()];
        let mut harness = Harness::builder()
            .with_size(egui::vec2(140.0, 200.0))
            .build_ui(move |ui| {
                ui.style_mut().interaction.tooltip_delay = 0.0;
                // As in the form, the row label names the picker.
                let label = ui.label("Output");
                let mut picked = None;
                device_pick(ui, "out_device", NAME, &devices, &mut picked).labelled_by(label.id);
            });
        harness.run();
        let button = harness.get_by_role(egui::accesskit::Role::ComboBox);
        assert!(
            button.rect().width() < 140.0,
            "the button fits its cell, so the name is cut short"
        );
        button.hover();
        harness.run_steps(4);
        assert!(
            harness.query_by_label(NAME).is_some(),
            "the tooltip shows the whole name"
        );
    }
}
