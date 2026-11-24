//! The mixer: one strip per deck, the cart bus, the live input and the output.
//! Levels come from the view-model. In the preview, the mock simulates them, and nothing plays.
//! Faders, mute, SND, devices and Icecast go to the view source as edits.
//! The ⚙ opens Preferences on the mixer's pages: Devices, Processing, Meters and Icecast.

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
    DeviceRole, Edit, IcecastView, MeterView, NO_DEVICE, OutputKind, OutputMeterView, StationView,
    Status, StreamFormat, StripChange,
};

/// The settings part where the view source keeps the audio devices.
const AUDIO_PART: &str = "audio";

/// The dB marks beside a meter, as on a broadcast console. A number that would touch the one
/// above it is left out; its mark stays.
const SCALE_DB: [f32; 9] = [0.0, -3.0, -6.0, -9.0, -12.0, -18.0, -20.0, -30.0, -40.0];
const NOTE: &str = "preview — simulated levels, no audio";
/// The one rate of the audio path (the playout's `SAMPLE_RATE`): decks, carts, live input and
/// outputs all run at it, and files are resampled to it.
const AUDIO_RATE: u32 = 48_000;
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
    Processing,
    Meters,
    Icecast,
}

impl SettingsTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Devices => "Devices",
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
    if crate::windows::title_band(ui) == crate::windows::Band::None {
        // The header is a row of the content here, and the strips keep their distance from it.
        ui.add_space(space::S);
    }
    let mut page = None;
    ui.horizontal_top(|ui| {
        for meter in &view.meters {
            let is_out = meter.is_program();
            let cue = monitor_ready(view);
            if let Some(tab) = strip_ui(ui, meter, is_out, cue, actions, t) {
                wanted = Some(tab);
            }
        }
        // A wider gap sets the outputs apart from the strips that mix.
        ui.add_space(space::L);
        for out in &view.output_meters {
            if let Some(p) = output_strip(ui, out, t) {
                page = Some(p);
            }
        }
    });
    if let Some(tab) = wanted {
        actions.push(Action::OpenPreferences(Page::from(tab)));
    } else if let Some(p) = page {
        actions.push(Action::OpenPreferences(p));
    }
}

