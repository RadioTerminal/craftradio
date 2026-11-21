//! The top bar: four deck cards with their buttons, the mode and the station clock.

use egui::{Align, DragValue, Layout, RichText, Sense, Stroke};

use crate::app::{Action, hms, mmss};
use crate::context_menu::{self, menu_item};
use crate::settings;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DeckCommand, DeckState, DeckView, Edit, StationView};
use crate::widgets::icon_button;

/// The fade time of a new deck, in seconds.
const DEFAULT_FADE_S: f32 = 3.0;

/// The deck settings. They are local to the window.
pub(crate) struct State {
    fade_s: [f32; 4],
    settings: [bool; 4],
}

impl Default for State {
    fn default() -> Self {
        Self {
            fade_s: [DEFAULT_FADE_S; 4],
            settings: [false; 4],
        }
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let gaps = space::M * view.decks.len() as f32;
        let free = ui.available_width() - size::CLOCK_W - gaps;
        let card_w = (free / view.decks.len() as f32).max(size::DECK_MIN_W);
        for (i, deck) in view.decks.iter().enumerate() {
            let card = deck_card(ui, i, deck, card_w, state, actions, t);
            card.context_menu(|ui| deck_menu(ui, i, state, actions));
        }
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| clock(ui, view, t));
    });
}

fn command(deck: usize, command: DeckCommand) -> Action {
    Action::Edit(Edit::Deck { deck, command })
}

fn deck_menu(ui: &mut egui::Ui, i: usize, state: &State, actions: &mut Vec<Action>) {
    if menu_item(ui, "Play / Pause", None) {
        actions.push(command(i, DeckCommand::PlayPause));
    }
    if menu_item(ui, "Fade out", None) {
        actions.push(command(i, DeckCommand::Fade(state.fade_s[i])));
    }
    if menu_item(ui, "Skip", None) {
        actions.push(command(i, DeckCommand::Skip));
    }
    ui.separator();
    for label in ["Stop", "Eject", "Reload", "Cue to start"] {
        if menu_item(ui, label, None) {
            actions.push(Action::Toast(context_menu::NEEDS_ENGINE.to_owned()));
        }
    }
}

fn deck_card(
    ui: &mut egui::Ui,
    i: usize,
    deck: &DeckView,
    width: f32,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> egui::Response {
    let live = matches!(deck.state, DeckState::Playing | DeckState::Fading);
    let edge = match deck.state {
        DeckState::Playing => t.on_air,
        DeckState::Fading => t.warn,
        _ => t.card_border,
    };
    let frame = theme::card_frame(t)
        .fill(if live { t.on_air_bg } else { t.field })
        .stroke(Stroke::new(size::HAIRLINE, edge));
    let inner = width - frame.total_margin().sum().x - size::HAIRLINE * 2.0;
    // The badge and the title take the right-click. A card-wide sensor would take the button clicks.
    frame
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.vertical(|ui| deck_body(ui, i, deck, state, actions, t))
                .inner
        })
        .inner
}

