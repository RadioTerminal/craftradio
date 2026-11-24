//! The shortcuts that act on the whole station: decks, carts and undo.
//! The panes handle the shortcuts of their own selection.

use super::*;
use crate::view::{DeckCommand, DeckState};

impl RadioApp {
    pub(super) fn global_keys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let p = self.link.keys;
        let now = ctx.input(|i| i.time);
        if p.undo {
            actions.push(Action::Edit(Edit::Undo));
        }
        if p.redo {
            actions.push(Action::Edit(Edit::Redo));
        }
        if p.preferences {
            self.open.set(ToolWindow::Preferences, true);
        }
        if let Some(n) = p.cart
            && let Some((cart, name)) = self.carts.fire_on_page(&self.view, n, now)
        {
            actions.push(Action::EditQuiet(Edit::FireCart { cart }));
            // With the cartwall closed, the toast says which cart the key fired.
            if !self.open.get(ToolWindow::Cartwall) {
                let audio = self.view.say(cartwall::NO_AUDIO, "fired");
                actions.push(Action::Toast(format!("{name} · {audio}")));
            }
        }
        if p.stop_carts {
            self.carts.stop_all();
            actions.push(Action::EditQuiet(Edit::StopCarts));
            // Escape also clears the selection of the lists.
            self.link.selected.clear();
        }
        if let Some(d) = p.focus_deck {
            self.decks.focus(d);
            // The Main list shows the row of that deck too.
            if let Some(r) = self.view.rows.iter().find(|r| r.deck == Some(d)) {
                self.link.follow = false;
                self.link.scroll_main = Some(r.id);
            }
        }
        // Play next and Fade out act on the station, whatever deck has the focus.
        if p.play_next
            && let Some(next) = self.decks.next_deck()
        {
            let cut = decks::cuts_on_air(&self.view.decks, next, self.decks.fade_in(next));
            if let Some(a) = self.decks.press(next, decks::Guarded::PlayNext, cut, now) {
                actions.push(a);
            }
        }
        if p.fade
            && let Some(air) = decks::on_air_deck(&self.view.decks)
            && let Some(a) = self.decks.press(air, decks::Guarded::Fade, true, now)
        {
            actions.push(a);
        }
        // The On air switch: the first press arms and says so in a toast. A key press
        // counts only after the key came up since the last one. A menu click with the
        // shortcut's modifiers held is that key press (the native menu's key equivalent while a
        // text field has the keys); a click without them is the pointer, and always counts.
        let on_air = self.view.on_air;
        if p.air_up_before {
            self.air.key_up();
        }
        let by_key = p.air || (p.air_menu && shortcuts::air_chord_held(ctx));
        let pressed = if by_key {
            self.air.press_key(on_air, now)
        } else {
            p.air_menu.then(|| self.air.press(on_air, now))
        };
        match pressed {
            Some(Some(a)) => actions.push(a),
            Some(None) => actions.push(Action::Toast(crate::on_air::arm_note(on_air).to_owned())),
            None => {}
        }
        if p.air_up_after {
            self.air.key_up();
        }
        let deck = self.decks.focused();
        let Some(dv) = self.view.decks.get(deck) else {
            return;
        };
        if dv.state == DeckState::Empty || !p.play_pause {
            return;
        }
        // Space pauses the deck on air. On another deck it is Play this deck, with its guard.
        if decks::is_live(dv) {
            actions.push(Action::Edit(Edit::Deck {
                deck,
                command: DeckCommand::PlayPause,
            }));
        } else {
            let cut = decks::cuts_on_air(&self.view.decks, deck, self.decks.fade_in(deck));
            if let Some(a) = self.decks.press(deck, decks::Guarded::Play, cut, now) {
                actions.push(a);
            }
        }
    }
}
