//! The mixer: one strip per deck, the cart bus, the live input and the output.
//! Levels come from the view-model. In the preview, the mock simulates them, and nothing plays.
//! Faders, mute, PFL, routing, devices and Icecast go to the view source as edits.

use egui::{Align, DragValue, Layout, RichText, Sense, Stroke};

use crate::app::Action;
use crate::context_menu::{check_item, menu_item};
use crate::icons::{self, Icon};
use crate::parts;
use crate::settings;
use crate::theme::{self, Tokens, pad, size, space};
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

/// The view state of the window. The mixer settings live in the view source.
#[derive(Default)]
pub(crate) struct State {
    settings: bool,
    icecast_open: bool,
    /// The Icecast fields while a person edits them. Apply sends them.
    icecast: Option<IcecastView>,
}

impl State {
    /// Open the Icecast settings of the Out strip.
    pub(crate) fn open_icecast(&mut self) {
        self.icecast_open = true;
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
    let mut open = state.settings;
    settings::header(ui, "Mixer", &mut open, |ui| {
        if view.preview {
            ui.label(
                RichText::new(NOTE)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        }
    });
    state.settings = open;
    ui.add_space(space::S);
    // The strips stay in view while the settings are open: the settings sit beside them.
    let mut open_settings = false;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.horizontal_top(|ui| {
                for meter in &view.meters {
                    let is_out = meter.label == "Out";
                    open_settings |=
                        strip_ui(ui, meter, is_out, &mut state.icecast_open, actions, t);
                }
            });
        });
        if !state.settings && !state.icecast_open {
            state.icecast = None;
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("mixer_side")
            .max_width(size::MIXER_SIDE_W)
            .show(ui, |ui| {
                ui.set_width(size::MIXER_SIDE_W);
                ui.vertical(|ui| {
                    if state.settings {
                        settings::section(ui, "Mixer", view.preview, t, |ui| {
                            mixer_settings(ui, view, actions, t)
                        });
                    }
                    if state.icecast_open {
                        let draft = state.icecast.get_or_insert_with(|| view.icecast.clone());
                        settings::section(ui, "Icecast", view.preview, t, |ui| {
                            icecast_settings(ui, view, draft, streaming, actions, t);
                        });
                    } else {
                        state.icecast = None;
                    }
                });
            });
    });
    if open_settings {
        state.settings = true;
    }
}

