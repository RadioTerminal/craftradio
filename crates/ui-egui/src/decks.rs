//! The top bar: four deck cards with their buttons, the mode and the station clock.

use egui::{Align, DragValue, Layout, RichText, Sense, Stroke};

use crate::app::{Action, hms, mmss};
use crate::context_menu::{self, menu_item};
use crate::mixer;
use crate::parts;
use crate::settings;
use crate::shortcuts;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DeckCommand, DeckState, DeckView, Edit, PlayMode, StationView, Status};
use crate::widgets::{self, guard_button, icon_button};

/// The part setting that switches the second press on the deck on air.
const CONFIRM_KEY: &str = "confirm_on_air";
/// While a button is armed, the strip draws again at this interval, in seconds.
const ARM_REPAINT_S: f32 = 0.1;
/// The fade-out time of a new deck, in seconds.
const DEFAULT_FADE_S: f32 = 3.0;
/// The fade-in of a new deck when it plays before its turn, in seconds.
const DEFAULT_FADE_IN_S: f32 = 0.5;

/// An action that needs a second press when it cuts the item on air before its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Guarded {
    /// Fade out the deck on air.
    Fade,
    /// Play this deck now: a crossfade from the item on air.
    Play,
    /// Play the NEXT 1 deck now, from the header. The deck of the guard is that deck.
    PlayNext,
}

/// The deck view state. The fade times and the confirm switch come from the view source.
pub(crate) struct State {
    fade_s: [f32; 4],
    fade_in_s: [f32; 4],
    settings: [bool; 4],
    /// True when an action that cuts the item on air needs a second press.
    confirm: bool,
    /// The armed button: deck, action and the egui time of the first press.
    armed: Option<(usize, Guarded, f64)>,
    /// The deck that Space acts on.
    focused: usize,
    /// The place of each deck in the queue, for this frame: 1 plays next.
    next: Vec<Option<usize>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            fade_s: [DEFAULT_FADE_S; 4],
            fade_in_s: [DEFAULT_FADE_IN_S; 4],
            settings: [false; 4],
            confirm: true,
            armed: None,
            focused: 0,
            next: Vec::new(),
        }
    }
}

