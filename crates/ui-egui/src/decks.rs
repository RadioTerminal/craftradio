//! The top bar: four deck cards with their buttons, the mode and the station clock.

use crate::numbers;
use egui::{Align, DragValue, Layout, RichText, Sense, Stroke};

use crate::app::{Action, hms, mmss};
use crate::context_menu::{self, menu_item};
use crate::icons::{self, Icon};
use crate::mixer;
use crate::parts;
use crate::preferences::Page;
use crate::settings;
use crate::shortcuts;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DeckCommand, DeckState, DeckView, Edit, StationView, Status};
use crate::widgets::{self, Armed, Look, guard_button_sized, icon_button_sized, transport_button};
use radio_model::{Clamp, GainSource, ItemGain};

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
    /// The covers on the decks in the last frame. egui forgets a cover when it leaves.
    covers: Vec<String>,
    fade_s: [f32; 4],
    fade_in_s: [f32; 4],
    /// True when an action that cuts the item on air needs a second press.
    confirm: bool,
    /// The armed button: deck, action and the egui time of the first press.
    armed: Option<(usize, Guarded, f64)>,
    /// The deck that Space acts on.
    focused: usize,
    /// The decks the strip shows: the decks in use, and a deck past them that holds an item.
    shown: [bool; 4],
    /// The place of each deck in the queue, for this frame: 1 plays next.
    next: Vec<Option<usize>>,
    /// The view is the UI preview: nothing plays, so no gain applies.
    preview: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            covers: Vec::new(),
            fade_s: [DEFAULT_FADE_S; 4],
            fade_in_s: [DEFAULT_FADE_IN_S; 4],
            confirm: true,
            armed: None,
            focused: 0,
            shown: [true; 4],
            next: Vec::new(),
            preview: false,
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

    /// Focus deck `deck` for the keys. A deck that is not shown is ignored.
    pub(crate) fn focus(&mut self, deck: usize) {
        if self.shown.get(deck) == Some(&true) {
            self.focused = deck;
        }
    }

    pub(crate) fn focused(&self) -> usize {
        self.focused
    }

    /// True while any button of deck `deck` waits for its second press.
    fn deck_armed(&self, deck: usize, now: f64) -> bool {
        self.armed
            .is_some_and(|(d, _, at)| d == deck && now - at < size::ARM_S)
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

/// The decks the station shows: the decks in use, and a deck past them while it still
/// holds an item.
pub(crate) fn in_use(view: &StationView) -> [bool; 4] {
    let count = parts::deck_count(view);
    std::array::from_fn(|i| i < count || view.decks[i].state != DeckState::Empty)
}

/// True unless `strip` is the mixer strip of a deck that is not shown.
pub(crate) fn strip_in_use(view: &StationView, strip: &str) -> bool {
    let shown = in_use(view);
    view.decks
        .iter()
        .position(|d| strip.chars().eq([d.label]))
        .is_none_or(|i| shown[i])
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
        .data_mut(|d| d.insert_temp(egui::Id::unique(NOW_KEY), now_ts));
    state.preview = view.preview;
    let covers: Vec<String> = view
        .decks
        .iter()
        .filter_map(|d| d.cover.as_ref().map(|c| c.uri.clone()))
        .collect();
    for gone in state.covers.iter().filter(|u| !covers.contains(u)) {
        ui.ctx().forget_image(gone);
    }
    state.covers = covers;
    let labels = ['A', 'B', 'C', 'D'];
    for (i, label) in labels.iter().enumerate() {
        state.fade_s[i] = parts::f32(view, parts::DECKS, &fade_key(*label), DEFAULT_FADE_S);
        let key = fade_in_key(*label);
        state.fade_in_s[i] = parts::f32(view, parts::DECKS, &key, DEFAULT_FADE_IN_S);
    }
    state.confirm = parts::bool(view, parts::DECKS, CONFIRM_KEY, true);
    let now = ui.input(|i| i.time);
    if state.armed.is_some_and(|(_, _, at)| now - at > size::ARM_S) {
        state.armed = None;
    }
    if state.armed.is_some() {
        // The ring counts down, so the deck strip draws again soon.
        ui.ctx().request_repaint_after_secs(ARM_REPAINT_S);
    }
    state.next = next_ranks(&view.decks);
    let avail = ui.available_width();
    let wide = avail >= size::STRIP_WIDE;
    let clock_width = if avail >= size::STRIP_FULL {
        size::CLOCK_W
    } else if wide {
        size::CLOCK_MID_W
    } else {
        size::CLOCK_NARROW_W
    };
    let deck_min = if wide {
        size::DECK_MIN_W
    } else {
        size::DECK_TINY_W
    };
    state.shown = in_use(view);
    let shown: Vec<usize> = (0..view.decks.len()).filter(|&i| state.shown[i]).collect();
    if !state.shown[state.focused] {
        state.focused = 0;
    }
    // The cards share the width that is there: a narrow window makes narrow cards, and
    // nothing scrolls.
    let fit = (avail - clock_width) / shown.len() as f32 - space::M;
    let deck_min = deck_min.min(fit.max(0.0));
    ui.spacing_mut().item_spacing.x = space::M;
    egui_extras::StripBuilder::new(ui)
        .size(egui_extras::Size::exact(size::DECK_STRIP_H))
        .vertical(|mut strip| {
            strip.strip(|builder| {
                builder
                    .sizes(
                        egui_extras::Size::remainder().at_least(deck_min),
                        shown.len(),
                    )
                    .size(egui_extras::Size::exact(clock_width))
                    .horizontal(|mut strip| {
                        for &i in &shown {
                            let deck = &view.decks[i];
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
                            cluster(ui, view, state, actions, clock_width, t);
                        });
                    });
            });
        });
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
fn primary(deck: &DeckView) -> (&'static str, Icon, &'static str) {
    match deck.state {
        DeckState::Playing => ("Pause", Icon::Pause, "Pause"),
        DeckState::Paused => ("Resume", Icon::Play, "Resume"),
        _ => ("Play", Icon::Play, "Play this deck"),
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
    // The edge has the colour of the deck. The deck on air has a thick edge, its fill and
    // the ON AIR word, so the colour is not the only sign.
    // An armed button turns the edge to the danger colour. Its width stays, so the card
    // never changes size.
    let now = ui.input(|i| i.time);
    let edge = if state.deck_armed(i, now) {
        t.danger
    } else {
        Tokens::deck(i)
    };
    let edge_w = match deck.state {
        DeckState::Playing | DeckState::Fading => size::ON_AIR_EDGE,
        _ => size::HAIRLINE,
    };
    let frame = theme::card_frame(t)
        .inner_margin(pad::DECK)
        .fill(card_fill(i, deck, t))
        .stroke(Stroke::new(edge_w, edge));
    let inner = width - frame.total_margin().sum().x - edge_w * 2.0;
    let frame_h = frame.total_margin().sum().y;
    // The badge and the title take the right-click. A card-wide sensor would take the button clicks.
    let shown = frame.show(ui, |ui| {
        ui.set_width(inner);
        // Every card fills the strip, so the four are as tall as each other.
        ui.set_min_height(size::DECK_STRIP_H - frame_h);
        ui.vertical(|ui| deck_body(ui, i, decks, state, actions, t))
            .inner
    });
    ui.ctx()
        .data_mut(|d| d.insert_temp(card_rect_id(i), shown.response.rect));
    shown.inner
}

/// The fill of a deck card: a dark tint of the deck colour while it is on air (playing or
/// fading), else the plain field.
fn card_fill(i: usize, deck: &DeckView, t: &Tokens) -> egui::Color32 {
    if matches!(deck.state, DeckState::Playing | DeckState::Fading) {
        t.deck_on_air_fill(i)
    } else {
        t.field
    }
}

/// Where the card of deck `i` was drawn, for tests.
pub(crate) fn card_rect_id(i: usize) -> egui::Id {
    egui::Id::unique(("deck_card_rect", i))
}

fn deck_body(
    ui: &mut egui::Ui,
    i: usize,
    decks: &[DeckView],
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) -> egui::Response {
    ui.spacing_mut().item_spacing.y = space::S;
    let deck = &decks[i];
    let now = ui.input(|i| i.time);
    let head = ui.horizontal(|ui| {
        let focused = state.focused == i;
        let keys = shortcuts::label_of(ui.ctx(), &shortcuts::focus_deck(i.min(3)));
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
            let mut open = false;
            settings::settings_button_sized(
                ui,
                &format!("Deck {name}"),
                &mut open,
                size::DECK_BUTTON,
            );
            if open {
                actions.push(Action::OpenPreferences(Page::Decks));
            }
            // A gap keeps the settings button away from the transport buttons. A narrow card
            // keeps a smaller gap.
            let narrow = ui.available_width() < size::DECK_MIN_W;
            ui.add_space(if narrow { space::XS } else { space::M });
            ui.add_enabled_ui(deck.state != DeckState::Empty, |ui| {
                // One main action: Play this deck, or Pause and Resume on the deck on air.
                let keys = shortcuts::label(ui.ctx(), shortcuts::PLAY_PAUSE);
                let (verb, icon, _) = primary(deck);
                let accessible = format!("{verb} deck {name}");
                if is_live(deck) {
                    let hint = format!("{verb} the item on air ({keys} on the focused deck)");
                    if icon_button_sized(ui, icon, false, &accessible, &hint, size::DECK_BUTTON)
                        .clicked()
                    {
                        actions.push(command(i, DeckCommand::PlayPause));
                    }
                } else {
                    let hint = format!(
                        "Play this deck now: it fades in over {:.1} s and the item on air fades \
                         out. The plan goes on from this deck ({keys} on the focused deck)",
                        state.fade_in(i)
                    );
                    let confirm = format!("Confirm: play deck {name} now (cuts the item on air)");
                    let note = format!(
                        "Play deck {name} now cuts the item on air: press again to confirm"
                    );
                    let armed = state.armed_left(i, Guarded::Play, now).map(|left| Armed {
                        left,
                        name: &confirm,
                        note: &note,
                    });
                    let play =
                        guard_button_sized(ui, icon, &accessible, &hint, armed, size::DECK_BUTTON);
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
    let title = if deck.title.is_empty() {
        "—"
    } else {
        deck.title.as_str()
    };
    // The cover of the file sits left of the title and the playlist, as tall as both rows.
    let cover = deck
        .cover
        .as_ref()
        .filter(|_| deck.state != DeckState::Empty);
    let title = ui
        .horizontal(|ui| {
            if let Some(c) = cover {
                ui.add(
                    egui::Image::from_bytes(c.uri.clone(), c.bytes.clone())
                        .fit_to_exact_size(egui::Vec2::splat(size::DECK_COVER))
                        .corner_radius(t.radius)
                        .alt_text("Cover"),
                );
            }
            ui.vertical(|ui| title_rows(ui, i, deck, title, state, t))
                .inner
        })
        .inner;
    progress(ui, deck, t);
    let left = deck.len.saturating_sub(deck.elapsed);
    let muted = |s: &str| {
        RichText::new(s)
            .font(theme::mono(theme::CODE))
            .color(t.text_muted)
    };
    // The time row: the time left on air, the length of a waiting deck, a dash when empty.
    // The largest number of the row is the same size in every state.
    ui.horizontal(|ui| {
        let big = |s: String| RichText::new(s).font(theme::mono(theme::SECTION)).strong();
        match deck.state {
            DeckState::Playing | DeckState::Fading | DeckState::Paused => {
                ui.label(big(format!("−{}", mmss(left))))
                    .on_hover_text("Time left on this item");
                let done = format!("{} / {}", mmss(deck.elapsed), mmss(deck.len));
                widgets::truncated_label(ui, muted(&done));
            }
            DeckState::Ready => {
                ui.label(big(mmss(deck.len)).color(t.text_muted))
                    .on_hover_text("Length of this item");
            }
            DeckState::Empty => {
                ui.label(big("–:––".to_owned()).color(t.text_muted));
            }
        }
    });
    // When the deck starts, or when it started and ends. The times come from the plan.
    // The line shows the longest form that fits; the title is the only text that truncates.
    let lines = start_lines(deck, now_ts(deck, ui));
    let room = ui.available_width();
    let font = theme::mono(theme::CODE);
    let fits = |s: &String| {
        ui.painter()
            .layout_no_wrap(s.clone(), font.clone(), t.text_muted)
            .size()
            .x
            <= room
    };
    let line = lines
        .iter()
        .find(|s| fits(s))
        .or(lines.last())
        .map_or("Nothing planned", String::as_str);
    ui.add(egui::Label::new(muted(line)).wrap_mode(egui::TextWrapMode::Extend));
    head.inner.union(title)
}

/// The title and the playlist row of a deck card. The title takes the right-click.
fn title_rows(
    ui: &mut egui::Ui,
    i: usize,
    deck: &DeckView,
    title: &str,
    state: &State,
    t: &Tokens,
) -> egui::Response {
    ui.spacing_mut().item_spacing.y = space::S;
    let label = egui::Label::new(RichText::new(title).font(theme::text(theme::BODY)));
    let title = ui.add(label.truncate().sense(Sense::click()));
    // The playlist of the item, as a chip: its colour and its name. An empty deck keeps the row.
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(size::DOT_R * 2.0), Sense::hover());
        let (name, colour) = if deck.state == DeckState::Empty {
            ("Empty: nothing planned", t.text_muted)
        } else if deck.playlist.is_empty() {
            ("No playlist", t.text_muted)
        } else {
            ui.painter().circle_filled(
                dot.center(),
                size::DOT_R,
                Tokens::playlist(deck.playlist_color),
            );
            (
                deck.playlist.as_str(),
                t.readable(deck.playlist_color, card_fill(i, deck, t)),
            )
        };
        let name = RichText::new(name).font(theme::text(theme::CAPTION));
        // The item gain sits at the end of the row; the playlist name truncates first.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            gain_caption(ui, deck, state.preview, t);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                widgets::truncated_label(ui, name.color(colour));
            });
        });
    });
    title
}

