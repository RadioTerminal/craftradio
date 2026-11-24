//! The key map: every keyboard shortcut of the app is a command of one craft-keymap catalog, as
//! in the Craft apps, so menus, tooltips and the keys all read the same chords. No shortcut
//! fires while a text field has the keyboard.

use std::sync::{Arc, OnceLock};

use craft_keymap::{Bindable, Binding, CommandRef, Keymap, Platform, Scope, Scopes};
use egui::{Event, KeyboardShortcut};
use serde_json::json;

/// The command ids. Menus and tooltips name a command; the keymap gives its chord.
pub(crate) const CUT: &str = "edit.cut";
pub(crate) const COPY: &str = "edit.copy";
pub(crate) const PASTE: &str = "edit.paste";
pub(crate) const DUPLICATE: &str = "edit.duplicate";
/// Remove the selected rows: Delete (Fn+Delete on a Mac) or Backspace (the Mac delete key).
pub(crate) const DELETE: &str = "edit.delete";
/// Select every row of the focused list.
pub(crate) const SELECT_ALL: &str = "edit.select_all";
pub(crate) const UNDO: &str = "edit.undo";
pub(crate) const REDO: &str = "edit.redo";
/// Play next starts the NEXT 1 deck. Fade out fades the item on air. Both arm first when
/// they cut the item on air, and take Command and Shift, so a stray press does not fire them.
pub(crate) const PLAY_NEXT: &str = "deck.play_next";
pub(crate) const FADE: &str = "deck.fade_out";
/// Space plays the focused deck, or pauses it when it is on air.
pub(crate) const PLAY_PAUSE: &str = "deck.play_pause";
pub(crate) const STOP_CARTS: &str = "cart.stop_all";
/// The On air switch: it arms first, and a second press within the arm time switches.
pub(crate) const AIR_TOGGLE: &str = "air.toggle";
/// Preferences, as in every Craft app.
pub(crate) const PREFERENCES: &str = "app.preferences";
/// The search field of Preferences takes the focus.
pub(crate) const FIND: &str = "app.search_preferences";
pub(crate) const UP: &str = "list.up";
pub(crate) const DOWN: &str = "list.down";
/// Focus deck `{"deck": 0..=3}`, and fire cart `{"cart": 0..=11}` of the page on show.
pub(crate) const FOCUS_DECK: &str = "deck.focus";
pub(crate) const FIRE_CART: &str = "cart.fire";
/// The decks and the carts that have a key.
const DECKS: usize = 4;
const CART_KEYS: usize = 12;

/// Deck `i` (A to D) takes the focus.
pub(crate) fn focus_deck(i: usize) -> CommandRef {
    CommandRef::with_params(FOCUS_DECK, json!({ "deck": i }))
}

/// Cart `n` (from 0) of the page on show fires.
pub(crate) fn fire_cart(n: usize) -> CommandRef {
    CommandRef::with_params(FIRE_CART, json!({ "cart": n }))
}

/// `bindable` with `keys` as its default chords. A text that does not parse is left out; the
/// catalog test catches one.
fn keys(bindable: Bindable, keys: &[&str]) -> Bindable {
    keys.iter()
        .filter_map(|k| Binding::parse(k).ok())
        .fold(bindable, Bindable::with_default)
}