/// The live input channels of Preferences › Devices, left and right, counted from 1. The add
/// live input dialogs start from them.
pub(crate) fn input_channels(view: &StationView) -> (u32, u32) {
    (
        parts::u32(view, parts::MIXER, "input_left", 1),
        parts::u32(view, parts::MIXER, "input_right", 2),
    )
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

/// Restore page defaults on a mixer page: Processing goes back to the model's default loudness
/// and dynamics, Meters to the default peak hold.
pub(crate) fn restore_defaults(
    state: &mut State,
    tab: Option<SettingsTab>,
    actions: &mut Vec<Action>,
) {
    match tab {
        Some(SettingsTab::Processing) => {
            // The drafts take the defaults too, so the page shows them at once, before the
            // view source gives them back.
            let (loudness, dynamics) = (
                radio_model::LoudnessSettings::default(),
                radio_model::DynamicsSettings::default(),
            );
            state.loudness = Some(loudness);
            state.dynamics = Some(dynamics);
            actions.push(Action::EditQuiet(Edit::SetLoudness(loudness)));
            actions.push(Action::EditQuiet(Edit::SetDynamics(dynamics)));
        }
        Some(SettingsTab::Meters) => {
            actions.push(parts::set(
                parts::MIXER,
                "peak_hold_s",
                f64::from(DEFAULT_PEAK_HOLD_S),
            ));
        }
        _ => {}
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
    let mut devices = false;
    let mut gain = meter.gain_db;
    let mut mute = meter.mute;

    strip_frame(ui, t, |ui| {
        let gear = is_out.then_some(&mut devices);
        let head = strip_head(ui, &meter.label, is_out, gear, t);
        head.context_menu(|ui| {
            if let Some(tab) = strip_menu(ui, meter, is_out, cue, actions) {
                settings_wanted = Some(tab);
            }
        });
        ui.horizontal_top(|ui| {
            meter_bars(ui, meter.level_db, meter.peak_db, t).context_menu(|ui| {
                if let Some(tab) = strip_menu(ui, meter, is_out, cue, actions) {
                    settings_wanted = Some(tab);
                }
            });
            // The fader sits in the middle of the room beside the bars.
            let room = egui::vec2(ui.available_width(), size::METER_H);
            ui.allocate_ui_with_layout(room, Layout::top_down(Align::Center), |ui| {
                ui.spacing_mut().slider_width = size::METER_H;
                let range = size::METER_MIN_DB..=size::METER_MAX_DB;
                let fader = egui::Slider::new(&mut gain, range)
                    .vertical()
                    .show_value(false);
                ui.add(fader)
                    .on_hover_text(format!("{} fader", meter.label));
            });
        });
        // Under the bars the held peak; at the right, under the fader, its gain.
        let peak = meter.peak_db[0].max(meter.peak_db[1]);
        let (row, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), size::CONTROL_SM),
            Sense::hover(),
        );
        let p = ui.painter();
        let small = theme::mono(theme::CAPTION);
        p.text(
            egui::pos2(row.left() + bars_centre(), row.center().y),
            egui::Align2::CENTER_CENTER,
            db_short(peak),
            small.clone(),
            t.text,
        );
        p.text(
            egui::pos2(row.right(), row.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("{gain:+.1}"),
            small,
            t.text_muted,
        );
        // Framed toggles: the word and the fill change, so the state does not hang on
        // colour. Each name starts with the visible word and says which strip it is.
        let word = if mute { "MUTED" } else { "Mute" };
        let fill = if mute { t.danger_fill } else { t.field };
        if toggle(ui, true, word, &meter.label, fill, t).clicked() {
            mute = !mute;
        }
        let on = cue && meter.snd;
        let word = if on { "SND ON" } else { "SND" };
        let fill = if on { t.warn_fill } else { t.field };
        let snd = toggle(ui, cue, word, &meter.label, fill, t).on_disabled_hover_text(SND_NEEDS);
        if snd.clicked() {
            actions.push(strip_edit(&meter.strip, StripChange::Snd(!meter.snd)));
        }
    });
    if gain != meter.gain_db {
        actions.push(strip_edit(&meter.strip, StripChange::Gain(gain)));
    }
    if mute != meter.mute {
        actions.push(mute_action(meter, is_out, mute));
    }

    // The Program ⚙ opens Devices, where its sound card is picked.
    if devices {
        Some(SettingsTab::Devices)
    } else {
        settings_wanted
    }
}

/// The card of one strip, at the strip width, with its parts centred.
fn strip_frame(ui: &mut egui::Ui, t: &Tokens, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
        .corner_radius(t.radius_lg)
        .inner_margin(pad::TIGHT)
        .show(ui, |ui| {
            ui.set_width(size::STRIP_W);
            ui.spacing_mut().item_spacing.y = space::XS;
            ui.vertical_centered(body);
        });
}

/// The name band at the top of a strip. Program's band has the selection fill, and its name
/// says so too. With `gear`, a ⚙ at the right opens the strip's Preferences page. It returns
/// the name, which takes a right-click.
fn strip_head(
    ui: &mut egui::Ui,
    label: &str,
    accent: bool,
    gear: Option<&mut bool>,
    t: &Tokens,
) -> egui::Response {
    let (band, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::BADGE),
        Sense::hover(),
    );
    let fill = if accent { t.selection } else { t.field };
    ui.painter().rect_filled(band, t.radius_sm, fill);
    let mut name_rect = band;
    if let Some(open) = gear {
        let side = band.height();
        let at = egui::Rect::from_min_size(
            egui::pos2(band.right() - side, band.top()),
            egui::Vec2::splat(side),
        );
        // The name keeps clear of the button and of the left edge of the band.
        name_rect.set_right(at.left());
        name_rect.set_left(band.left() + space::XS);
        ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
            settings::settings_button_sized(ui, label, open, side);
        });
    }
    // One size for every strip, small enough that "Program" fits beside its button.
    let font = theme::text(theme::LABEL);
    let name = RichText::new(label).font(font).color(t.text).strong();
    ui.put(
        name_rect,
        egui::Label::new(name).truncate().sense(Sense::click()),
    )
}

