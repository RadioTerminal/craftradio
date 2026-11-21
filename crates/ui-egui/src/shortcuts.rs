//! The key map: every keyboard shortcut of the app in one place, so a later version can make it
//! configurable. No shortcut fires while a text field has the keyboard.

use egui::{Event, Key, KeyboardShortcut, Modifiers};

/// Command plus Shift: on-air actions need two keys, so a stray press does not fire them.
const CMD_SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: true,
};

pub(crate) const CUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::X);
pub(crate) const COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::C);
pub(crate) const PASTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::V);
pub(crate) const DUPLICATE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::D);
/// Remove the selected rows: Delete (Fn+Delete on a Mac) or Backspace (the Mac delete key).
pub(crate) const DELETE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Delete);
pub(crate) const BACKSPACE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::NONE, Key::Backspace);
/// Select every row of the focused list.
pub(crate) const SELECT_ALL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::A);
pub(crate) const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
pub(crate) const REDO: KeyboardShortcut = KeyboardShortcut::new(CMD_SHIFT, Key::Z);
/// Play next starts the NEXT 1 deck. Fade out fades the item on air. Both arm first when
/// they cut the item on air.
pub(crate) const PLAY_NEXT: KeyboardShortcut = KeyboardShortcut::new(CMD_SHIFT, Key::N);
pub(crate) const FADE: KeyboardShortcut = KeyboardShortcut::new(CMD_SHIFT, Key::F);
/// Space plays the focused deck, or pauses it when it is on air.
pub(crate) const PLAY_PAUSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Space);
pub(crate) const STOP_CARTS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Escape);
/// Focus deck A to D.
pub(crate) const FOCUS_DECK: [KeyboardShortcut; 4] = [
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Num1),
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Num2),
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Num3),
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Num4),
];
/// Fire carts 1 to 12 of the page on show.
pub(crate) const CARTS: [Key; 12] = [
    Key::F1,
    Key::F2,
    Key::F3,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
    Key::F10,
    Key::F11,
    Key::F12,
];
pub(crate) const UP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowUp);
pub(crate) const DOWN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowDown);

/// The text of a shortcut for tooltips and menus, in the style of the platform.
pub(crate) fn label(ctx: &egui::Context, shortcut: &KeyboardShortcut) -> String {
    ctx.format_shortcut(shortcut)
}

/// The text of the key for cart `n` (from 0) on the page on show.
pub(crate) fn cart_label(n: usize) -> Option<&'static str> {
    CARTS.get(n).map(|k| k.name())
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
    pub fade: bool,
    pub play_pause: bool,
    pub stop_carts: bool,
    pub focus_deck: Option<usize>,
    /// The cart key, as a number from 0 on the page on show.
    pub cart: Option<usize>,
    /// The arrow keys: -1 for up, 1 for down.
    pub step: i32,
}

/// Read the shortcuts of this frame, once. A focused text field keeps all keys for itself.
pub(crate) fn read(ctx: &egui::Context) -> Pressed {
    if ctx.egui_wants_keyboard_input() {
        return Pressed::default();
    }
    // Space and the arrows belong to a focused button, slider or list, when there is one.
    let free = ctx.memory(|m| m.focused().is_none());
    // Escape closes an open menu first, then the one-off dialog.
    // It also closes an open settings page under the pointer or the focus.
    let menu = egui::Popup::is_any_open(ctx)
        | crate::settings::escape_claimed(ctx)
        | crate::one_off::escape_claimed(ctx);
    ctx.input_mut(|i| {
        let mut p = Pressed::default();
        for e in &i.events {
            match e {
                Event::Cut => p.cut = true,
                Event::Copy => p.copy = true,
                Event::Paste(_) => p.paste = true,
                _ => {}
            }
        }
        // Read the longer shortcuts first, so Command+Shift+Z is not also Command+Z.
        p.redo = i.consume_shortcut(&REDO);
        p.undo = i.consume_shortcut(&UNDO);
        p.play_next = i.consume_shortcut(&PLAY_NEXT);
        p.fade = i.consume_shortcut(&FADE);
        p.duplicate = i.consume_shortcut(&DUPLICATE);
        p.delete = i.consume_shortcut(&DELETE) | i.consume_shortcut(&BACKSPACE);
        p.select_all = i.consume_shortcut(&SELECT_ALL);
        p.play_pause = free && i.consume_shortcut(&PLAY_PAUSE);
        p.stop_carts = !menu && i.consume_shortcut(&STOP_CARTS);
        p.focus_deck = FOCUS_DECK.iter().position(|s| i.consume_shortcut(s));
        p.cart = CARTS
            .iter()
            .position(|k| i.consume_key(Modifiers::NONE, *k));
        if free && i.consume_shortcut(&UP) {
            p.step = -1;
        } else if free && i.consume_shortcut(&DOWN) {
            p.step = 1;
        }
        p
    })
}
