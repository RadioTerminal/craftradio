//! The top bar: four deck cards with their buttons, the mode and the station clock.

use egui::{Align, DragValue, Layout, RichText, Sense, Stroke};

use crate::app::{Action, hms, mmss};
use crate::context_menu::{self, menu_item};
use crate::parts;
use crate::settings;
use crate::shortcuts;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DeckCommand, DeckState, DeckView, Edit, StationView};
use crate::widgets::{guard_button, icon_button};

/// The part setting that switches the second press on the deck on air.
const CONFIRM_KEY: &str = "confirm_on_air";
/// While a button is armed, the strip draws again at this interval, in seconds.
const ARM_REPAINT_S: f32 = 0.1;
/// The fade time of a new deck, in seconds.
const DEFAULT_FADE_S: f32 = 3.0;

/// An action that needs a second press on the deck on air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Guarded {
    Fade,
    Skip,
}

/// The deck view state. The fade times and the confirm switch come from the view source.
pub(crate) struct State {
    fade_s: [f32; 4],
    settings: [bool; 4],
    /// True when Skip and Fade on the deck on air need a second press.
    confirm: bool,
    /// The armed button: deck, action and the egui time of the first press.
    armed: Option<(usize, Guarded, f64)>,
    /// The deck that Space, Skip and Fade keys act on.
    focused: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            fade_s: [DEFAULT_FADE_S; 4],
            settings: [false; 4],
            confirm: true,
            armed: None,
            focused: 0,
        }
    }
}

impl State {
    /// A press of Skip or Fade. It returns the action when the press fires.
    /// On the deck on air, with confirm on, the first press arms and a second press fires.
    pub(crate) fn press(
        &mut self,
        deck: usize,
        kind: Guarded,
        live: bool,
        now: f64,
    ) -> Option<Action> {
        if live && self.confirm {
            let again = self
                .armed
                .is_some_and(|(d, k, at)| d == deck && k == kind && now - at <= size::ARM_S);
            if !again {
                self.armed = Some((deck, kind, now));
                return None;
            }
        }
        self.armed = None;
        Some(match kind {
            Guarded::Fade => command(deck, DeckCommand::Fade(self.fade_s[deck])),
            Guarded::Skip => command(deck, DeckCommand::Skip),
        })
    }

    /// Focus deck `deck` for the keys. A number past the last deck is ignored.
    pub(crate) fn focus(&mut self, deck: usize) {
        if deck < self.fade_s.len() {
            self.focused = deck;
        }
    }

    pub(crate) fn focused(&self) -> usize {
        self.focused
    }

    /// The part of the arm time that is left for this button, while it is armed.
    fn armed_left(&self, deck: usize, kind: Guarded, now: f64) -> Option<f32> {
        let (d, k, at) = self.armed?;
        let left = 1.0 - (now - at) / size::ARM_S;
        (d == deck && k == kind && left > 0.0).then_some(left as f32)
    }
}

/// True when a deck is on air: playing, fading or paused in the middle of the item.
pub(crate) fn is_live(deck: &DeckView) -> bool {
    matches!(
        deck.state,
        DeckState::Playing | DeckState::Fading | DeckState::Paused
    )
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let labels = ['A', 'B', 'C', 'D'];
    for (i, label) in labels.iter().enumerate() {
        state.fade_s[i] = parts::f32(view, parts::DECKS, &fade_key(*label), DEFAULT_FADE_S);
    }
    let old = state.fade_s;
    state.confirm = parts::bool(view, parts::DECKS, CONFIRM_KEY, true);
    let old_confirm = state.confirm;
    let now = ui.input(|i| i.time);
    if state.armed.is_some_and(|(_, _, at)| now - at > size::ARM_S) {
        state.armed = None;
    }
    if state.armed.is_some() {
        // The ring counts down, so the deck strip draws again soon.
        ui.ctx().request_repaint_after_secs(ARM_REPAINT_S);
    }
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let gaps = space::M * view.decks.len() as f32;
        let free = ui.available_width() - size::CLOCK_W - gaps;
        let card_w = (free / view.decks.len() as f32).max(size::DECK_MIN_W);
        for (i, deck) in view.decks.iter().enumerate() {
            let card = deck_card(ui, i, deck, card_w, state, actions, t);
            card.context_menu(|ui| deck_menu(ui, i, deck, state, now, actions));
        }
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| clock(ui, view, t));
    });
    if state.confirm != old_confirm {
        actions.push(parts::set(parts::DECKS, CONFIRM_KEY, state.confirm));
    }
    for (i, label) in labels.iter().enumerate() {
        if state.fade_s[i] != old[i] {
            let value = f64::from(state.fade_s[i]);
            actions.push(parts::set(parts::DECKS, &fade_key(*label), value));
        }
    }
}

fn fade_key(deck: char) -> String {
    format!("{deck}_fade_s")
}

fn command(deck: usize, command: DeckCommand) -> Action {
    Action::Edit(Edit::Deck { deck, command })
}