fn deck_body(
    ui: &mut egui::Ui,
    i: usize,
    deck: &DeckView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> egui::Response {
    ui.spacing_mut().item_spacing.y = space::XS;
    let live = matches!(deck.state, DeckState::Playing | DeckState::Fading);
    let head = ui.horizontal(|ui| {
        let badge = badge(ui, deck.label, live, t);
        let (word, colour) = match deck.state {
            DeckState::Playing => ("▶ ON AIR", t.on_air),
            DeckState::Fading => ("⤵ FADING", t.warn),
            DeckState::Paused => ("⏸ PAUSED", t.warn),
            DeckState::Ready => ("READY", t.text_muted),
            DeckState::Empty => ("EMPTY", t.text_muted),
        };
        let word = RichText::new(word).font(theme::text(theme::LABEL)).strong();
        ui.label(word.color(colour));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = space::XXS;
            let name = deck.label;
            let mut open = state.settings[i];
            settings::settings_button(ui, &format!("Deck {name}"), &mut open);
            state.settings[i] = open;
            ui.add_enabled_ui(deck.state != DeckState::Empty, |ui| {
                let skip = icon_button(
                    ui,
                    "⏭",
                    false,
                    &format!("⏭ Skip deck {name}"),
                    "Skip: fade out and start the next item",
                );
                if skip.clicked() {
                    actions.push(command(i, DeckCommand::Skip));
                }
                let hint = format!("Fade out over {:.1} s", state.fade_s[i]);
                let fade = icon_button(
                    ui,
                    "⤵",
                    deck.state == DeckState::Fading,
                    &format!("⤵ Fade deck {name}"),
                    &hint,
                );
                if fade.clicked() {
                    actions.push(command(i, DeckCommand::Fade(state.fade_s[i])));
                }
                let (glyph, verb) = if deck.state == DeckState::Playing {
                    ("⏸", "Pause")
                } else {
                    ("▶", "Play")
                };
                let play = icon_button(
                    ui,
                    glyph,
                    false,
                    &format!("{glyph} {verb} deck {name}"),
                    verb,
                );
                if play.clicked() {
                    actions.push(command(i, DeckCommand::PlayPause));
                }
            });
        });
        badge
    });
    if state.settings[i] {
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(size::HAIRLINE, t.accent))
            .corner_radius(t.radius)
            .inner_margin(pad::ROW)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let label = ui.label(RichText::new("Fade time").color(t.text_muted));
                    let fade = DragValue::new(&mut state.fade_s[i])
                        .range(0.5..=10.0)
                        .speed(0.1)
                        .suffix(" s");
                    ui.add(fade).labelled_by(label.id);
                });
                let note = RichText::new(settings::NOT_SAVED).font(theme::text(theme::CAPTION));
                ui.label(note.color(t.text_muted));
            });
    }
    let title = if deck.title.is_empty() {
        "—"
    } else {
        deck.title.as_str()
    };
    let label = egui::Label::new(RichText::new(title).font(theme::text(theme::BODY)));
    let title = ui.add(label.truncate().sense(Sense::click()));
    progress(ui, deck, t);
    let left = deck.len.saturating_sub(deck.elapsed);
    let times = match deck.state {
        DeckState::Playing | DeckState::Fading | DeckState::Paused => {
            format!(
                "{} / {} · −{}",
                mmss(deck.elapsed),
                mmss(deck.len),
                mmss(left)
            )
        }
        DeckState::Ready => format!("ready · {}", mmss(deck.len)),
        DeckState::Empty => "no item".to_owned(),
    };
    ui.label(
        RichText::new(times)
            .font(theme::mono(theme::CODE))
            .color(t.text_muted),
    );
    head.inner.union(title)
}

fn badge(ui: &mut egui::Ui, label: char, playing: bool, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(size::BADGE), Sense::click());
    let (fill, ink) = if playing {
        (t.on_air, t.canvas)
    } else {
        (t.hover, t.text)
    };
    ui.painter().rect_filled(rect, t.radius, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        theme::mono(theme::CARD),
        ink,
    );
    resp
}

fn progress(ui: &mut egui::Ui, deck: &DeckView, t: &Tokens) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, size::PROGRESS_H), Sense::hover());
    ui.painter().rect_filled(rect, t.radius_sm, t.canvas);
    let len = deck.len.as_secs_f32();
    if len > 0.0 {
        let frac = (deck.elapsed.as_secs_f32() / len).clamp(0.0, 1.0);
        let mut done = rect;
        done.set_width(rect.width() * frac);
        let colour = match deck.state {
            DeckState::Playing => t.on_air,
            DeckState::Fading | DeckState::Paused => t.warn,
            _ => t.field_border,
        };
        ui.painter().rect_filled(done, t.radius_sm, colour);
    }
}

fn clock(ui: &mut egui::Ui, view: &StationView, t: &Tokens) {
    ui.vertical(|ui| {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(t.selection)
                    .stroke(Stroke::new(size::HAIRLINE, t.accent))
                    .corner_radius(t.radius_lg)
                    .inner_margin(pad::CHIP)
                    .show(ui, |ui| {
                        let mode = RichText::new(&view.mode).font(theme::mono(theme::LABEL));
                        ui.label(mode.strong().color(t.accent_ink));
                    });
                ui.label(
                    RichText::new(hms(&view.now))
                        .font(theme::mono(theme::TITLE))
                        .strong(),
                );
            });
            let date = view.now.strftime("%a %-d %b %Y").to_string();
            ui.label(
                RichText::new(date)
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        });
    });
}