/// A gain in dB as text, with a real minus sign: "−3.2 dB", "+1.5 dB", "0.0 dB".
fn db_text(db: f32) -> String {
    let v = (db * 10.0).round() / 10.0;
    let sign = if v < 0.0 {
        "−"
    } else if v > 0.0 {
        "+"
    } else {
        ""
    };
    format!("{sign}{:.1} dB", v.abs())
}

/// The short caption of an item gain, and whether it shows a gain that applies.
pub(crate) fn gain_words(g: &ItemGain) -> (String, bool) {
    match g.source {
        GainSource::Measured => (format!("RG {}", db_text(g.db)), true),
        GainSource::Tag => (format!("tag {}", db_text(g.db)), true),
        GainSource::NotMeasured => ("not measured".to_owned(), false),
        GainSource::Off => ("RG off".to_owned(), false),
    }
}

/// The tooltip of an item gain: the loudness, the peak, the target and any clamp.
pub(crate) fn gain_detail(g: &ItemGain, preview: bool) -> String {
    let mut lines = Vec::new();
    let level = |v: f32, unit: &str| format!("{} {unit}", db_text(v).trim_end_matches(" dB"));
    match (g.source, g.loudness) {
        (GainSource::Off, _) => {
            lines.push("Loudness normalisation is off: the item plays at unity.".to_owned());
        }
        (GainSource::NotMeasured, _) => lines
            .push("Not measured yet, and no ReplayGain tag: the item plays at unity.".to_owned()),
        (source, Some(l)) => {
            let from = if source == GainSource::Tag {
                "From the ReplayGain tag"
            } else {
                "Measured (EBU R128)"
            };
            lines.push(format!("{from}: {}", level(l.lufs, "LUFS")));
            lines.push(match l.peak_db {
                Some(p) => format!("True peak: {}", level(p, "dBTP")),
                None => "True peak: not known".to_owned(),
            });
        }
        (_, None) => {}
    }
    lines.push(format!("Target: {}", level(g.target_lufs, "LUFS")));
    let applies = matches!(g.source, GainSource::Measured | GainSource::Tag);
    if g.trim_db != 0.0 && applies {
        lines.push(format!("Trim: {}", db_text(g.trim_db)));
    }
    if let Some(c) = g.clamp {
        lines.push(
            match c {
                Clamp::Peak => "Boost held back: the true peak would pass −1 dBTP",
                Clamp::NoPeak => "No boost: the tag gives no peak",
                Clamp::MaxBoost => "Boost held at the max boost",
                Clamp::MaxCut => "Cut held at −24 dB",
            }
            .to_owned(),
        );
    }
    if applies {
        lines.push(format!("Gain on this deck: {}", db_text(g.db)));
    }
    if preview {
        lines.push("Preview: nothing plays, so no gain applies here.".to_owned());
    }
    lines.join("\n")
}

