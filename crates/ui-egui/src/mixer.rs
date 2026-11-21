//! The mixer: one strip per deck, the cart bus, the live input and the output.
//! Levels come from the view-model. In the preview, the mock simulates them, and nothing plays.
//! Faders, mute and PFL change only this window.

use egui::{DragValue, RichText, Sense, Stroke};

use crate::settings;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{MeterView, StationView};

const NOTE: &str =
    "preview — simulated levels, no audio · faders, mute and PFL change this window only";
const RATES: [u32; 3] = [44_100, 48_000, 96_000];
const ROUTES: [&str; 3] = ["Out", "Out + cue", "Cue only"];
const FORMATS: [&str; 4] = ["MP3", "Ogg Vorbis", "Opus", "AAC"];
const BITRATES: [u32; 6] = [64, 96, 128, 192, 256, 320];

struct Strip {
    label: String,
    gain_db: f32,
    mute: bool,
    pfl: bool,
    route: usize,
}

/// Icecast mount settings. The preview never connects.
struct Icecast {
    open: bool,
    host: String,
    port: u16,
    mount: String,
    user: String,
    password: String,
    format: usize,
    bitrate: usize,
}

/// The mixer state. All of it is local to the window.
pub(crate) struct State {
    strips: Vec<Strip>,
    settings: bool,
    out_device: usize,
    cue_device: usize,
    live_device: usize,
    live_channels: (u32, u32),
    rate: usize,
    peak_hold_s: f32,
    icecast: Icecast,
}

impl Default for State {
    fn default() -> Self {
        Self {
            strips: Vec::new(),
            settings: false,
            out_device: 0,
            cue_device: 3,
            live_device: 1,
            live_channels: (1, 2),
            rate: 1,
            peak_hold_s: 1.5,
            icecast: Icecast {
                open: false,
                host: "stream.example.org".to_owned(),
                port: 8000,
                mount: "/live".to_owned(),
                user: "source".to_owned(),
                password: String::new(),
                format: 0,
                bitrate: 3,
            },
        }
    }
}

pub(crate) fn show(ui: &mut egui::Ui, view: &StationView, state: &mut State, t: &Tokens) {
    // Keep one local strip per meter, in the order of the view.
    let same = state.strips.len() == view.meters.len()
        && state
            .strips
            .iter()
            .zip(&view.meters)
            .all(|(s, m)| s.label == m.label);
    if !same {
        state.strips = view
            .meters
            .iter()
            .map(|m| Strip {
                label: m.label.clone(),
                gain_db: m.gain_db,
                mute: false,
                pfl: false,
                route: 0,
            })
            .collect();
    }
    let mut open = state.settings;
    settings::header(ui, "Mixer", &mut open, |_| {});
    state.settings = open;
    ui.label(
        RichText::new(NOTE)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
    ui.add_space(space::S);
    egui::ScrollArea::vertical()
        .id_salt("mixer_body")
        .show(ui, |ui| {
            if state.settings {
                settings::section(ui, "Mixer", t, |ui| mixer_settings(ui, view, state, t));
            }
            if state.icecast.open {
                settings::section(ui, "Icecast", t, |ui| {
                    icecast_settings(ui, &mut state.icecast, t)
                });
            }
            ui.horizontal_top(|ui| {
                for (meter, strip) in view.meters.iter().zip(state.strips.iter_mut()) {
                    let is_out = meter.label == "Out";
                    strip_ui(ui, meter, strip, is_out, &mut state.icecast.open, t);
                }
            });
        });
}

fn strip_ui(
    ui: &mut egui::Ui,
    meter: &MeterView,
    strip: &mut Strip,
    is_out: bool,
    icecast: &mut bool,
    t: &Tokens,
) {
    let fill = if is_out { t.selection } else { t.card };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
        .corner_radius(t.radius_lg)
        .inner_margin(pad::TIGHT)
        .show(ui, |ui| {
            ui.set_width(size::STRIP_W);
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    let name = RichText::new(&meter.label).font(theme::mono(theme::CARD));
                    ui.label(name.strong());
                    if is_out {
                        settings::settings_button(ui, "Icecast", icecast);
                    }
                });
                ui.horizontal(|ui| {
                    meter_bars(ui, meter, t);
                    ui.spacing_mut().slider_width = size::METER_H;
                    let range = size::METER_MIN_DB..=size::METER_MAX_DB;
                    let fader = egui::Slider::new(&mut strip.gain_db, range)
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
                ui.label(small(format!("gain {:+.1}", strip.gain_db)).color(t.text_muted));
                ui.horizontal(|ui| {
                    let cap = |s: &str| RichText::new(s).font(theme::text(theme::CAPTION));
                    ui.toggle_value(&mut strip.mute, cap("Mute"));
                    ui.toggle_value(&mut strip.pfl, cap("PFL"));
                });
            });
        });
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
fn frac(v: f32) -> f32 {
    ((v - size::METER_MIN_DB) / (size::METER_MAX_DB - size::METER_MIN_DB)).clamp(0.0, 1.0)
}