/// Every command that has a key, with its menu path and default chords.
pub(crate) fn catalog() -> Vec<Bindable> {
    let entry = |command: CommandRef, label: &str, menu: &str, chords: &[&str]| {
        let path = vec![menu.to_owned(), label.to_owned()];
        keys(
            Bindable::new(command, label, Scope::app()).with_path(path),
            chords,
        )
    };
    let mut list = vec![
        entry(UNDO.into(), "Undo", "Edit", &["Cmd+Z"]),
        entry(REDO.into(), "Redo", "Edit", &["Cmd+Shift+Z"]),
        entry(CUT.into(), "Cut", "Edit", &["Cmd+X"]),
        entry(COPY.into(), "Copy", "Edit", &["Cmd+C"]),
        entry(PASTE.into(), "Paste", "Edit", &["Cmd+V"]),
        entry(DUPLICATE.into(), "Duplicate", "Edit", &["Cmd+D"]),
        entry(DELETE.into(), "Remove", "Edit", &["Delete", "Backspace"]),
        entry(SELECT_ALL.into(), "Select All", "Edit", &["Cmd+A"]),
        entry(PREFERENCES.into(), "Preferences", "Edit", &["Cmd+,"]),
        entry(FIND.into(), "Search Preferences", "Edit", &["Cmd+F"]),
        entry(PLAY_NEXT.into(), "Play Next", "Playout", &["Cmd+Shift+N"]),
        entry(FADE.into(), "Fade Out", "Playout", &["Cmd+Shift+F"]),
        entry(PLAY_PAUSE.into(), "Play or Pause", "Playout", &["Space"]),
        entry(STOP_CARTS.into(), "Stop All Carts", "Playout", &["Escape"]),
        entry(AIR_TOGGLE.into(), "On Air", "Playout", &["Cmd+Shift+O"]),
        entry(UP.into(), "Previous Row", "Lists", &["Up"]),
        entry(DOWN.into(), "Next Row", "Lists", &["Down"]),
    ];
    for (i, name) in ['A', 'B', 'C', 'D'].iter().enumerate().take(DECKS) {
        let chord = format!("Cmd+{}", i + 1);
        list.push(entry(
            focus_deck(i),
            &format!("Focus Deck {name}"),
            "Playout",
            &[&chord],
        ));
    }
    for n in 0..CART_KEYS {
        let chord = format!("F{}", n + 1);
        list.push(entry(
            fire_cart(n),
            &format!("Fire Cart {}", n + 1),
            "Cartwall",
            &[&chord],
        ));
    }
    list
}

/// The keymap with the catalog's defaults, for a context that has none installed.
pub(crate) fn default_keymap() -> Keymap {
    Keymap::new(catalog(), Scopes::new(), Platform::current())
}

fn installed_id() -> egui::Id {
    egui::Id::unique("radio::keymap")
}

/// Uses `keymap` for the keys, menus and tooltips of `ctx`.
pub(crate) fn install(ctx: &egui::Context, keymap: Arc<Keymap>) {
    ctx.data_mut(|d| d.insert_temp(installed_id(), keymap));
}

/// The keymap of `ctx`: the one the app installed (the user's shortcuts), else the defaults.
pub(crate) fn keymap(ctx: &egui::Context) -> Arc<Keymap> {
    static DEFAULTS: OnceLock<Arc<Keymap>> = OnceLock::new();
    ctx.data(|d| d.get_temp::<Arc<Keymap>>(installed_id()))
        .unwrap_or_else(|| DEFAULTS.get_or_init(|| Arc::new(default_keymap())).clone())
}

/// The egui shortcuts of `command`, its single-press bindings in order.
fn shortcuts_of(ctx: &egui::Context, command: &CommandRef) -> Vec<KeyboardShortcut> {
    keymap(ctx)
        .bindings(command)
        .into_iter()
        .filter(|b| !b.trigger.double)
        .map(|b| b.trigger.chord.to_egui_shortcut())
        .collect()
}

/// The text of the command's first shortcut for tooltips and menus, in the style of the
/// platform; empty when it has none.
pub(crate) fn label(ctx: &egui::Context, id: &str) -> String {
    label_of(ctx, &id.into())
}

/// As [`label`], for a command with parameters.
pub(crate) fn label_of(ctx: &egui::Context, command: &CommandRef) -> String {
    shortcuts_of(ctx, command)
        .first()
        .map_or_else(String::new, |s| ctx.format_shortcut(s))
}

/// The text of the key for cart `n` (from 0) on the page on show.
pub(crate) fn cart_label(ctx: &egui::Context, n: usize) -> Option<String> {
    shortcuts_of(ctx, &fire_cart(n))
        .first()
        .map(|s| s.logical_key.name().to_owned())
}