/// The item gain at the end of the playlist row: "RG −3.2 dB", "tag −2.1 dB", or "not
/// measured" in muted ink. It has the caption font of the row, so the row keeps its
/// height. The words carry the state, not the colour alone.
fn gain_caption(ui: &mut egui::Ui, deck: &DeckView, preview: bool, t: &Tokens) {
    let Some(g) = &deck.gain else { return };
    let (words, applied) = gain_words(g);
    let colour = if applied && !preview {
        t.text
    } else {
        t.text_muted
    };
    let text = RichText::new(words)
        .font(theme::text(theme::CAPTION))
        .color(colour);
    ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Truncate))
        .on_hover_text(gain_detail(g, preview));
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

/// How much room the status cluster has: it decides which words and parts show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// One column, icons only, no date.
    Narrow,
    /// Two columns. Only Play next keeps its word.
    Mid,
    /// Two columns. Every label and the date.
    Full,
}

impl Tier {
    fn of(width: f32) -> Self {
        if width >= size::CLOCK_W {
            Self::Full
        } else if width >= size::CLOCK_MID_W {
            Self::Mid
        } else {
            Self::Narrow
        }
    }
}

/// The block beside the decks. On top, the transport on the left and the LIVE chip, the clock
/// and the date on the right; under them the Out meter across the whole width. It is as tall
/// as a deck card and has the same edge.
fn cluster(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    width: f32,
    t: &Tokens,
) {
    let tier = Tier::of(width);
    let frame = theme::card_frame(t).inner_margin(pad::DECK);
    let margin = frame.total_margin();
    let inner_w = width - margin.sum().x;
    let inner_h = size::DECK_STRIP_H - margin.sum().y;
    frame.show(ui, |ui| {
        ui.set_width(inner_w);
        ui.set_min_height(inner_h);
        if tier == Tier::Narrow {
            ui.spacing_mut().item_spacing.y = space::XS;
            let y0 = ui.cursor().top();
            ui.horizontal(|ui| transport(ui, view, state, actions, tier));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::XS;
                live_chip(ui, view, false, actions, t);
                share_button(ui, view);
                ui.label(
                    RichText::new(hms(&view.now))
                        .font(theme::mono(theme::TITLE))
                        .strong(),
                );
            });
            let rest = (inner_h - (ui.cursor().top() - y0) - size::NOW_LINE - space::XS)
                .max(size::VU_NARROW_H);
            out_meter(ui, view, rest, false, actions, t);
            now_playing(ui, view, t);
            return;
        }
        ui.spacing_mut().item_spacing = egui::vec2(space::M, space::S);
        let right_w = size::CLOCK_COL_W;
        let left_w = inner_w - right_w - space::M;
        let top = ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, 0.0),
                Layout::top_down(Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(t.canvas)
                        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
                        .corner_radius(t.radius_lg)
                        .inner_margin(pad::GROUP)
                        .show(ui, |ui| {
                            ui.set_width(left_w - pad::GROUP.sum().x - 2.0 * size::HAIRLINE);
                            ui.horizontal(|ui| transport(ui, view, state, actions, tier));
                        });
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(right_w, 0.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(right_w);
                    ui.spacing_mut().item_spacing.y = space::XS;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = space::XS;
                        live_chip(ui, view, true, actions, t);
                        share_button(ui, view);
                    });
                    ui.label(
                        RichText::new(hms(&view.now))
                            .font(theme::mono(theme::TITLE))
                            .strong(),
                    );
                    let date = view.now.strftime("%a %-d %b").to_string();
                    let date = RichText::new(date).font(theme::text(theme::LABEL));
                    ui.label(date.color(t.text_muted));
                },
            );
        });
        let rest = (inner_h - top.response.rect.height() - size::NOW_LINE - 2.0 * space::S)
            .max(size::VU_NARROW_H);
        out_meter(ui, view, rest, true, actions, t);
        now_playing(ui, view, t);
    });
}