impl State {
    /// A press of a guarded action. It returns the action when the press fires. With confirm
    /// on, a press that `cuts` the item on air arms, and a second press fires.
    pub(crate) fn press(
        &mut self,
        deck: usize,
        kind: Guarded,
        cuts: bool,
        now: f64,
    ) -> Option<Action> {
        if cuts && self.confirm {
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
            Guarded::Play => command(deck, DeckCommand::Play(self.fade_in_s[deck])),
            Guarded::PlayNext => Action::Edit(Edit::PlayNext {
                fade_in_s: self.fade_in_s[deck],
            }),
        })
    }

    /// The NEXT 1 deck of this frame, if a deck waits.
    pub(crate) fn next_deck(&self) -> Option<usize> {
        self.next.iter().position(|r| *r == Some(1))
    }

    /// The fade-in of deck `deck`, in seconds.
    pub(crate) fn fade_in(&self, deck: usize) -> f32 {
        self.fade_in_s
            .get(deck)
            .copied()
            .unwrap_or(DEFAULT_FADE_IN_S)
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

/// True when a start of deck `deck` cuts an item on air before its end. The fade lasts
/// `fade_s` seconds. An item that ends inside the fade, or fades out already, is not cut.
pub(crate) fn cuts_on_air(decks: &[DeckView], deck: usize, fade_s: f32) -> bool {
    decks.iter().enumerate().any(|(k, d)| {
        let left = d.len.saturating_sub(d.elapsed).as_secs_f32();
        k != deck && matches!(d.state, DeckState::Playing | DeckState::Paused) && left > fade_s
    })
}

/// The deck on air that Fade out acts on: the deck that plays, else the paused deck.
pub(crate) fn on_air_deck(decks: &[DeckView]) -> Option<usize> {
    decks
        .iter()
        .position(|d| d.state == DeckState::Playing)
        .or_else(|| decks.iter().position(|d| d.state == DeckState::Paused))
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    // The countdowns of the decks count from the snapshot instant.
    let now_ts = view.now.timestamp();
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(NOW_KEY), now_ts));
    let labels = ['A', 'B', 'C', 'D'];
    for (i, label) in labels.iter().enumerate() {
        state.fade_s[i] = parts::f32(view, parts::DECKS, &fade_key(*label), DEFAULT_FADE_S);
        let key = fade_in_key(*label);
        state.fade_in_s[i] = parts::f32(view, parts::DECKS, &key, DEFAULT_FADE_IN_S);
    }
    let old = state.fade_s;
    let old_in = state.fade_in_s;
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
    state.next = next_ranks(&view.decks);
    let wide = ui.available_width() >= size::STRIP_WIDE;
    let clock_width = if wide {
        size::CLOCK_W
    } else {
        size::CLOCK_NARROW_W
    };
    let deck_min = if wide {
        size::DECK_MIN_W
    } else {
        size::DECK_TINY_W
    };
    egui::ScrollArea::horizontal()
        .id_salt("deck_strip_scroll")
        .show(ui, |ui| {
            ui.set_min_width((deck_min + space::M) * view.decks.len() as f32 + clock_width);
            ui.spacing_mut().item_spacing.x = space::M;
            egui_extras::StripBuilder::new(ui)
                .size(egui_extras::Size::exact(size::DECK_STRIP_H))
                .vertical(|mut strip| {
                    strip.strip(|builder| {
                        builder
                            .sizes(
                                egui_extras::Size::remainder().at_least(deck_min),
                                view.decks.len(),
                            )
                            .size(egui_extras::Size::exact(clock_width))
                            .horizontal(|mut strip| {
                                for (i, deck) in view.decks.iter().enumerate() {
                                    strip.cell(|ui| {
                                        let card = deck_card(
                                            ui,
                                            i,
                                            &view.decks,
                                            ui.available_width(),
                                            state,
                                            actions,
                                            t,
                                        );
                                        card.context_menu(|ui| {
                                            deck_menu(ui, i, deck, view, state, now, actions)
                                        });
                                    });
                                }
                                strip.cell(|ui| {
                                    clock(ui, view, state, actions, t);
                                });
                            });
                    });
                });
        });
    if state.confirm != old_confirm {
        actions.push(parts::set(parts::DECKS, CONFIRM_KEY, state.confirm));
    }
    for (i, label) in labels.iter().enumerate() {
        if state.fade_s[i] != old[i] {
            let value = f64::from(state.fade_s[i]);
            actions.push(parts::set(parts::DECKS, &fade_key(*label), value));
        }
        if state.fade_in_s[i] != old_in[i] {
            let value = f64::from(state.fade_in_s[i]);
            actions.push(parts::set(parts::DECKS, &fade_in_key(*label), value));
        }
    }
}

/// The place of each ready deck in the queue, by its planned start: 1 plays next.
fn next_ranks(decks: &[DeckView]) -> Vec<Option<usize>> {
    let mut ready: Vec<(usize, jiff::Timestamp)> = decks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.state == DeckState::Ready)
        .filter_map(|(i, d)| d.start.as_ref().map(|s| (i, s.timestamp())))
        .collect();
    ready.sort_by_key(|(_, at)| *at);
    let mut ranks = vec![None; decks.len()];
    for (rank, (i, _)) in ready.iter().enumerate() {
        if let Some(r) = ranks.get_mut(*i) {
            *r = Some(rank + 1);
        }
    }
    ranks
}

fn fade_key(deck: char) -> String {
    format!("{deck}_fade_s")
}

fn fade_in_key(deck: char) -> String {
    format!("{deck}_fade_in_s")
}

fn command(deck: usize, command: DeckCommand) -> Action {
    Action::Edit(Edit::Deck { deck, command })
}