/// The x of the middle of the two bars, from the left edge of [`meter_bars`].
fn bars_centre() -> f32 {
    size::METER_SCALE_W + space::XS + size::METER_BAR_W + space::XXS / 2.0
}

/// A level as a short number for under a meter. Silence shows as "−∞".
fn db_short(v: f32) -> String {
    if v <= size::METER_MIN_DB {
        "−∞".to_owned()
    } else {
        format!("{v:.1}")
    }
}

/// Why SND is off: it plays on the monitor device, which must be there and open.
const SND_NEEDS: &str = "SND needs a present monitor device and an open monitor output";

/// The monitor output plays, so SND can be heard.
fn monitor_ready(view: &StationView) -> bool {
    view.monitor_open && view.devices.contains(&view.device_roles.monitor)
}

/// A framed toggle button with a fill that shows its state. Its name is the word and the
/// strip, for example "SND A".
fn toggle(
    ui: &mut egui::Ui,
    enabled: bool,
    word: &str,
    strip: &str,
    fill: egui::Color32,
    t: &Tokens,
) -> egui::Response {
    let text = RichText::new(word)
        .font(theme::text(theme::CAPTION))
        .color(t.text);
    let button = egui::Button::new(text)
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, t.field_border))
        .min_size(egui::vec2(ui.available_width(), size::CONTROL_SM));
    let r = ui.add_enabled(enabled, button);
    r.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, enabled, format!("{word} {strip}"))
    });
    r
}

/// One output strip: the level of what that output sends, and its state in words. A down
/// output shows empty bars. Its ⚙ returns the page of that output.
fn output_strip(ui: &mut egui::Ui, out: &OutputMeterView, t: &Tokens) -> Option<Page> {
    let mut open = false;
    strip_frame(ui, t, |ui| {
        strip_head(ui, &out.label, false, Some(&mut open), t);
        let silent = [size::METER_MIN_DB; 2];
        let (level, peak) = if out.live {
            (out.level_db, out.peak_db)
        } else {
            (silent, silent)
        };
        meter_bars(ui, level, peak, t);
        let row = egui::vec2(ui.available_width(), size::CONTROL_SM);
        let (row, _) = ui.allocate_exact_size(row, Sense::hover());
        ui.painter().text(
            row.center(),
            egui::Align2::CENTER_CENTER,
            db_short(peak[0].max(peak[1])),
            theme::mono(theme::CAPTION),
            if out.live { t.text } else { t.text_muted },
        );
        // The state in words, in the room that the toggles take on a mixing strip.
        let foot = egui::vec2(
            ui.available_width(),
            size::CONTROL_SM * 2.0 + ui.spacing().item_spacing.y,
        );
        let centred = Layout::top_down(Align::Center).with_main_align(Align::Center);
        ui.allocate_ui_with_layout(foot, centred, |ui| {
            ui.set_min_size(foot);
            let colour = if out.live { t.text } else { t.text_muted };
            let text = RichText::new(&out.note)
                .font(theme::text(theme::CAPTION))
                .color(colour);
            let note = ui.add(egui::Label::new(text).wrap());
            note.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Label,
                    true,
                    format!("{}: {}", out.label, out.note),
                )
            });
        });
    });
    open.then_some(match out.kind {
        OutputKind::Device => Page::Devices,
        OutputKind::Icecast => Page::Icecast,
        OutputKind::WebRtc => Page::WebRtc,
    })
}

/// The action for a Mute press. Muting Out silences the whole programme, so it asks first.
fn mute_action(meter: &MeterView, is_out: bool, mute: bool) -> Action {
    let edit = strip_edit(&meter.strip, StripChange::Mute(mute));
    if is_out && mute {
        Action::Confirm {
            text: format!(
                "Mute {}? Listeners hear silence until you unmute it.",
                meter.label
            ),
            yes: format!("Mute {}", meter.label),
            then: Box::new(edit),
        }
    } else {
        edit
    }
}

/// The part of the meter height for a level.
pub(crate) fn frac(v: f32) -> f32 {
    ((v - size::METER_MIN_DB) / (size::METER_MAX_DB - size::METER_MIN_DB)).clamp(0.0, 1.0)
}