/// The deck that listeners hear now: the deck that plays, else the deck that fades out.
/// A paused deck is silent, so it does not count.
fn audible_deck(decks: &[DeckView]) -> Option<usize> {
    decks
        .iter()
        .position(|d| d.state == DeckState::Playing)
        .or_else(|| decks.iter().position(|d| d.state == DeckState::Fading))
}

/// The line under the Out meter: the badge of the deck on air, its "Artist – Title" and the
/// time left at the right end. With nothing on air it says so, and never shows a stale item.
fn now_playing(ui: &mut egui::Ui, view: &StationView, t: &Tokens) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(w, size::NOW_LINE),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.x = space::S;
            let small = |s: String| RichText::new(s).font(theme::text(theme::LABEL));
            let Some((i, deck)) =
                audible_deck(&view.decks).and_then(|i| Some((i, view.decks.get(i)?)))
            else {
                ui.label(small("Nothing playing".to_owned()).color(t.text_muted));
                return;
            };
            // The badge: the deck colour with the letter on it, as on the deck card.
            let (rect, badge) =
                ui.allocate_exact_size(egui::Vec2::splat(size::DECK_MARK), Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, t.radius_sm, Tokens::deck(i));
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                deck.label,
                theme::mono(theme::CAPTION),
                t.deck_ink(i),
            );
            badge.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Label,
                    true,
                    format!("Deck {} on air", deck.label),
                )
            });
            let left = deck.len.saturating_sub(deck.elapsed);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let time = RichText::new(format!("−{}", mmss(left))).font(theme::mono(theme::CODE));
                ui.label(time.color(t.text_muted))
                    .on_hover_text("Time left on the item on air");
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let title = if deck.title.is_empty() {
                        "—"
                    } else {
                        deck.title.as_str()
                    };
                    widgets::truncated_label(ui, small(title.to_owned())).on_hover_text(title);
                });
            });
        },
    );
}