fn deck_menu(
    ui: &mut egui::Ui,
    i: usize,
    deck: &DeckView,
    view: &StationView,
    state: &mut State,
    now: f64,
    actions: &mut Vec<Action>,
) {
    // The menu goes through the same guard as the buttons: a cut of the item on air arms.
    let (_, _, label) = primary(deck);
    if menu_item(ui, label, Some(shortcuts::PLAY_PAUSE)) {
        if is_live(deck) {
            actions.push(command(i, DeckCommand::PlayPause));
        } else if let Some(a) = state.press(i, Guarded::Play, cuts(view, i, state), now) {
            actions.push(a);
        }
    }
    let live = is_live(deck);
    if menu_item(ui, "Fade out", None)
        && let Some(a) = state.press(i, Guarded::Fade, live, now)
    {
        actions.push(a);
    }
    ui.separator();
    for label in ["Stop", "Eject", "Reload", "Cue to start"] {
        if menu_item(ui, label, None) {
            actions.push(Action::Toast(context_menu::not_here(view)));
        }
    }
}

/// The one main button of a deck: Pause on air, Resume after a pause, or Play this deck. It
/// gives the verb, the glyph and the words of the deck menu.
fn primary(deck: &DeckView) -> (&'static str, &'static str, &'static str) {
    match deck.state {
        DeckState::Playing => ("Pause", "⏸", "Pause"),
        DeckState::Paused => ("Resume", "▶", "Resume"),
        _ => ("Play", "▶", "Play this deck"),
    }
}

/// True when playing deck `i` now cuts the item on air before its end.
fn cuts(view: &StationView, i: usize, state: &State) -> bool {
    cuts_on_air(&view.decks, i, state.fade_in(i))
}

fn deck_card(
    ui: &mut egui::Ui,
    i: usize,
    decks: &[DeckView],
    width: f32,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> egui::Response {
    let deck = &decks[i];
    let live = matches!(deck.state, DeckState::Playing | DeckState::Fading);
    // The edge has the colour of the deck. The deck on air has a thick edge, its fill and
    // the ON AIR word, so the colour is not the only sign.
    let edge = Tokens::deck(i);
    let edge_w = match deck.state {
        DeckState::Playing | DeckState::Fading => size::ON_AIR_EDGE,
        _ => size::HAIRLINE,
    };
    let frame = theme::card_frame(t)
        .fill(if live { t.on_air_bg } else { t.field })
        .stroke(Stroke::new(edge_w, edge));
    let inner = width - frame.total_margin().sum().x - edge_w * 2.0;
    // The badge and the title take the right-click. A card-wide sensor would take the button clicks.
    frame
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.vertical(|ui| deck_body(ui, i, decks, state, actions, t))
                .inner
        })
        .inner
}