/// The shortcuts that a person pressed in this frame.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct Pressed {
    pub cut: bool,
    pub copy: bool,
    pub paste: bool,
    pub duplicate: bool,
    pub delete: bool,
    pub select_all: bool,
    pub undo: bool,
    pub redo: bool,
    pub play_next: bool,
    pub preferences: bool,
    /// Search Preferences: its search field takes the focus.
    pub find: bool,
    pub fade: bool,
    pub play_pause: bool,
    pub stop_carts: bool,
    /// A key press of the On air shortcut: one press arms it.
    pub air: bool,
    /// The On air shortcut's key came up before its press of this frame (or in a frame
    /// without one), and after it. A held key sends presses with no key-up between them.
    pub air_up_before: bool,
    pub air_up_after: bool,
    /// On Air chosen from a menu with the pointer: a press that no key-up follows.
    pub air_menu: bool,
    pub focus_deck: Option<usize>,
    /// The cart key, as a number from 0 on the page on show.
    pub cart: Option<usize>,
    /// The arrow keys: -1 for up, 1 for down.
    pub step: i32,
}

impl Pressed {
    /// The shortcuts of `self` and of `other`: a menu click joins the keys of its frame.
    pub(crate) fn or(self, other: Pressed) -> Pressed {
        Pressed {
            cut: self.cut | other.cut,
            copy: self.copy | other.copy,
            paste: self.paste | other.paste,
            duplicate: self.duplicate | other.duplicate,
            delete: self.delete | other.delete,
            select_all: self.select_all | other.select_all,
            undo: self.undo | other.undo,
            redo: self.redo | other.redo,
            play_next: self.play_next | other.play_next,
            preferences: self.preferences | other.preferences,
            find: self.find | other.find,
            fade: self.fade | other.fade,
            play_pause: self.play_pause | other.play_pause,
            stop_carts: self.stop_carts | other.stop_carts,
            air: self.air | other.air,
            air_up_before: self.air_up_before | other.air_up_before,
            air_up_after: self.air_up_after | other.air_up_after,
            air_menu: self.air_menu | other.air_menu,
            focus_deck: self.focus_deck.or(other.focus_deck),
            cart: self.cart.or(other.cart),
            step: if self.step == 0 {
                other.step
            } else {
                self.step
            },
        }
    }
}

/// Read the shortcuts of this frame, once. A focused text field keeps all keys for itself.
/// Where the On air shortcut's key came up in this frame's events: before its first press
/// (or anywhere, when it was not pressed) and after its last press. It reads the order of the
/// events, not egui's repeat flag, which a press from the native menu never sets.
fn air_ups(ctx: &egui::Context) -> (bool, bool) {
    let shortcuts = shortcuts_of(ctx, &AIR_TOGGLE.into());
    ctx.input(|i| {
        let presses: Vec<usize> = i
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                shortcuts.iter().any(|s| {
                    matches!(e, Event::Key { key, modifiers, pressed: true, .. }
                        if *key == s.logical_key && modifiers.matches_logically(s.modifiers))
                })
            })
            .map(|(n, _)| n)
            .collect();
        let ups: Vec<usize> = i
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                shortcuts.iter().any(|s| {
                    matches!(e, Event::Key { key, pressed: false, .. } if *key == s.logical_key)
                })
            })
            .map(|(n, _)| n)
            .collect();
        match (presses.first(), presses.last()) {
            (Some(first), Some(last)) => {
                (ups.iter().any(|u| u < first), ups.iter().any(|u| u > last))
            }
            _ => (!ups.is_empty(), false),
        }
    })
}