/// The transport of the station: Play next, Fade out and Stop after current, at one height.
/// Play next and Fade out arm first when they cut the item on air before its end. A narrow
/// cluster, or a tight one, shows icons alone; the names and the tooltips stay.
fn transport(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    tier: Tier,
) {
    let now = ui.input(|i| i.time);
    let air = on_air_deck(&view.decks);
    let next = state.next_deck();
    let full = tier == Tier::Full;
    let narrow = tier == Tier::Narrow;
    ui.spacing_mut().item_spacing.x = space::S;
    ui.add_enabled_ui(next.is_some(), |ui| {
        let deck = next.unwrap_or(0);
        let keys = shortcuts::label(ui.ctx(), shortcuts::PLAY_NEXT);
        let hint = format!(
            "Play next: start the NEXT 1 deck now. It fades in and the item on air fades out ({keys})"
        );
        let letter = view.decks.get(deck).map_or('?', |d| d.label);
        let confirm = format!("Confirm: play deck {letter} now (cuts the item on air)");
        let note = format!(
            "Play next starts deck {letter} now and cuts the item on air: press again to confirm"
        );
        let armed = next
            .and_then(|d| state.armed_left(d, Guarded::PlayNext, now))
            .map(|left| Armed {
                left,
                name: &confirm,
                note: &note,
            });
        let play = transport_button(
            ui,
            Icon::SkipForward,
            "Play next",
            Look::Primary,
            narrow,
            armed,
            &hint,
        );
        let cut = cuts(view, deck, state);
        if play.clicked()
            && let Some(a) = state.press(deck, Guarded::PlayNext, cut, now)
        {
            actions.push(a);
        }
    });
    ui.add_enabled_ui(air.is_some(), |ui| {
        let deck = air.unwrap_or(0);
        let keys = shortcuts::label(ui.ctx(), shortcuts::FADE);
        let hint = format!("Fade out the item on air ({keys})");
        let letter = view.decks.get(deck).map_or('?', |d| d.label);
        let confirm = format!("Confirm: fade out deck {letter} now (cuts the item on air)");
        let note = format!(
            "Fade out ends the item on deck {letter} before its end: press again to confirm"
        );
        let armed = air
            .and_then(|d| state.armed_left(d, Guarded::Fade, now))
            .map(|left| Armed {
                left,
                name: &confirm,
                note: &note,
            });
        let fade = transport_button(
            ui,
            Icon::TrendingDown,
            "Fade out",
            Look::Secondary,
            !full,
            armed,
            &hint,
        );
        if fade.clicked()
            && let Some(a) = state.press(deck, Guarded::Fade, true, now)
        {
            actions.push(a);
        }
    });
    stop_after(ui, view, actions, !full);
}

/// The "Stop after current" toggle, at the height of the other transport buttons.
fn stop_after(ui: &mut egui::Ui, view: &StationView, actions: &mut Vec<Action>, compact: bool) {
    let stop = view.stop_after_current;
    let hint = view.say(
        "The station stops when the item on air ends · preview: here only",
        "The station stops when the item on air ends",
    );
    let (icon, look) = if stop {
        (Icon::SquareCheckBig, Look::On)
    } else {
        (Icon::Square, Look::Secondary)
    };
    let hint = if stop {
        format!("On. {hint}")
    } else {
        hint.to_owned()
    };
    let button = transport_button(ui, icon, "Stop after current", look, compact, None, &hint);
    if button.clicked() {
        actions.push(Action::Edit(Edit::StopAfterCurrent(!stop)));
    }
}