fn deck_body(
    ui: &mut egui::Ui,
    i: usize,
    decks: &[DeckView],
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> egui::Response {
    ui.spacing_mut().item_spacing.y = space::XS;
    let deck = &decks[i];
    let now = ui.input(|i| i.time);
    let head = ui.horizontal(|ui| {
        let focused = state.focused == i;
        let keys = shortcuts::label(ui.ctx(), &shortcuts::FOCUS_DECK[i.min(3)]);
        let hint = if focused {
            "Space acts on this deck. Right-click for the deck menu.".to_owned()
        } else {
            format!("{keys} focuses this deck for Space. Right-click for the deck menu.")
        };
        let badge = badge(ui, i, deck.label, focused, t).on_hover_text(hint);
        let (word, colour) = match deck.state {
            DeckState::Playing => ("ON AIR", t.on_air),
            DeckState::Fading => ("FADING", t.warn),
            DeckState::Paused => ("PAUSED", t.warn),
            DeckState::Ready => ("READY", t.text_muted),
            DeckState::Empty => ("EMPTY", t.text_muted),
        };
        let rank = state.next.get(i).copied().flatten();
        let queued;
        let (word, colour) = match rank {
            Some(n) => {
                queued = format!("NEXT {n}");
                let colour = if n == 1 { t.accent_ink } else { t.text_muted };
                (queued.as_str(), colour)
            }
            None => (word, colour),
        };
        let word = RichText::new(word)
            .font(theme::text(theme::LABEL))
            .strong()
            .color(colour);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = space::XXS;
            let name = deck.label;
            let mut open = state.settings[i];
            settings::settings_button(ui, &format!("Deck {name}"), &mut open);
            state.settings[i] = open;
            // A gap keeps the settings button away from the transport buttons. A narrow card
            // keeps a smaller gap.
            let narrow = ui.available_width() < size::DECK_MIN_W;
            ui.add_space(if narrow { space::XS } else { space::M });
            ui.add_enabled_ui(deck.state != DeckState::Empty, |ui| {
                // One main action: Play this deck, or Pause and Resume on the deck on air.
                let keys = shortcuts::label(ui.ctx(), &shortcuts::PLAY_PAUSE);
                let (verb, glyph, _) = primary(deck);
                let accessible = format!("{glyph} {verb} deck {name}");
                if is_live(deck) {
                    let hint = format!("{verb} the item on air ({keys} on the focused deck)");
                    if icon_button(ui, glyph, false, &accessible, &hint).clicked() {
                        actions.push(command(i, DeckCommand::PlayPause));
                    }
                } else {
                    let hint = format!(
                        "Play this deck now: it fades in over {:.1} s and the item on air fades \
                         out. The plan goes on from this deck ({keys} on the focused deck)",
                        state.fade_in(i)
                    );
                    let armed = state.armed_left(i, Guarded::Play, now);
                    let play = guard_button(ui, glyph, &accessible, &hint, armed);
                    let cut = cuts_on_air(decks, i, state.fade_in(i));
                    if play.clicked()
                        && let Some(a) = state.press(i, Guarded::Play, cut, now)
                    {
                        actions.push(a);
                    }
                }
            });
            // The word takes the room that the buttons leave, and truncates there.
            // With no room for a readable word, the card edge and the badge carry the state.
            if ui.available_width() >= size::DECK_WORD_MIN_W {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    widgets::truncated_label(ui, word);
                });
            }
        });
        badge
    });
    if let Some((d, kind, _)) = state.armed
        && d == i
    {
        let what = match kind {
            Guarded::Fade => "Fade out cuts the item on air",
            Guarded::Play => "Play cuts the item on air",
            Guarded::PlayNext => "Play next cuts the item on air",
        };
        let note = format!("{what}: press again to confirm");
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
                    "Confirm a cut of the item on air (press Play or Fade out twice)",
                );
                ui.horizontal(|ui| {
                    let label = ui.label(RichText::new("Fade-in").color(t.text_muted));
                    let fade = DragValue::new(&mut state.fade_in_s[i])
                        .range(size::FADE_IN_MIN_S..=size::FADE_MAX_S)
                        .speed(0.1)
                        .suffix(" s");
                    ui.add(fade)
                        .labelled_by(label.id)
                        .on_hover_text("Play this deck and Play next fade this deck in so long");
                });
                ui.horizontal(|ui| {
                    let label = ui.label(RichText::new("Fade-out").color(t.text_muted));
                    let fade = DragValue::new(&mut state.fade_s[i])
                        .range(size::FADE_MIN_S..=size::FADE_MAX_S)
                        .speed(0.1)
                        .suffix(" s");
                    ui.add(fade).labelled_by(label.id).on_hover_text(
                        "Stored for this deck. Fade out in the header now uses the fade-out \
                             of the item, or 1 s",
                    );
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
    // The playlist of the item, as a chip: its colour and its name.
    if deck.state == DeckState::Empty {
        let empty = RichText::new("Empty: nothing planned").font(theme::text(theme::CAPTION));
        widgets::truncated_label(ui, empty.color(t.text_muted));
    } else if !deck.playlist.is_empty() {
        ui.horizontal(|ui| {
            let (dot, _) =
                ui.allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
            ui.painter().circle_filled(
                dot.center(),
                size::DOT_R,
                Tokens::playlist(deck.playlist_color),
            );
            let ink = t.readable(deck.playlist_color, t.field);
            let name = RichText::new(&deck.playlist).font(theme::text(theme::CAPTION));
            widgets::truncated_label(ui, name.color(ink));
        });
    }
    progress(ui, deck, t);
    let left = deck.len.saturating_sub(deck.elapsed);
    let muted = |s: String| {
        RichText::new(s)
            .font(theme::mono(theme::CODE))
            .color(t.text_muted)
    };
    match deck.state {
        DeckState::Playing | DeckState::Fading | DeckState::Paused => {
            // The time left is the largest number on the card.
            ui.horizontal(|ui| {
                let left = RichText::new(format!("−{}", mmss(left)))
                    .font(theme::mono(theme::SECTION))
                    .strong();
                ui.label(left).on_hover_text("Time left on this item");
                let done = format!("{} / {}", mmss(deck.elapsed), mmss(deck.len));
                widgets::truncated_label(ui, muted(done));
            });
        }
        DeckState::Ready | DeckState::Empty => {}
    }
    // When the deck starts, or when it started and ends. The times come from the plan.
    if let Some(line) = start_line(deck, now_ts(deck, ui)) {
        widgets::truncated_label(ui, muted(line));
    }
    head.inner.union(title)
}

fn badge(ui: &mut egui::Ui, i: usize, label: char, focused: bool, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(size::BADGE), Sense::click());
    // The badge has the identity colour of the deck, the same as its rows in the lists.
    let (fill, ink) = (Tokens::deck(i), t.deck_ink(i));
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

fn clock(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    ui.vertical(|ui| {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.y = space::XXS;
            let w = ui.available_width();
            let row = |h: f32| egui::vec2(w, h);
            let rtl = Layout::right_to_left(Align::Center);
            // Left to right: Play next, Fade out, the mode, the LIVE chip and the Out meter.
            // A narrow cluster has no room for the transport here: it goes to the third row.
            let roomy = w >= size::CLOCK_MID_W;
            ui.allocate_ui_with_layout(row(size::HEADER_ROW), rtl, |ui| {
                ui.spacing_mut().button_padding = egui::vec2(space::XS, space::XXS);
                out_vu(ui, view, actions, t);
                live_chip(ui, view, w >= size::CLOCK_MID_W, actions, t);
                mode_control(ui, view, w >= size::CLOCK_W, actions, t);
                if roomy {
                    transport(ui, view, state, actions);
                }
            });
            // A narrow window drops the date first, then the word of the LIVE chip.
            let room = w >= size::CLOCK_W;
            ui.allocate_ui_with_layout(row(theme::TITLE + space::S), rtl, |ui| {
                ui.label(
                    RichText::new(hms(&view.now))
                        .font(theme::mono(theme::TITLE))
                        .strong(),
                );
                if room {
                    let date = view.now.strftime("%a %-d %b").to_string();
                    ui.label(
                        RichText::new(date)
                            .font(theme::text(theme::CAPTION))
                            .color(t.text_muted),
                    );
                }
            });
            let wide = w >= size::CLOCK_W;
            ui.allocate_ui_with_layout(row(size::HEADER_ROW), rtl, |ui| {
                stop_after(ui, view, actions, t);
                if !roomy {
                    transport(ui, view, state, actions);
                }
                if view.warnings > 0 {
                    // Warnings show here too, where the eyes are.
                    let chip = RichText::new(format!("⚠ {}", view.warnings))
                        .font(theme::text(theme::LABEL))
                        .color(t.warn);
                    ui.label(chip)
                        .on_hover_text(format!("{} warnings; see the status line", view.warnings));
                }
            });
            // The countdown has a row of its own, so the warnings never cut it.
            ui.allocate_ui_with_layout(row(size::HEADER_ROW), rtl, |ui| {
                countdown(ui, view, wide, t);
            });
        });
    });
}