fn strip_ui(
    ui: &mut egui::Ui,
    meter: &MeterView,
    is_out: bool,
    icecast: &mut bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> bool {
    let mut settings_wanted = false;
    let mut gain = meter.gain_db;
    let mut mute = meter.mute;
    let mut pfl = meter.pfl;
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
                    r.context_menu(|ui| settings_wanted |= strip_menu(ui, meter, is_out, actions));
                    if is_out {
                        settings::settings_button(ui, "Icecast", icecast);
                    }
                });
                ui.horizontal(|ui| {
                    meter_bars(ui, meter, t).context_menu(|ui| {
                        settings_wanted |= strip_menu(ui, meter, is_out, actions)
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
                    let word = if pfl { "PFL ON" } else { "PFL" };
                    let fill = if pfl { t.warn_fill } else { t.field };
                    if toggle(ui, word, fill, t).clicked() {
                        pfl = !pfl;
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
    if pfl != meter.pfl {
        actions.push(strip_edit(&meter.label, StripChange::Pfl(pfl)));
    }
    settings_wanted
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

/// The right-click menu of a strip. It returns true when the person asks for the settings.
fn strip_menu(
    ui: &mut egui::Ui,
    meter: &MeterView,
    is_out: bool,
    actions: &mut Vec<Action>,
) -> bool {
    if check_item(ui, meter.mute, "Mute") {
        actions.push(mute_action(meter, is_out, !meter.mute));
    }
    if check_item(ui, meter.pfl, "PFL") {
        actions.push(strip_edit(&meter.label, StripChange::Pfl(!meter.pfl)));
    }
    if menu_item(ui, "Reset gain to 0 dB", None) {
        actions.push(strip_edit(&meter.label, StripChange::Gain(0.0)));
    }
    ui.menu_button("Route to", |ui| {
        for route in Route::ALL {
            if check_item(ui, meter.route == route, route.label()) {
                actions.push(strip_edit(&meter.label, StripChange::Route(route)));
            }
        }
    });
    ui.separator();
    menu_item(ui, &icons::lead(Icon::Settings, "Settings"), None)
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

/// A dropdown of devices by name. It returns the new name when a person picks one.
fn device_pick(ui: &mut egui::Ui, id: &str, current: &str, devices: &[String]) -> Option<String> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .width(size::COMBO_W)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for d in devices {
                if ui.selectable_label(d == current, d).clicked() && d != current {
                    picked = Some(d.clone());
                }
            }
        });
    picked
}

fn mixer_settings(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, t: &Tokens) {
    let roles = &view.device_roles;
    let rate = parts::u32(view, parts::MIXER, "sample_rate", DEFAULT_RATE);
    let mut left = parts::u32(view, parts::MIXER, "input_left", 1);
    let mut right = parts::u32(view, parts::MIXER, "input_right", 2);
    let mut hold = parts::f32(view, parts::MIXER, "peak_hold_s", DEFAULT_PEAK_HOLD_S);
    let (old_left, old_right, old_hold) = (left, right, hold);
    egui::Grid::new("mixer_settings")
        .spacing(egui::vec2(space::M, space::S))
        .show(ui, |ui| {
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
                settings::label(ui, label, t);
                if let Some(device) = device_pick(ui, id, current, &view.devices) {
                    actions.push(Action::EditQuiet(Edit::SetDevice { role, device }));
                }
                ui.end_row();
            }
            settings::label(ui, "Live input channels", t);
            ui.horizontal(|ui| {
                let chans = 1..=MAX_INPUT_CHANNEL;
                ui.add(DragValue::new(&mut left).range(chans.clone()).prefix("L "));
                ui.add(DragValue::new(&mut right).range(chans).prefix("R "));
            });
            ui.end_row();
            settings::label(ui, "Sample rate", t);
            egui::ComboBox::from_id_salt("sample_rate")
                .width(size::COMBO_W)
                .selected_text(format!("{} kHz", f64::from(rate) / HZ_PER_KHZ))
                .show_ui(ui, |ui| {
                    for r in RATES {
                        let label = format!("{} kHz", f64::from(r) / HZ_PER_KHZ);
                        if ui.selectable_label(r == rate, label).clicked() && r != rate {
                            actions.push(parts::set(parts::MIXER, "sample_rate", r));
                        }
                    }
                });
            ui.end_row();
            settings::label(ui, "Peak hold", t);
            let field = DragValue::new(&mut hold)
                .range(0.0..=MAX_PEAK_HOLD_S)
                .speed(0.1);
            ui.add(field.suffix(" s"));
            ui.end_row();
        });
    if left != old_left {
        actions.push(parts::set(parts::MIXER, "input_left", left));
    }
    if right != old_right {
        actions.push(parts::set(parts::MIXER, "input_right", right));
    }
    if hold != old_hold {
        actions.push(parts::set(parts::MIXER, "peak_hold_s", f64::from(hold)));
    }
    ui.add_space(space::S);
    settings::label(ui, "Routing per strip", t);
    egui::Grid::new("mixer_routing")
        .spacing(egui::vec2(space::M, space::XS))
        .show(ui, |ui| {
            for m in &view.meters {
                ui.label(RichText::new(&m.label).font(theme::mono(theme::CODE)));
                egui::ComboBox::from_id_salt(("route", &m.label))
                    .width(size::COMBO_W)
                    .selected_text(m.route.label())
                    .show_ui(ui, |ui| {
                        for r in Route::ALL {
                            if ui.selectable_label(r == m.route, r.label()).clicked() {
                                actions.push(strip_edit(&m.label, StripChange::Route(r)));
                            }
                        }
                    });
                ui.end_row();
            }
        });
}

/// The warning for a password that goes to another computer without TLS.
pub(crate) const PASSWORD_IN_CLEAR: &str = "password sent unencrypted · turn on TLS";

fn icecast_settings(
    ui: &mut egui::Ui,
    view: &StationView,
    ice: &mut IcecastView,
    streaming: &str,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let (word, ink) = match &view.output.icecast {
        _ if view.preview => (
            icons::lead(Icon::Circle, "Icecast not connected — preview"),
            t.text_muted,
        ),
        Status::Connected => (icons::lead(Icon::CircleDot, "Icecast connected"), t.ok),
        Status::Reconnecting { attempt } => (
            icons::lead(
                Icon::RefreshCw,
                &format!("Icecast connecting, try {attempt}"),
            ),
            t.warn,
        ),
        Status::Failed(e) => (
            icons::lead(Icon::Circle, &format!("Icecast down: {e}")),
            t.danger,
        ),
        Status::NotConnected | Status::Preview => (
            icons::lead(Icon::Circle, "Icecast not connected"),
            t.text_muted,
        ),
    };
    let status = RichText::new(word).font(theme::text(theme::LABEL));
    ui.label(status.color(ink));
    ui.horizontal(|ui| {
        ui.label(RichText::new("Now streaming:").color(t.text_muted));
        let text = RichText::new(format!("“{streaming}”")).font(theme::mono(theme::CODE));
        ui.label(text.color(t.accent_ink));
    });
    let note = view.say(
        "The stream would carry this text. The preview sends nothing.",
        "Changes to the mount apply on the next start of the app.",
    );
    ui.label(
        RichText::new(note)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
    ui.add_space(space::S);
    let field = |ui: &mut egui::Ui, text: &mut String, secret: bool| {
        let edit = egui::TextEdit::singleline(text).password(secret);
        ui.add(edit.desired_width(size::FIELD_W));
    };
    egui::Grid::new("icecast_settings")
        .spacing(egui::vec2(space::M, space::S))
        .show(ui, |ui| {
            settings::label(ui, "Host", t);
            field(ui, &mut ice.host, false);
            ui.end_row();
            settings::label(ui, "Port", t);
            ui.add(DragValue::new(&mut ice.port).range(1..=u16::MAX));
            ui.end_row();
            settings::label(ui, "TLS", t);
            ui.checkbox(&mut ice.tls, "Encrypt (TLS)");
            ui.end_row();
            settings::label(ui, "Mount", t);
            field(ui, &mut ice.mount, false);
            ui.end_row();
            settings::label(ui, "User", t);
            field(ui, &mut ice.user, false);
            ui.end_row();
            settings::label(ui, "Password", t);
            let hint = if view.icecast.password_set {
                "set · leave empty to keep it"
            } else {
                "not set"
            };
            let edit = egui::TextEdit::singleline(&mut ice.password)
                .password(true)
                .hint_text(hint);
            ui.add(edit.desired_width(size::FIELD_W));
            ui.end_row();
            settings::label(ui, "Format", t);
            ui.horizontal(|ui| {
                for f in StreamFormat::ALL {
                    if ui.selectable_label(ice.format == f, f.label()).clicked() {
                        ice.format = f;
                    }
                }
            });
            ui.end_row();
            settings::label(ui, "Bitrate", t);
            egui::ComboBox::from_id_salt("ice_kbps")
                .width(size::COMBO_W)
                .selected_text(format!("{} kbit/s", ice.kbps))
                .show_ui(ui, |ui| {
                    for b in BITRATES {
                        ui.selectable_value(&mut ice.kbps, b, format!("{b} kbit/s"));
                    }
                });
            ui.end_row();
        });
    // The edited form decides: a typed password counts as a password.
    let typed = IcecastView {
        password_set: view.icecast.password_set || !ice.password.is_empty(),
        ..ice.clone()
    };
    if typed.password_in_clear() {
        ui.label(
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