/// The on-air chip, and the switch for streaming to listeners. A click turns streaming
/// on or off. It shows ON AIR while listeners can tune in and Out has sound.
/// The preview never shows ON AIR. A word and an icon carry the state, not the colour alone.
fn live_chip(
    ui: &mut egui::Ui,
    view: &StationView,
    wide: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let out = view.meters.iter().find(|m| m.is_program());
    let sound =
        out.is_some_and(|m| !m.mute && m.level_db[0].max(m.level_db[1]) > size::METER_MIN_DB);
    let on = view.output.streaming.unwrap_or(false);
    let listeners = view
        .output
        .webrtc_listeners_text()
        .map_or(String::new(), |n| format!(", {n}"));
    let (icon, word, ink, fill, why) = match (&view.output.local, &view.output.webrtc) {
        (Status::Preview, _) => (
            Icon::Eye,
            "PREVIEW",
            t.text_muted,
            t.field,
            "UI preview: no audio goes out".to_owned(),
        ),
        (_, Status::Connected) if on && sound => (
            Icon::Radio,
            "ON AIR",
            t.on_air,
            t.on_air_bg,
            format!("streaming to listeners{listeners}"),
        ),
        (_, Status::Connected) if on => (
            Icon::VolumeX,
            "SILENT",
            t.warn,
            t.warn_fill,
            format!("streaming, but Out is silent{listeners}"),
        ),
        (_, Status::Reconnecting { .. }) if on => (
            Icon::RefreshCw,
            "STARTING",
            t.warn,
            t.warn_fill,
            "streaming starts".to_owned(),
        ),
        (_, Status::Failed(e)) if on => (
            Icon::Circle,
            "OFF AIR",
            t.danger,
            t.field,
            format!("streaming failed: {e}"),
        ),
        _ => (
            Icon::Circle,
            "OFF AIR",
            t.text_muted,
            t.field,
            "not streaming to listeners".to_owned(),
        ),
    };
    let icecast = match &view.output.icecast {
        Status::Connected => "connected",
        Status::Reconnecting { .. } => "reconnecting",
        Status::Failed(_) => "failed",
        Status::OffAir => "off air",
        Status::Preview | Status::NotConnected => "not connected",
    };
    let click = match view.output.streaming {
        Some(true) => "Click to stop streaming",
        Some(false) => "Click to stream to listeners, with a link anyone can open",
        None => "This station has no streaming to listeners",
    };
    let router = match (&view.output.forwarded, on) {
        (Some(addr), _) => {
            format!("\nRouter: forwards {addr}, so phones on mobile data reach the station")
        }
        (None, true) => {
            "\nRouter: no port forwarded (no UPnP or NAT-PMP), so some listeners may not connect"
                .to_owned()
        }
        (None, false) => String::new(),
    };
    let hint = format!("{word}: {why}{router}\nIcecast: {icecast}\n{click}");
    // A narrow cluster keeps only the icon of the chip. The tooltip still has the words.
    let label = if wide {
        icons::lead(icon, word)
    } else {
        icons::glyph(icon).to_string()
    };
    let text = RichText::new(label)
        .font(theme::mono(theme::LABEL))
        .strong();
    let chip = egui::Button::new(text.color(ink))
        .fill(fill)
        .stroke(Stroke::new(size::HAIRLINE, ink))
        .min_size(egui::vec2(0.0, size::CONTROL_SM));
    let resp = ui
        .add_enabled(view.output.streaming.is_some(), chip)
        .on_hover_text(hint)
        .on_disabled_hover_text(format!("{word}: {why}"));
    // Screen readers hear a switch named by its word, on while streaming is on.
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, word));
    if resp.clicked() {
        actions.push(Action::Edit(Edit::SetStreaming(!on)));
    }
}

/// The Share button beside the on-air chip, while streaming runs. Its menu copies the
/// link that plays the station from anywhere, and the address on the local network.
fn share_button(ui: &mut egui::Ui, view: &StationView) {
    let anywhere = view.output.listen_link.as_deref();
    let local = view.output.local_link.as_deref();
    if anywhere.is_none() && local.is_none() {
        return;
    }
    let button = widgets::icon_button_sized(
        ui,
        Icon::Share2,
        false,
        "Share",
        "Share the station: copy a link that listeners open",
        size::CONTROL_SM,
    );
    egui::Popup::menu(&button).show(|ui| {
        // The link works only while a tracker carries it, so the row waits for one.
        if context_menu::menu_item_if(ui, anywhere.is_some(), "Copy listen link", None)
            && let Some(link) = anywhere
        {
            ui.ctx().copy_text(link.to_owned());
        }
        if context_menu::menu_item_if(ui, local.is_some(), "Copy local address", None)
            && let Some(link) = local
        {
            ui.ctx().copy_text(link.to_owned());
        }
    });
}

/// The levels of the dB scale under the Out meter.
const SCALE_DB: [f32; 6] = [-48.0, -24.0, -12.0, -6.0, -3.0, 0.0];