/// The transport of the station: Play next and Fade out. Each arms first when it cuts the
/// item on air before its end.
fn transport(ui: &mut egui::Ui, view: &StationView, state: &mut State, actions: &mut Vec<Action>) {
    let now = ui.input(|i| i.time);
    let air = on_air_deck(&view.decks);
    // The parent lays out right to left, so Fade out comes first.
    ui.add_enabled_ui(air.is_some(), |ui| {
        let deck = air.unwrap_or(0);
        let keys = shortcuts::label(ui.ctx(), &shortcuts::FADE);
        let hint = format!("Fade out the item on air ({keys})");
        let armed = air.and_then(|d| state.armed_left(d, Guarded::Fade, now));
        let fade = guard_button(ui, "⤵", "⤵ Fade out", &hint, armed);
        if fade.clicked()
            && let Some(a) = state.press(deck, Guarded::Fade, true, now)
        {
            actions.push(a);
        }
    });
    let next = state.next_deck();
    ui.add_enabled_ui(next.is_some(), |ui| {
        let deck = next.unwrap_or(0);
        let keys = shortcuts::label(ui.ctx(), &shortcuts::PLAY_NEXT);
        let assist = view.play_mode == PlayMode::Assist;
        let what = if assist {
            "Next: start"
        } else {
            "Play next: start"
        };
        let hint = format!(
            "{what} the NEXT 1 deck now. It fades in and the item on air fades out ({keys})"
        );
        let armed = next.and_then(|d| state.armed_left(d, Guarded::PlayNext, now));
        let play = guard_button(ui, "⏭", "⏭ Play next", &hint, armed);
        let cut = cuts(view, deck, state);
        if play.clicked()
            && let Some(a) = state.press(deck, Guarded::PlayNext, cut, now)
        {
            actions.push(a);
        }
    });
}