fn deck_menu(
    ui: &mut egui::Ui,
    i: usize,
    deck: &DeckView,
    state: &mut State,
    now: f64,
    actions: &mut Vec<Action>,
) {
    if menu_item(ui, "Play / Pause", None) {
        actions.push(command(i, DeckCommand::PlayPause));
    }
    // The menu goes through the same guard as the buttons: on air, it arms the button.
    let live = is_live(deck);
    for (label, kind, keys) in [
        ("Fade out", Guarded::Fade, shortcuts::FADE),
        ("Skip", Guarded::Skip, shortcuts::SKIP),
    ] {
        if menu_item(ui, label, Some(keys))
            && let Some(a) = state.press(i, kind, live, now)
        {
            actions.push(a);
        }
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
    // The deck on air has a thick edge and the ON AIR word, so the colour is not the only sign.
    let (edge, edge_w) = match deck.state {
        DeckState::Playing => (t.on_air, size::ON_AIR_EDGE),
        DeckState::Fading => (t.warn, size::ON_AIR_EDGE),
        _ => (t.card_border, size::HAIRLINE),
    };
    let frame = theme::card_frame(t)
        .fill(if live { t.on_air_bg } else { t.field })
        .stroke(Stroke::new(edge_w, edge));
    let inner = width - frame.total_margin().sum().x - edge_w * 2.0;
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
    let live = is_live(deck);
    let now = ui.input(|i| i.time);
    let head = ui.horizontal(|ui| {
        let focused = state.focused == i;
        let keys = shortcuts::label(ui.ctx(), &shortcuts::FOCUS_DECK[i.min(3)]);
        let hint = if focused {
            "The keys act on this deck. Right-click for the deck menu.".to_owned()
        } else {
            format!("{keys} focuses this deck for the keys. Right-click for the deck menu.")
        };
        let badge = badge(ui, deck.label, live, focused, t).on_hover_text(hint);
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
            // A gap keeps the settings button away from the transport buttons.
            ui.add_space(space::M);
            ui.add_enabled_ui(deck.state != DeckState::Empty, |ui| {
                let keys = shortcuts::label(ui.ctx(), &shortcuts::SKIP);
                let hint = format!("Skip: fade out and start the next item ({keys})");
                let armed = state.armed_left(i, Guarded::Skip, now);
                let skip = guard_button(ui, "⏭", &format!("⏭ Skip deck {name}"), &hint, armed);
                if skip.clicked()
                    && let Some(a) = state.press(i, Guarded::Skip, live, now)
                {
                    actions.push(a);
                }
                let keys = shortcuts::label(ui.ctx(), &shortcuts::FADE);
                let hint = format!("Fade out over {:.1} s ({keys})", state.fade_s[i]);
                let armed = state.armed_left(i, Guarded::Fade, now);
                let fade = guard_button(ui, "⤵", &format!("⤵ Fade deck {name}"), &hint, armed);
                if fade.clicked()
                    && let Some(a) = state.press(i, Guarded::Fade, live, now)
                {
                    actions.push(a);
                }
                let (glyph, verb) = if deck.state == DeckState::Playing {
                    ("⏸", "Pause")
                } else {
                    ("▶", "Play")
                };
                let keys = shortcuts::label(ui.ctx(), &shortcuts::PLAY_PAUSE);
                let play = icon_button(
                    ui,
                    glyph,
                    false,
                    &format!("{glyph} {verb} deck {name}"),
                    &format!("{verb} ({keys} on the focused deck)"),
                );
                if play.clicked() {
                    actions.push(command(i, DeckCommand::PlayPause));
                }
            });
        });
        badge
    });
    if let Some((d, kind, _)) = state.armed
        && d == i
    {
        let what = match kind {
            Guarded::Fade => "Fade",
            Guarded::Skip => "Skip",
        };
        let note = format!("{what} is on air: press again to confirm");
        let note = RichText::new(note)
            .font(theme::text(theme::CAPTION))
            .strong();
        ui.label(note.color(t.danger));
    }
    if state.settings[i] {
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(size::HAIRLINE, t.accent))
            .corner_radius(t.radius)
            .inner_margin(pad::ROW)
            .show(ui, |ui| {
                ui.checkbox(
                    &mut state.confirm,
                    "Confirm on-air actions (press Skip or Fade twice)",
                );
                ui.horizontal(|ui| {
                    let label = ui.label(RichText::new("Fade time").color(t.text_muted));
                    let fade = DragValue::new(&mut state.fade_s[i])
                        .range(size::FADE_MIN_S..=size::FADE_MAX_S)
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

fn badge(
    ui: &mut egui::Ui,
    label: char,
    playing: bool,
    focused: bool,
    t: &Tokens,
) -> egui::Response {
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
    if focused {
        // A ring outside the badge marks the deck the keys act on.
        let ring = Stroke::new(size::ON_AIR_EDGE, t.accent);
        ui.painter()
            .rect_stroke(rect, t.radius, ring, egui::StrokeKind::Outside);
    }
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