/// The stereo meter of Out across the column: two bars, the zones from the theme and a peak
/// mark each. With `full`, a dB scale runs under the bars; without it, only the bars show,
/// for a narrow window. The bars use the whole width: the peak in dB is in the tooltip.
fn out_meter(
    ui: &mut egui::Ui,
    view: &StationView,
    height: f32,
    full: bool,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let Some(m) = view.meters.iter().find(|m| m.is_program()) else {
        return;
    };
    let db = |v: f32| {
        if v <= size::METER_MIN_DB {
            "−∞".to_owned()
        } else {
            format!("{v:.1}")
        }
    };
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    let p = ui.painter();
    let caption = theme::mono(theme::CAPTION);
    let right = rect.right();
    let scale_h = if full {
        theme::CAPTION + space::XS
    } else {
        0.0
    };
    let zone_h = if full { size::HAIRLINE * 2.0 } else { 0.0 };
    let bars_h = (rect.height() - scale_h - zone_h).max(0.0);
    let bar_h = ((bars_h - size::VU_GAP) / 2.0).max(0.0);
    let (left, span) = (rect.left(), (right - rect.left()).max(0.0));
    let x_of = |v: f32| left + span * mixer::frac(v);
    let zones = [
        (size::METER_MIN_DB, size::METER_AMBER_DB, t.meter_low),
        (size::METER_AMBER_DB, size::METER_RED_DB, t.meter_mid),
        (size::METER_RED_DB, size::METER_MAX_DB, t.meter_high),
    ];
    // LED bars, as on the mixer strips: an unlit LED shows its zone faintly, the held peak is
    // one bright LED.
    let step = size::METER_SEG_H + size::METER_SEG_GAP;
    let count = (span / step).floor().max(1.0);
    let db_span = size::METER_MAX_DB - size::METER_MIN_DB;
    for ch in 0..2 {
        let top = rect.top() + ch as f32 * (bar_h + size::VU_GAP);
        let y = egui::Rangef::new(top, top + bar_h);
        let peak_led = ((mixer::frac(m.peak_db[ch]) * count).ceil() - 1.0).max(-1.0);
        let mut k = 0.0;
        while k < count {
            let low = size::METER_MIN_DB + db_span * k / count;
            let zone = zones
                .iter()
                .rev()
                .find(|(lo, ..)| low >= *lo)
                .map_or(t.meter_low, |z| z.2);
            let colour = if k == peak_led {
                t.text
            } else if m.level_db[ch] > low {
                zone
            } else {
                zone.gamma_multiply(size::METER_UNLIT)
            };
            let x = left + k * step;
            let led = egui::Rect::from_x_y_ranges(x..=(x + size::METER_SEG_H), y);
            p.rect_filled(led, 0.0, colour);
            k += 1.0;
        }
    }
    if full {
        let bars_bottom = rect.top() + 2.0 * bar_h + size::VU_GAP;
        // The zones stay visible when the bars are silent.
        for (lo, hi, colour) in zones {
            let strip = egui::Rect::from_x_y_ranges(
                x_of(lo)..=x_of(hi),
                (bars_bottom + size::HAIRLINE)..=(bars_bottom + size::HAIRLINE + zone_h),
            );
            p.rect_filled(strip, 0.0, colour);
        }
        for tick in SCALE_DB {
            let x = x_of(tick);
            // A short mark under the bars, so no line crosses the LEDs.
            p.vline(
                x,
                bars_bottom..=(bars_bottom + size::HAIRLINE + zone_h + space::XXS),
                Stroke::new(size::HAIRLINE, t.text_muted),
            );
            let label = if tick == 0.0 {
                "0".to_owned()
            } else {
                format!("{tick:.0}").replace('-', "−")
            };
            // -6 and -3 sit close together: -6 hangs to the left of its tick.
            let (x, anchor) = if tick == -6.0 {
                (x + space::XXS, egui::Align2::RIGHT_TOP)
            } else {
                (x, egui::Align2::CENTER_TOP)
            };
            p.text(
                egui::pos2(x, bars_bottom + zone_h + size::HAIRLINE + space::XXS),
                anchor,
                label,
                caption.clone(),
                t.text_muted,
            );
        }
    }
    let hint = format!(
        "Out · peak L {} dB, R {} dB · simulated levels\nClick for the mixer",
        db(m.peak_db[0]),
        db(m.peak_db[1])
    );
    let resp = resp.on_hover_text(hint);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Out meter"));
    if resp.clicked() {
        actions.push(Action::OpenWindow(crate::windows::ToolWindow::Mixer));
    }
}

/// The instant of the snapshot, from the clock of the UI.
fn now_ts(_deck: &DeckView, ui: &egui::Ui) -> Option<jiff::Timestamp> {
    ui.ctx()
        .data(|d| d.get_temp::<jiff::Timestamp>(egui::Id::unique(NOW_KEY)))
}

/// The key of the snapshot instant in the egui data, for the countdowns of the decks.
pub(crate) const NOW_KEY: &str = "decks_now";

/// The line under a deck as candidates, the longest first: when it starts, or when it
/// started and ends. A narrow card takes a shorter one; the less important part goes first.
fn start_lines(deck: &DeckView, now: Option<jiff::Timestamp>) -> Vec<String> {
    let Some(start) = deck.start.as_ref() else {
        return vec!["Nothing planned".to_owned()];
    };
    match deck.state {
        DeckState::Empty => vec!["Nothing planned".to_owned()],
        DeckState::Paused => vec!["Paused".to_owned()],
        DeckState::Playing | DeckState::Fading => {
            let end = jiff::SignedDuration::try_from(deck.len)
                .ok()
                .and_then(|d| start.timestamp().checked_add(d).ok())
                .map(|e| e.to_zoned(start.time_zone().clone()));
            let from = hms(start);
            match end {
                Some(end) => {
                    let end_hm = end.strftime("%H:%M").to_string();
                    vec![
                        format!("{from} → {}", hms(&end)),
                        format!("{from} → {end_hm}"),
                        from,
                    ]
                }
                None => vec![from],
            }
        }
        DeckState::Ready => {
            let at = hms(start);
            let sharp = if deck.hard { "⏱ " } else { "" };
            let tail = if deck.hard { " sharp" } else { "" };
            let left = now.map(|n| {
                let secs = start.timestamp().duration_since(n).as_secs().max(0) as u64;
                mmss(std::time::Duration::from_secs(secs))
            });
            let mut out = Vec::new();
            if let Some(left) = &left {
                out.push(format!("{sharp}Starts {at}{tail} · in {left}"));
            }
            out.push(format!("{sharp}Starts {at}{tail}"));
            out.push(format!("{sharp}{at}"));
            out
        }
    }
}