/// The right-click menu of a strip. It returns the settings tab that the person asks for.
/// Settings opens Devices; Program also links to Processing.
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
    ui.add_enabled_ui(cue, |ui| {
        if check_item(ui, meter.snd, "SND") {
            actions.push(strip_edit(&meter.strip, StripChange::Snd(!meter.snd)));
        }
    })
    .response
    .on_disabled_hover_text(SND_NEEDS);
    if menu_item(ui, "Reset gain to 0 dB", None) {
        actions.push(strip_edit(&meter.strip, StripChange::Gain(0.0)));
    }
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
    // As the ⚙ of Program: every strip's settings are its devices.
    let settings = menu_item(ui, &icons::lead(Icon::Settings, "Settings"), None);
    settings.then_some(SettingsTab::Devices)
}

/// Two LED meter bars (left and right) with green, amber and red zones and a held peak, and
/// the dB scale at their left. Unlit LEDs show their zone faintly.
fn meter_bars(
    ui: &mut egui::Ui,
    level_db: [f32; 2],
    peak_db: [f32; 2],
    t: &Tokens,
) -> egui::Response {
    let bars_w = size::METER_BAR_W * 2.0 + space::XXS;
    let full = size::METER_SCALE_W + space::XS + bars_w;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(full, size::METER_H), Sense::click());
    let p = ui.painter();
    let bars_left = rect.right() - bars_w;
    let step = size::METER_SEG_H + size::METER_SEG_GAP;
    let count = (rect.height() / step).floor().max(1.0);
    let span = size::METER_MAX_DB - size::METER_MIN_DB;
    for ch in 0..2 {
        let left = bars_left + ch as f32 * (size::METER_BAR_W + space::XXS);
        let x = egui::Rangef::new(left, left + size::METER_BAR_W);
        let peak_led = ((frac(peak_db[ch]) * count).ceil() - 1.0).max(-1.0);
        let mut k = 0.0;
        while k < count {
            // The LED is lit when the level passes its lower edge.
            let low = size::METER_MIN_DB + span * k / count;
            let zone = if low >= size::METER_RED_DB {
                t.meter_high
            } else if low >= size::METER_AMBER_DB {
                t.meter_mid
            } else {
                t.meter_low
            };
            let colour = if k == peak_led {
                t.text
            } else if level_db[ch] > low {
                zone
            } else {
                zone.gamma_multiply(size::METER_UNLIT)
            };
            let bottom = rect.bottom() - k * step;
            let led = egui::Rect::from_x_y_ranges(x, (bottom - size::METER_SEG_H)..=bottom);
            p.rect_filled(led, 0.0, colour);
            k += 1.0;
        }
    }
    // The scale: a short mark at each step, and its number where it does not touch the last.
    let font = theme::mono(theme::TINY);
    let line = ui.fonts_mut(|f| f.row_height(&font));
    let mut last_label = f32::NEG_INFINITY;
    for v in SCALE_DB {
        let y = rect.bottom() - rect.height() * frac(v);
        let mark = egui::Rangef::new(bars_left - space::XS, bars_left - space::XXS);
        p.hline(mark, y, Stroke::new(size::HAIRLINE, t.text_muted));
        if y - last_label >= line {
            p.text(
                egui::pos2(bars_left - space::XS - space::XXS, y),
                egui::Align2::RIGHT_CENTER,
                format!("{v:.0}"),
                font.clone(),
                t.text_muted,
            );
            last_label = y;
        }
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

/// The Program device dropdown: None (no sound card), System default, or a device. `saved`
/// is the setting: `NO_DEVICE`, empty for the system default, or a name.
fn program_device_pick(
    ui: &mut egui::Ui,
    saved: &str,
    devices: &[String],
    picked: &mut Option<String>,
) -> egui::Response {
    const SYSTEM_DEFAULT: &str = "System default";
    let shown = match saved {
        NO_DEVICE => "None",
        "" => SYSTEM_DEFAULT,
        name => name,
    };
    craft_ui::ComboBox::from_id_salt("out_device")
        .width(ui.available_width())
        .truncate()
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (word, value) in [("None", NO_DEVICE), (SYSTEM_DEFAULT, "")] {
                if widgets::selectable(ui, saved == value, word).clicked() && saved != value {
                    *picked = Some(value.to_owned());
                }
            }
            for d in devices {
                if widgets::selectable(ui, d == saved, d).clicked() && d != saved {
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
    // The output device opens when the playout starts: a new pick waits for the next start,
    // so the row shows the pick, and a note says which device plays until then.
    let saved_out = view
        .settings
        .get(AUDIO_PART)
        .and_then(|a| a.get("out"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    settings::section(ui, "Audio devices", t, |ui| {
        let mut picked = None;
        settings::form_row(ui, "Program device", t, |ui| {
            program_device_pick(ui, &saved_out, &view.devices, &mut picked)
        });
        if let Some(device) = picked {
            actions.push(Action::EditQuiet(Edit::SetDevice {
                role: DeviceRole::Out,
                device,
            }));
        }
        // The sound card that plays until the next start, when it differs from the pick. A
        // pick of the system default names no card, so there is nothing to compare.
        let wanted = match saved_out.as_str() {
            NO_DEVICE => Some(""),
            "" => None,
            name => Some(name),
        };
        // The system default names no card, but with none open it too waits for a start.
        let waits = match wanted {
            Some(w) => w != roles.out,
            None => roles.out.is_empty(),
        };
        if !view.preview && waits {
            let now = if roles.out.is_empty() {
                "no sound card"
            } else {
                roles.out.as_str()
            };
            settings::form_note(
                ui,
                RichText::new(format!(
                    "Opens at the next start. Until then the sound goes to {now}."
                ))
                .font(theme::text(theme::CAPTION))
                .color(t.warn),
            );
        }
        for (label, id, role, current) in [
            (
                "Monitor device (SND)",
                "monitor_device",
                DeviceRole::Monitor,
                &roles.monitor,
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
    });
    settings::section(ui, "Signal", t, |ui| {
        let (mut left, mut right) = input_channels(view);
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
        // A fixed fact, not a choice: a picker here would change nothing.
        settings::form_row(ui, "Sample rate", t, |ui| {
            ui.label(format!("{}, fixed", khz(AUDIO_RATE)))
                .on_hover_text("The audio always runs at this rate; files are resampled to it")
        });
    });
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
    let note = view.say(
        "Preview: the simulated meters keep their own hold.",
        "The meters take a new peak hold at the next start.",
    );
    settings::form_note(
        ui,
        RichText::new(note)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
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
    let mut commit = false;
    settings::section(ui, "Loudness", t, |ui| {
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
            RichText::new(
                "A boost never lifts a true peak over −1 dBTP. The deepest cut is −24 dB.",
            )
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
        );
    });
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
    let mut commit = false;
    let c = &mut d.compressor;
    settings::section(ui, "Compressor", t, |ui| {
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
    });
    let l = &mut d.limiter;
    settings::section(ui, "Limiter", t, |ui| {
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
    });
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
    settings::section(ui, "Status", t, |ui| {
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
            "Applied changes take effect at the next start of the app.",
        );
        settings::form_note(
            ui,
            RichText::new(note)
                .font(theme::text(theme::CAPTION))
                .color(t.text_muted),
        );
    });
    settings::section(ui, "Server", t, |ui| {
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
        // The password is kept for one server: user, host, port and mount. A change of any of
        // them needs the password typed again, or Icecast stays off.
        let same_server = ice.user == view.icecast.user
            && ice.host == view.icecast.host
            && ice.port == view.icecast.port
            && ice.mount == view.icecast.mount;
        let hint = match (view.icecast.password_set, same_server) {
            (true, true) => "set · leave empty to keep it",
            (true, false) => "type it again for this server",
            (false, _) => "not set",
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
        settings::form_row(ui, "Public address", t, |ui| {
            let edit = egui::TextEdit::singleline(&mut ice.public_url)
                .hint_text("https://stream.example.org/live")
                .desired_width(ui.available_width())
                .min_size(egui::vec2(0.0, size::CONTROL_SM))
                .align(egui::Align2::LEFT_CENTER);
            ui.add(edit)
        });
        settings::form_note(
            ui,
            RichText::new(
                "The address listeners open. The web player plays it when WebRTC cannot \
                 connect, and the station's link carries it. Leave it empty to keep it private.",
            )
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
        );
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