/// "Next: News in 4:12", to the next fixed item. Under a minute it warns, with ⚠ and the colour.
fn countdown(ui: &mut egui::Ui, view: &StationView, wide: bool, t: &Tokens) {
    let now = view.now.timestamp();
    let next = view
        .rows
        .iter()
        .find(|r| r.fixed.is_some() && r.start.timestamp() > now);
    let Some(r) = next else {
        let none = RichText::new("No fixed item ahead").font(theme::text(theme::LABEL));
        ui.label(none.color(t.text_muted));
        return;
    };
    let secs = r.start.timestamp().duration_since(now).as_secs().max(0) as u64;
    let left = mmss(std::time::Duration::from_secs(secs));
    // A narrow window puts the time first, so a cut keeps the time.
    let (text, colour) = if !wide {
        let colour = if secs < size::NOW_S {
            t.on_air
        } else if secs < size::SOON_S {
            t.warn
        } else {
            t.text
        };
        let glyph = if secs < size::SOON_S { "⚠" } else { "⏱" };
        (format!("{glyph} {left} {}", r.title), colour)
    } else if secs < size::NOW_S {
        (format!("⚠ {} starts in {left}", r.title), t.on_air)
    } else if secs < size::SOON_S {
        (format!("⚠ Next: {} in {left}", r.title), t.warn)
    } else {
        (format!("⏱ Next: {} in {left}", r.title), t.text)
    };
    let text = RichText::new(text)
        .font(theme::mono(theme::SECTION))
        .strong();
    let at = hms(&r.start);
    ui.add(egui::Label::new(text.color(colour)).truncate())
        .on_hover_text(format!("{} is fixed at {at}", r.title));
}

/// The "Stop after current" toggle. Its word changes with its state.
fn stop_after(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, t: &Tokens) {
    let stop = view.stop_after_current;
    let word = if stop {
        "⏹ Stops after current"
    } else {
        "Stop after current"
    };
    let fill = if stop { t.warn_fill } else { t.field };
    let button = egui::Button::new(RichText::new(word).font(theme::text(theme::CAPTION)))
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, t.field_border));
    let hint = view.say(
        "The station stops when the item on air ends · preview: here only",
        "The station stops when the item on air ends",
    );
    if ui.add(button).on_hover_text(hint).clicked() {
        actions.push(Action::Edit(Edit::StopAfterCurrent(!stop)));
    }
}

