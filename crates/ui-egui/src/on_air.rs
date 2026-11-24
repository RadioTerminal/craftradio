//! The On air switch: the Playout menu item and its shortcut. Each switch needs a second
//! press within the arm time, both ways, as a cut of the item on air does. The deck preference that turns that second press off does not apply here:
//! going off air is never one press. A `settings.set` from the CLI, MCP or a script switches
//! at once.

use crate::app::Action;
use crate::parts;
use crate::theme::size;

/// What the first press arms, and what the toast says until the second press.
pub(crate) fn arm_note(on_air: bool) -> &'static str {
    if on_air {
        "Press again to go off air"
    } else {
        "Press again to go on air"
    }
}

/// The armed switch, kept across frames by the app, so the menu and the key share one arm.
#[derive(Default)]
pub(crate) struct State {
    /// The state a second press switches to, and the egui time of the first press.
    armed: Option<(bool, f64)>,
    /// While the shortcut's key is down: the egui time of its last press. A press before its
    /// key comes up is the key held down (AppKit sends one on each autorepeat), not a second
    /// press. Without a key-up it lapses after the arm time, so a lost key-up never blocks
    /// the key for long.
    key_down: Option<f64>,
}

impl State {
    /// A press of the switch while the station is `on_air`. The first press arms and returns
    /// nothing. A second press within the arm time returns the action that switches. An arm
    /// for the other way (the station switched meanwhile) or one that ran out arms again.
    pub(crate) fn press(&mut self, on_air: bool, now: f64) -> Option<Action> {
        let to = !on_air;
        let again = self
            .armed
            .is_some_and(|(t, at)| t == to && now - at <= size::ARM_S);
        if again {
            self.armed = None;
            Some(parts::set(
                radio_model::AIR_PART,
                radio_model::ON_AIR_KEY,
                to,
            ))
        } else {
            self.armed = Some((to, now));
            None
        }
    }

    /// A key press of the shortcut. It returns `None` while the key is held down: such a press
    /// neither arms nor switches. Otherwise it returns what [`State::press`] returns.
    pub(crate) fn press_key(&mut self, on_air: bool, now: f64) -> Option<Option<Action>> {
        let held = self.key_down.is_some_and(|at| now - at <= size::ARM_S);
        self.key_down = Some(now);
        if held {
            return None;
        }
        Some(self.press(on_air, now))
    }

    /// The shortcut's key came up.
    pub(crate) fn key_up(&mut self) {
        self.key_down = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arm_for_the_other_way_does_not_switch() {
        let mut s = State::default();
        assert!(s.press(true, 0.0).is_none(), "armed to go off air");
        // The station went off air from the CLI meanwhile: the press arms to go on air.
        assert!(s.press(false, 1.0).is_none());
        assert!(
            s.press(false, 2.0).is_some(),
            "now the second press switches"
        );
    }
}
