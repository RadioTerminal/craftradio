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
        if let Some(n) = p.cart
            && let Some(name) = self.carts.fire_on_page(&self.view, n, now)
            && !self.open.get(ToolWindow::Cartwall)
        {
            // With the cartwall closed, the toast says which cart the key fired.
            actions.push(Action::Toast(format!("{name} · {}", cartwall::NO_AUDIO)));
        }
        if p.stop_carts {
            self.carts.stop_all();
        }
        if let Some(d) = p.focus_deck {
            self.decks.focus(d);
        }
        let deck = self.decks.focused();
        let Some(dv) = self.view.decks.get(deck) else {
            return;
        };
        if dv.state == DeckState::Empty {
            return;
        }
        if p.play_pause {
            actions.push(Action::Edit(Edit::Deck {
                deck,
                command: DeckCommand::PlayPause,
            }));
        }
        // The keys go through the same guard as the buttons.
        let live = decks::is_live(dv);
        for (pressed, kind) in [
            (p.skip, decks::Guarded::Skip),
            (p.fade, decks::Guarded::Fade),
        ] {
            if pressed && let Some(a) = self.decks.press(deck, kind, live, now) {
                actions.push(a);
            }
        }
    }
}