/// What the listeners get: LIVE only while Icecast is connected and Out has sound.
/// The preview never shows LIVE. A word and an icon carry the state, not the colour alone.
fn live_chip(
    ui: &mut egui::Ui,
    view: &StationView,
    wide: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let out = view.meters.iter().find(|m| m.label == "Out");
    let sound =
        out.is_some_and(|m| !m.mute && m.level_db[0].max(m.level_db[1]) > size::METER_MIN_DB);
    let (word, ink, fill, why) = match (&view.output.local, &view.output.icecast) {
        (Status::Preview, _) => (
            "◇ PREVIEW",
            t.text_muted,
            t.field,
            "UI preview: no audio goes out".to_owned(),
        ),
        (_, Status::Connected) if sound => ("● LIVE", t.on_air, t.on_air_bg, "on air".to_owned()),
        (_, Status::Connected) => (
            "○ SILENT",
            t.warn,
            t.warn_fill,
            "connected, but Out is silent".to_owned(),
        ),
        (_, Status::Reconnecting { attempt }) => (
            "↻ RECONNECTING",
            t.warn,
            t.warn_fill,
            format!("reconnecting, try {attempt}"),
        ),
        (_, Status::Failed(e)) => ("○ OFF AIR", t.text_muted, t.field, format!("down: {e}")),
        (_, _) => (
            "○ OFF AIR",
            t.text_muted,
            t.field,
            "Icecast is not connected".to_owned(),
        ),
    };
    let full_word = word;
    let ice = &view.icecast;
    let listeners = view
        .output
        .listeners
        .map_or("listeners unknown".to_owned(), |n| format!("{n} listeners"));
    let clear = if ice.password_in_clear() {
        format!("\n{}", crate::mixer::PASSWORD_IN_CLEAR)
    } else {
        String::new()
    };
    let hint = format!(
        "{why}\nMount {} · {listeners} · {} {} kbps{clear}\nClick for the Icecast settings",
        ice.mount,
        ice.format.label(),
        ice.kbps
    );
    // A narrow window keeps only the glyph of the chip. The tooltip still has the words.
    let word = if wide {
        word.to_owned()
    } else {
        word.chars().take(1).collect()
    };
    let hint = if wide {
        hint
    } else {
        format!("{word_all}\n{hint}", word_all = full_word)
    };
    let text = RichText::new(word).font(theme::mono(theme::LABEL)).strong();
    let chip = egui::Button::new(text.color(ink))
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, ink));
    if ui.add(chip).on_hover_text(hint).clicked() {
        actions.push(Action::OpenIcecast);
    }
}

/// A compact stereo meter of Out: two bars, the zones from the theme and a peak mark.
fn out_vu(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, t: &Tokens) {
    let Some(m) = view.meters.iter().find(|m| m.label == "Out") else {
        return;
    };
    let h = size::VU_BAR_H * 2.0 + space::XXS;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size::VU_W, h), Sense::click());
    let p = ui.painter();
    for ch in 0..2 {
        let top = rect.top() + ch as f32 * (size::VU_BAR_H + space::XXS);
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(rect.width(), size::VU_BAR_H),
        );
        p.rect_filled(bar, t.radius_sm, t.canvas);
        let x_of = |v: f32| bar.left() + bar.width() * mixer::frac(v);
        let level = m.level_db[ch];
        for (lo, hi, colour) in [
            (size::METER_MIN_DB, size::METER_AMBER_DB, t.meter_low),
            (size::METER_AMBER_DB, size::METER_RED_DB, t.meter_mid),
            (size::METER_RED_DB, size::METER_MAX_DB, t.meter_high),
        ] {
            if level > lo {
                let seg =
                    egui::Rect::from_x_y_ranges(x_of(lo)..=x_of(level.min(hi)), bar.y_range());
                p.rect_filled(seg, 0.0, colour);
            }
        }
        let peak = m.peak_db[ch];
        if peak > size::METER_MIN_DB {
            p.vline(
                x_of(peak),
                bar.y_range(),
                Stroke::new(size::HAIRLINE * 2.0, t.text),
            );
        }
    }
    let db = |v: f32| {
        if v <= size::METER_MIN_DB {
            "−∞".to_owned()
        } else {
            format!("{v:.1}")
        }
    };
    let hint = format!(
        "Out · peak L {} dB, R {} dB · simulated levels\nClick for the mixer",
        db(m.peak_db[0]),
        db(m.peak_db[1])
    );
    let resp = resp.on_hover_text(hint);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Out meter"));
    if resp.clicked() {
        actions.push(Action::OpenWindow(crate::windows::ToolWindow::Mixer));
    }
}