pub(crate) fn read(ctx: &egui::Context) -> Pressed {
    // A key-up counts whoever else holds the keys, so a held switch is never stuck.
    let (air_up_before, air_up_after) = air_ups(ctx);
    let ups = Pressed {
        air_up_before: air_up_before || air_up_after,
        ..Pressed::default()
    };
    if ctx.egui_wants_keyboard_input() {
        return ups;
    }
    // The in-window menu bar holds the keys while a menu is open or it has the keyboard (an
    // Alt tap); a command shortcut takes the keyboard off it first and runs.
    craft_menu::bar::leave_for_shortcut(ctx);
    if craft_menu::bar::is_open(ctx) || craft_menu::bar::is_focused(ctx) {
        return ups;
    }
    // Space and the arrows belong to a focused button, slider or list, when there is one.
    let free = ctx.memory(|m| m.focused().is_none());
    // Escape closes an open menu first, then the one-off dialog.
    // It also closes an open settings page under the pointer or the focus.
    let menu = egui::Popup::is_any_open(ctx)
        | crate::settings::escape_claimed(ctx)
        | crate::one_off::escape_claimed(ctx)
        | crate::chrome::escape_claimed(ctx);
    let map = keymap(ctx);
    let take = |i: &mut egui::InputState, command: &CommandRef| {
        map.bindings(command)
            .into_iter()
            .filter(|b| !b.trigger.double)
            .map(|b| b.trigger.chord.to_egui_shortcut())
            .collect::<Vec<_>>()
            .iter()
            .fold(false, |hit, s| i.consume_shortcut(s) | hit)
    };
    ctx.input_mut(|i| {
        let mut p = Pressed {
            air_up_before,
            air_up_after,
            ..Pressed::default()
        };
        for e in &i.events {
            match e {
                Event::Cut => p.cut = true,
                Event::Copy => p.copy = true,
                Event::Paste(_) => p.paste = true,
                _ => {}
            }
        }
        // Read the longer shortcuts first, so Command+Shift+Z is not also Command+Z.
        p.redo = take(i, &REDO.into());
        p.undo = take(i, &UNDO.into());
        p.play_next = take(i, &PLAY_NEXT.into());
        p.preferences = take(i, &PREFERENCES.into());
        p.find = take(i, &FIND.into());
        p.fade = take(i, &FADE.into());
        p.air = take(i, &AIR_TOGGLE.into());
        p.duplicate = take(i, &DUPLICATE.into());
        p.delete = take(i, &DELETE.into());
        p.select_all = take(i, &SELECT_ALL.into());
        p.play_pause = free && take(i, &PLAY_PAUSE.into());
        p.stop_carts = !menu && take(i, &STOP_CARTS.into());
        p.focus_deck = (0..DECKS).position(|d| take(i, &focus_deck(d)));
        p.cart = (0..CART_KEYS).position(|n| take(i, &fire_cart(n)));
        if free && take(i, &UP.into()) {
            p.step = -1;
        } else if free && take(i, &DOWN.into()) {
            p.step = 1;
        }
        p
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first shortcut of the command `id`.
    fn shortcut(id: &str) -> Option<KeyboardShortcut> {
        shortcuts_of(&egui::Context::default(), &id.into())
            .into_iter()
            .next()
    }

    #[test]
    fn every_command_has_the_chords_it_had() {
        // The chords the app had before the keymap, on the command modifier of the platform.
        for (id, text) in [
            (UNDO, "Cmd+Z"),
            (REDO, "Cmd+Shift+Z"),
            (PLAY_NEXT, "Cmd+Shift+N"),
            (FADE, "Cmd+Shift+F"),
            (PREFERENCES, "Cmd+,"),
            (FIND, "Cmd+F"),
            (PLAY_PAUSE, "Space"),
            (STOP_CARTS, "Escape"),
            (AIR_TOGGLE, "Cmd+Shift+O"),
            (SELECT_ALL, "Cmd+A"),
        ] {
            let want = craft_keymap::Chord::parse(text)
                .map(|c| c.to_egui_shortcut())
                .ok();
            assert_eq!(shortcut(id), want, "{id}");
        }
        let ctx = egui::Context::default();
        assert_eq!(
            shortcuts_of(&ctx, &DELETE.into()).len(),
            2,
            "Delete and Backspace"
        );
        assert_eq!(cart_label(&ctx, 0).as_deref(), Some("F1"));
        assert_eq!(cart_label(&ctx, 11).as_deref(), Some("F12"));
        assert!(cart_label(&ctx, 12).is_none());
    }

    #[test]
    fn the_catalog_parses_and_has_no_clashes() {
        let map = default_keymap();
        assert!(map.problems().is_empty(), "{:?}", map.problems());
        assert!(map.conflicts().is_empty(), "{:?}", map.conflicts());
    }
}
