//! The studio clock: a seconds ring, the time, the date and two countdowns.
//! All sizes come from the window size, because people put the clock on a second screen.

use std::time::Duration;

use egui::{Align2, RichText, Sense};
use jiff::SignedDuration;

use crate::app::Action;
use crate::context_menu::check_item;
use crate::icons::{self, Icon};
use crate::parts;
use crate::preferences::Page;
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{DeckState, StationView};

/// The view state of the clock window. The settings live in the view source.
#[derive(Default)]
pub(crate) struct State {}

/// The clock settings, read from the view source for one frame.
#[derive(Clone, Copy, PartialEq)]
struct Prefs {
    h24: bool,
    show_date: bool,
    show_ring: bool,
    show_next_fixed: bool,
    show_item_end: bool,
}

impl Prefs {
    fn read(view: &StationView) -> Self {
        let get = |key| parts::bool(view, parts::CLOCK, key, true);
        Self {
            h24: get("h24"),
            show_date: get("show_date"),
            show_ring: get("show_ring"),
            show_next_fixed: get("show_next_fixed"),
            show_item_end: get("show_item_end"),
        }
    }

    /// The actions that store each setting that differs from `old`.
    fn write(self, old: Self, actions: &mut Vec<Action>) {
        let pairs = [
            ("h24", self.h24, old.h24),
            ("show_date", self.show_date, old.show_date),
            ("show_ring", self.show_ring, old.show_ring),
            ("show_next_fixed", self.show_next_fixed, old.show_next_fixed),
            ("show_item_end", self.show_item_end, old.show_item_end),
        ];
        for (key, new, was) in pairs {
            if new != was {
                actions.push(parts::set(parts::CLOCK, key, new));
            }
        }
    }
}

/// A countdown line. In its last seconds it grows, turns to the warning colour and gets a warning icon.
fn countdown_line(
    ui: &mut egui::Ui,
    text: String,
    left: Option<Duration>,
    size_pt: f32,
    ink: egui::Color32,
    t: &Tokens,
) {
    let near = left.is_some_and(|d| d.as_secs() < size::CLOCK_URGENT_S);
    let (text, ink, font) = if near {
        (
            icons::lead(Icon::TriangleAlert, &text),
            t.warn,
            size_pt * size::CLOCK_URGENT_GROW,
        )
    } else {
        (text, ink, size_pt)
    };
    ui.label(
        RichText::new(text)
            .font(theme::mono(font))
            .strong()
            .color(ink),
    );
}

/// A span as m:ss, or h:mm:ss from one hour.
fn countdown(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let old = Prefs::read(view);
    let mut p = old;
    body(ui, view, state, &mut p, actions, t);
    p.write(old, actions);
}

/// The Clock page of Preferences.
pub(crate) fn preferences(
    ui: &mut egui::Ui,
    view: &StationView,
    actions: &mut Vec<Action>,
    _t: &Tokens,
) {
    let old = Prefs::read(view);
    let mut p = old;
    ui.checkbox(&mut p.h24, "24-hour time");
    ui.checkbox(&mut p.show_date, "Show the date");
    ui.checkbox(&mut p.show_ring, "Show the seconds ring");
    ui.checkbox(&mut p.show_next_fixed, "Countdown to the next fixed event");
    ui.checkbox(
        &mut p.show_item_end,
        "Countdown to the end of the current item",
    );
    p.write(old, actions);
}

fn body(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    prefs: &mut Prefs,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let _ = state;
    settings::opens_preferences(ui, "Clock", Page::Clock, actions);

    let now = &view.now;
    let avail = ui.available_size();
    let lines = [prefs.show_next_fixed, prefs.show_item_end]
        .iter()
        .filter(|b| **b)
        .count();
    let side = avail
        .x
        .min(avail.y * size::CLOCK_RING_SHARE)
        .max(size::CONTROL_LG);
    let (rect, ring) = ui.allocate_exact_size(egui::vec2(avail.x, side), Sense::click());
    ring.context_menu(|ui| {
        if check_item(ui, prefs.h24, "24-hour time") {
            prefs.h24 = !prefs.h24;
        }
        if check_item(ui, prefs.show_date, "Show date") {
            prefs.show_date = !prefs.show_date;
        }
        if check_item(ui, prefs.show_ring, "Show seconds ring") {
            prefs.show_ring = !prefs.show_ring;
        }
    });
    let c = rect.center();
    let r = side / 2.0 - size::CLOCK_TICK_R * 2.0;
    let p = ui.painter();
    if prefs.show_ring {
        let sec = now.second();
        for i in 0..60i8 {
            let a = std::f32::consts::TAU * f32::from(i) / 60.0 - std::f32::consts::FRAC_PI_2;
            let pos = c + egui::vec2(a.cos(), a.sin()) * r;
            let lit = i <= sec;
            // Unlit dots use the 3:1 control edge, so the ring reads from across the room.
            let colour = if lit { t.on_air } else { t.field_border };
            let big = if i % size::CLOCK_MAJOR_EVERY == 0 {
                size::CLOCK_MAJOR_TICK
            } else {
                1.0
            };
            p.circle_filled(pos, size::CLOCK_TICK_R * big, colour);
        }
    }
    let time = if prefs.h24 {
        now.strftime("%H:%M:%S").to_string()
    } else {
        now.strftime("%-I:%M:%S %p").to_string()
    };
    let digits = side * size::CLOCK_DIGITS_PER_RING;
    p.text(c, Align2::CENTER_CENTER, time, theme::mono(digits), t.text);
    if prefs.show_date {
        let date = now.strftime("%A %-d %B %Y").to_string();
        let below = c + egui::vec2(0.0, digits);
        p.text(
            below,
            Align2::CENTER_CENTER,
            date,
            theme::text(digits * size::CLOCK_DATE_PER_DIGITS),
            t.text_muted,
        );
    }
    if lines == 0 {
        return;
    }
    ui.add_space(space::S);
    let line = (side * size::CLOCK_LINE_PER_RING).max(theme::SECTION);
    let nowt = now.timestamp();
    ui.vertical_centered(|ui| {
        if prefs.show_next_fixed {
            let next = view
                .rows
                .iter()
                .find(|r| r.fixed.is_some() && r.start.timestamp() > nowt);
            let (text, d) = match next {
                Some(r) => {
                    let d = Duration::try_from(r.start.timestamp().duration_since(nowt))
                        .unwrap_or_default();
                    let kind = match r.timing {
                        Some(tm) if !tm.hard => "soft",
                        _ => "hard",
                    };
                    let at = r.start.strftime("%H:%M:%S");
                    let text = icons::lead(
                        Icon::Timer,
                        &format!("{at} {} ({kind}) in {}", r.title, countdown(d)),
                    );
                    (text, Some(d))
                }
                None => ("No fixed item in the next hours".to_owned(), None),
            };
            countdown_line(ui, text, d, line, t.accent_ink, t);
        }
        if prefs.show_item_end {
            let (text, left) = match view.decks.iter().find(|d| d.state == DeckState::Playing) {
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
                    (format!("{kind} ends in {}", countdown(left)), Some(left))
                }
                None => ("Nothing on air".to_owned(), None),
            };
            countdown_line(ui, text, left, line, t.text, t);
        }
    });
}