/// Auto, Assist and Manual as a segmented control.
fn mode_control(
    ui: &mut egui::Ui,
    view: &StationView,
    wide: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::XXS;
        // The parent lays out right to left, so the reversed list reads AUTO · ASSIST · MANUAL.
        for mode in PlayMode::ALL.iter().rev() {
            let on = view.play_mode == *mode;
            // A narrow window shows two letters; the tooltip names the mode.
            let word = if wide {
                mode.label().to_owned()
            } else {
                short_mode(*mode).to_owned()
            };
            let text = RichText::new(word).font(theme::mono(theme::LABEL));
            let text = if on {
                text.strong().color(t.accent_ink)
            } else {
                text
            };
            let fill = if on { t.selection } else { t.field };
            let edge = if on { t.accent } else { t.field_border };
            let button = egui::Button::new(text)
                .fill(fill)
                .stroke(Stroke::new(size::HAIRLINE, edge));
            let hint = format!(
                "{} mode{}",
                mode.label(),
                view.say(
                    " · preview: stored here, the scheduler is not connected",
                    ""
                )
            );
            if ui.add(button).on_hover_text(hint).clicked() && !on {
                let edit = Action::Edit(Edit::SetMode(*mode));
                // Leaving Auto stops the automation, so it asks first.
                actions.push(if view.play_mode == PlayMode::Auto {
                    Action::Confirm {
                        text: format!(
                            "Switch to {}? The station stops starting items on its own.",
                            mode.label()
                        ),
                        yes: format!("Switch to {}", mode.label()),
                        then: Box::new(edit),
                    }
                } else {
                    edit
                });
            }
        }
    });
}

/// Two letters for a mode, for a narrow window.
fn short_mode(mode: PlayMode) -> &'static str {
    match mode {
        PlayMode::Auto => "AU",
        PlayMode::Assist => "AS",
        PlayMode::Manual => "MN",
    }
}

/// The instant of the snapshot, from the clock of the UI.
fn now_ts(_deck: &DeckView, ui: &egui::Ui) -> Option<jiff::Timestamp> {
    ui.ctx()
        .data(|d| d.get_temp::<jiff::Timestamp>(egui::Id::new(NOW_KEY)))
}

/// The key of the snapshot instant in the egui data, for the countdowns of the decks.
pub(crate) const NOW_KEY: &str = "decks_now";

/// The line under a deck: when it starts, or when it started and ends.
fn start_line(deck: &DeckView, now: Option<jiff::Timestamp>) -> Option<String> {
    let start = deck.start.as_ref()?;
    match deck.state {
        DeckState::Empty => None,
        DeckState::Paused => Some("Paused".to_owned()),
        DeckState::Playing | DeckState::Fading => {
            let end = jiff::SignedDuration::try_from(deck.len)
                .ok()
                .and_then(|d| start.timestamp().checked_add(d).ok())
                .map(|e| hms(&e.to_zoned(start.time_zone().clone())))
                .unwrap_or_default();
            Some(format!("Started {} · ends {end}", hms(start)))
        }
        DeckState::Ready if deck.waiting => Some("Waits for Next".to_owned()),
        DeckState::Ready => {
            let left = now.map_or(String::new(), |n| {
                let secs = start.timestamp().duration_since(n).as_secs().max(0) as u64;
                format!(" · in {}", mmss(std::time::Duration::from_secs(secs)))
            });
            if deck.hard {
                Some(format!("⏱ Starts {} sharp{left}", hms(start)))
            } else {
                Some(format!("Starts {}{left}", hms(start)))
            }
        }
    }
}