/// Two meter bars (left and right) with green, amber and red zones and a peak line.
fn meter_bars(ui: &mut egui::Ui, meter: &MeterView, t: &Tokens) {
    let w = size::METER_BAR_W * 2.0 + space::XXS;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, size::METER_H), Sense::hover());
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
    // Scale marks at the zone edges and at 0 dB.
    for v in [size::METER_AMBER_DB, size::METER_RED_DB, 0.0] {
        let y = rect.bottom() - rect.height() * frac(v);
        p.hline(
            rect.x_range(),
            y,
            Stroke::new(size::HAIRLINE, t.field_border),
        );
    }
}

fn mixer_settings(ui: &mut egui::Ui, view: &StationView, state: &mut State, t: &Tokens) {
    let rates: Vec<String> = RATES
        .iter()
        .map(|r| format!("{} kHz", f64::from(*r) / 1000.0))
        .collect();
    egui::Grid::new("mixer_settings")
        .spacing(egui::vec2(space::M, space::S))
        .show(ui, |ui| {
            settings::label(ui, "Output device (Out)", t);
            settings::pick(ui, "out_device", &mut state.out_device, &view.devices);
            ui.end_row();
            settings::label(ui, "Cue device (PFL)", t);
            settings::pick(ui, "cue_device", &mut state.cue_device, &view.devices);
            ui.end_row();
            settings::label(ui, "Live input device", t);
            settings::pick(ui, "live_device", &mut state.live_device, &view.devices);
            ui.end_row();
            settings::label(ui, "Live input channels", t);
            ui.horizontal(|ui| {
                ui.add(
                    DragValue::new(&mut state.live_channels.0)
                        .range(1..=16)
                        .prefix("L "),
                );
                ui.add(
                    DragValue::new(&mut state.live_channels.1)
                        .range(1..=16)
                        .prefix("R "),
                );
            });
            ui.end_row();
            settings::label(ui, "Sample rate", t);
            settings::pick(ui, "sample_rate", &mut state.rate, &rates);
            ui.end_row();
            settings::label(ui, "Peak hold", t);
            let hold = DragValue::new(&mut state.peak_hold_s)
                .range(0.0..=10.0)
                .speed(0.1);
            ui.add(hold.suffix(" s"));
            ui.end_row();
        });
    ui.add_space(space::S);
    settings::label(ui, "Routing per strip", t);
    let routes: Vec<String> = ROUTES.iter().map(|r| (*r).to_owned()).collect();
    egui::Grid::new("mixer_routing")
        .spacing(egui::vec2(space::M, space::XS))
        .show(ui, |ui| {
            for strip in &mut state.strips {
                ui.label(RichText::new(&strip.label).font(theme::mono(theme::CODE)));
                settings::pick(ui, ("route", &strip.label), &mut strip.route, &routes);
                ui.end_row();
            }
        });
}

fn icecast_settings(ui: &mut egui::Ui, ice: &mut Icecast, t: &Tokens) {
    let status = RichText::new("Icecast ○ not connected — preview").font(theme::text(theme::LABEL));
    ui.label(status.color(t.text_muted));
    ui.add_space(space::S);
    let formats: Vec<String> = FORMATS.iter().map(|f| (*f).to_owned()).collect();
    let rates: Vec<String> = BITRATES.iter().map(|b| format!("{b} kbit/s")).collect();
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
            ui.add(DragValue::new(&mut ice.port).range(1..=65_535));
            ui.end_row();
            settings::label(ui, "Mount", t);
            field(ui, &mut ice.mount, false);
            ui.end_row();
            settings::label(ui, "User", t);
            field(ui, &mut ice.user, false);
            ui.end_row();
            settings::label(ui, "Password", t);
            field(ui, &mut ice.password, true);
            ui.end_row();
            settings::label(ui, "Format", t);
            settings::pick(ui, "ice_format", &mut ice.format, &formats);
            ui.end_row();
            settings::label(ui, "Bitrate", t);
            settings::pick(ui, "ice_bitrate", &mut ice.bitrate, &rates);
            ui.end_row();
        });
}