/// The Decks page of Preferences: the confirm guard of all decks, and the fades of each deck.
/// Restore page defaults on the Decks page: confirm a cut, and the default fades of every deck.
pub(crate) fn restore_defaults(actions: &mut Vec<Action>) {
    actions.push(parts::set(parts::DECKS, CONFIRM_KEY, true));
    actions.push(parts::set(
        parts::DECKS,
        radio_model::DECK_COUNT_KEY,
        radio_model::DEFAULT_DECKS as u64,
    ));
    for label in ['A', 'B', 'C', 'D'] {
        actions.push(parts::set(
            parts::DECKS,
            &fade_key(label),
            f64::from(DEFAULT_FADE_S),
        ));
        actions.push(parts::set(
            parts::DECKS,
            &fade_in_key(label),
            f64::from(DEFAULT_FADE_IN_S),
        ));
    }
}

pub(crate) fn preferences(
    ui: &mut egui::Ui,
    view: &StationView,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let old_count = parts::deck_count(view);
    let mut count = old_count;
    settings::section(ui, "On air", t, |ui| {
        let old_confirm = parts::bool(view, parts::DECKS, CONFIRM_KEY, true);
        let mut confirm = old_confirm;
        ui.checkbox(
            &mut confirm,
            "Confirm a cut of the item on air (press Play or Fade out twice)",
        );
        if confirm != old_confirm {
            actions.push(parts::set(parts::DECKS, CONFIRM_KEY, confirm));
        }
        settings::form_row(ui, "Number of decks", t, |ui| {
            let field = numbers::field(
                ui,
                DragValue::new(&mut count).range(radio_model::MIN_DECKS..=radio_model::MAX_DECKS),
            );
            ui.add(field).on_hover_text(
                "The station loads items on this many decks. A deck past this number plays \
                 out what it holds and then goes away. A hard event, a bed or a fallback \
                 file may still take one.",
            )
        });
    });
    if count != old_count {
        actions.push(parts::set(
            parts::DECKS,
            radio_model::DECK_COUNT_KEY,
            count as u64,
        ));
    }
    for label in ['A', 'B', 'C', 'D'].into_iter().take(old_count) {
        settings::section(ui, &format!("Deck {label}"), t, |ui| {
            deck_fades(ui, view, label, actions, t);
        });
    }
}

/// The fade-in and fade-out rows of deck `label`.
fn deck_fades(
    ui: &mut egui::Ui,
    view: &StationView,
    label: char,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let (out_key, in_key) = (fade_key(label), fade_in_key(label));
    let old_out = parts::f32(view, parts::DECKS, &out_key, DEFAULT_FADE_S);
    let old_in = parts::f32(view, parts::DECKS, &in_key, DEFAULT_FADE_IN_S);
    let (mut fade_out, mut fade_in) = (old_out, old_in);
    settings::form_row(ui, &format!("Deck {label} fade-in"), t, |ui| {
        let field = numbers::field(
            ui,
            DragValue::new(&mut fade_in)
                .range(size::FADE_IN_MIN_S..=size::FADE_MAX_S)
                .speed(0.1)
                .suffix(" s"),
        );
        ui.add(field)
            .on_hover_text("Play this deck and Play next fade this deck in so long")
    });
    settings::form_row(ui, &format!("Deck {label} fade-out"), t, |ui| {
        let field = numbers::field(
            ui,
            DragValue::new(&mut fade_out)
                .range(size::FADE_MIN_S..=size::FADE_MAX_S)
                .speed(0.1)
                .suffix(" s"),
        );
        ui.add(field)
            .on_hover_text("Fade out (in the header, or its key) fades this deck out so long")
    });
    if fade_out != old_out {
        actions.push(parts::set(parts::DECKS, &out_key, f64::from(fade_out)));
    }
    if fade_in != old_in {
        actions.push(parts::set(parts::DECKS, &in_key, f64::from(fade_in)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radio_model::{Loudness, LoudnessSettings};

    /// A trim that does not apply (normalisation off, or nothing known) is not shown as
    /// part of the gain.
    #[test]
    fn the_tooltip_shows_a_trim_only_when_it_applies() {
        let measured = Some((
            Loudness {
                lufs: -20.0,
                peak_db: Some(-6.0),
            },
            GainSource::Measured,
        ));
        let on = LoudnessSettings::default();
        assert!(gain_detail(&on.gain(measured, -2.0), false).contains("Trim"));
        let off = LoudnessSettings {
            enabled: false,
            ..on
        };
        assert!(!gain_detail(&off.gain(measured, -2.0), false).contains("Trim"));
        assert!(!gain_detail(&on.gain(None, -2.0), false).contains("Trim"));
    }
}
