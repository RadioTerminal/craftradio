//! The studio clock: a seconds ring, the time, the date and two countdowns.
//! All sizes come from the window size, because people put the clock on a second screen.

use std::time::Duration;

use egui::{Align2, RichText, Sense};
use jiff::SignedDuration;

use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{DeckState, StationView};

/// The clock settings. They are local to the window.
pub(crate) struct State {
    settings: bool,
    h24: bool,
    show_date: bool,
    show_ring: bool,
    show_next_fixed: bool,
    show_item_end: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            settings: false,
            h24: true,
            show_date: true,
            show_ring: true,
            show_next_fixed: true,
            show_item_end: true,
        }
    }
}

/// A span as m:ss, or h:mm:ss from one hour.
fn countdown(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub(crate) fn show(ui: &mut egui::Ui, view: &StationView, state: &mut State, t: &Tokens) {
    let mut open = state.settings;
    settings::header(ui, "Clock", &mut open, |_| {});
    state.settings = open;
    if state.settings {
        settings::section(ui, "Clock", t, |ui| {
            ui.checkbox(&mut state.h24, "24-hour time");
            ui.checkbox(&mut state.show_date, "Show the date");
            ui.checkbox(&mut state.show_ring, "Show the seconds ring");
            ui.checkbox(
                &mut state.show_next_fixed,
                "Countdown to the next fixed event",
            );
            ui.checkbox(
                &mut state.show_item_end,
                "Countdown to the end of the current item",
            );
        });
    }

    let now = &view.now;
    let avail = ui.available_size();
    let lines = [state.show_next_fixed, state.show_item_end]
        .iter()
        .filter(|b| **b)
        .count();
    let side = avail
        .x
        .min(avail.y * size::CLOCK_RING_SHARE)
        .max(size::CONTROL_LG);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(avail.x, side), Sense::hover());
    let c = rect.center();
    let r = side / 2.0 - size::CLOCK_TICK_R * 2.0;
    let p = ui.painter();
    if state.show_ring {
        let sec = now.second();
        for i in 0..60i8 {
            let a = std::f32::consts::TAU * f32::from(i) / 60.0 - std::f32::consts::FRAC_PI_2;
            let pos = c + egui::vec2(a.cos(), a.sin()) * r;
            let lit = i <= sec;
            let colour = if lit { t.on_air } else { t.field };
            let big = if i % 5 == 0 { 1.5 } else { 1.0 };
            p.circle_filled(pos, size::CLOCK_TICK_R * big, colour);
        }
    }
    let time = if state.h24 {
        now.strftime("%H:%M:%S").to_string()
    } else {
        now.strftime("%-I:%M:%S %p").to_string()
    };
    let digits = side * size::CLOCK_DIGITS_PER_RING;
    p.text(c, Align2::CENTER_CENTER, time, theme::mono(digits), t.text);
    if state.show_date {
        let date = now.strftime("%A %-d %B %Y").to_string();
        let below = c + egui::vec2(0.0, digits);
        p.text(
            below,
            Align2::CENTER_CENTER,
            date,
            theme::text(digits * 0.3),
            t.text_muted,
        );
    }
    if lines == 0 {
        return;
    }
    ui.add_space(space::S);
    let line = (side * size::CLOCK_LINE_PER_RING).max(theme::BODY);
    let nowt = now.timestamp();
    ui.vertical_centered(|ui| {
        if state.show_next_fixed {
            let next = view
                .rows
                .iter()
                .find(|r| r.fixed.is_some() && r.start.timestamp() > nowt);
            let text = match next {
                Some(r) => {
                    let d = Duration::try_from(r.start.timestamp().duration_since(nowt))
                        .unwrap_or_default();
                    format!("{} in {}", r.title, countdown(d))
                }
                None => "No fixed event in the next hours".to_owned(),
            };
            ui.label(
                RichText::new(text)
                    .font(theme::mono(line))
                    .color(t.accent_ink),
            );
        }
        if state.show_item_end {
            let text = match view.decks.iter().find(|d| d.state == DeckState::Playing) {
                Some(d) => {
                    let left = d.len.saturating_sub(d.elapsed);
                    let kind = view
                        .rows
                        .iter()
                        .find(|r| {
                            let end = SignedDuration::try_from(r.len)
                                .map_or(r.start.timestamp(), |s| r.start.timestamp() + s);
                            r.start.timestamp() <= nowt && nowt < end
                        })
                        .map_or("Item", |r| if r.is_main { "Song" } else { "Item" });
                    format!("{kind} ends in {}", countdown(left))
                }
                None => "Nothing on air".to_owned(),
            };
            ui.label(RichText::new(text).font(theme::mono(line)).color(t.text));
        }
    });
}
