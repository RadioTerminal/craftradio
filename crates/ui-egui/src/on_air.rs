//! The On air switch: a large button above the output strips, the Playout menu item and its
//! shortcut. Each switch needs a second press within the arm time, both ways, as a cut of the
//! item on air does. The deck preference that turns that second press off does not apply here:
//! going off air is never one press. A `settings.set` from the CLI, MCP or a script switches
//! at once.

use egui::{Align2, Role, Sense, Stroke, StrokeKind, WidgetInfo};

use crate::app::Action;
use crate::parts;
use crate::shortcuts;
use crate::theme::{self, Tokens, size, space};
use crate::view::StationView;
use crate::widgets;

/// What the first press arms, and what the button says until the second press.
pub(crate) fn arm_note(on_air: bool) -> &'static str {
    if on_air {
        "Press again to go off air"
    } else {
        "Press again to go on air"
    }
}

/// The armed switch, kept across frames by the app, so the button, the menu and the key share
/// one arm.
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

    /// The part of the arm time that is left, while the switch is armed for the way it can go.
    fn left(&mut self, on_air: bool, now: f64) -> Option<f32> {
        let (to, at) = self.armed?;
        let left = 1.0 - (now - at) / size::ARM_S;
        if to == !on_air && left > 0.0 {
            Some(left as f32)
        } else {
            self.armed = None;
            None
        }
    }
}

/// What the button shows: its word, the tooltip and the accessible name.
struct Words {
    word: String,
    name: String,
    hint: String,
}

fn words(view: &StationView, armed: bool, keys: &str) -> Words {
    let preview = view.say(" · preview", "");
    let (word, go) = if view.on_air && view.going_on_air {
        ("Going on air…", "go off air")
    } else if view.on_air {
        ("On air", "go off air")
    } else {
        ("Off air", "go on air")
    };
    let why = if view.preview {
        "UI preview: the switch is kept, but no audio goes out"
    } else if view.on_air && view.going_on_air {
        "On air: Icecast is still connecting"
    } else if view.on_air {
        "On air: Icecast and WebRTC send Program to listeners"
    } else {
        "Off air: nothing goes to listeners, and the decks play on here"
    };
    let hint = format!("{why}. Press twice to {go} ({keys})");
    if armed {
        let note = arm_note(view.on_air).to_owned();
        return Words {
            word: note.clone(),
            name: note,
            hint,
        };
    }
    let word = format!("{word}{preview}");
    Words {
        name: format!("{word}: press twice to {go}"),
        word,
        hint,
    }
}

/// The On air button, at least `width` wide and one height in every state. On air, its dot
/// pulses slowly in the on-air colour; off air, it is still, in the warning colour, and says
/// Off air. While Icecast connects after going on air, a ring runs around the dot.
pub(crate) fn button(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    width: f32,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let now = ui.input(|i| i.time);
    let left = state.left(view.on_air, now);
    let keys = shortcuts::label(ui.ctx(), shortcuts::AIR_TOGGLE);
    let w = words(view, left.is_some(), &keys);
    let font = theme::text(theme::LEAD);
    // The widest word, so the button keeps its width whatever it says.
    let widest = [
        arm_note(true),
        arm_note(false),
        "Going on air… · preview",
        "Off air · preview",
    ]
    .iter()
    .map(|s| {
        ui.painter()
            .layout_no_wrap((*s).to_owned(), font.clone(), t.text)
            .size()
            .x
    })
    .fold(0.0, f32::max);
    let dot_room = size::ON_AIR_DOT * 2.0 + space::M;
    let w_min = space::L * 2.0 + dot_room + widest;
    let size = egui::vec2(width.max(w_min), size::ON_AIR_BUTTON);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &w.name));
    let resp = resp.on_hover_text(&w.hint);
    // A held Enter or Space repeats and clicks again; that is not a second press.
    let held = ui.input(|i| {
        let key = |repeat: bool| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Enter | egui::Key::Space,
                        pressed: true,
                        repeat: r,
                        ..
                    } if *r == repeat
                )
            })
        };
        key(true) && !key(false)
    });
    if resp.clicked() && !held {
        resp.request_focus();
        if let Some(a) = state.press(view.on_air, now) {
            actions.push(a);
        }
    }
    // Switching crossfades between the two looks.
    let k = ui.ctx().animate_bool(resp.id.with("on_air"), view.on_air);
    let going = view.on_air && view.going_on_air;
    let live = view.on_air && !going && !view.preview;
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let (fill, edge, dot) = if left.is_some() {
            (t.danger_fill, t.danger, t.danger)
        } else if going {
            // Not live yet, so not in the on-air colour.
            (t.warn_fill, t.warn, t.warn)
        } else {
            let lerp = |off: egui::Color32, on: egui::Color32| off.lerp_to_gamma(on, k);
            // The preview sends nothing, so its On air never takes the live colours (rule 7).
            let (on_fill, on_edge, on_dot) = if view.preview {
                (t.field, t.field_border, t.text_muted)
            } else {
                (t.on_air_bg, t.on_air, t.on_air)
            };
            (
                lerp(t.warn_fill, on_fill),
                lerp(t.warn, on_edge),
                lerp(t.warn, on_dot),
            )
        };
        let edge = if resp.has_focus() { t.accent_ink } else { edge };
        p.rect(
            rect,
            t.radius,
            fill,
            Stroke::new(size::ON_AIR_EDGE, edge),
            StrokeKind::Inside,
        );
        let centre = egui::pos2(rect.left() + space::L + size::ON_AIR_DOT, rect.center().y);
        let r = size::ON_AIR_DOT;
        let stroke = Stroke::new(size::ON_AIR_EDGE, dot);
        if going {
            // Going on air: a ring that runs until Icecast says connected.
            let turn = (now / f64::from(size::SPIN_S)).fract() as f32;
            let from = std::f32::consts::TAU * turn;
            let sweep = std::f32::consts::TAU * size::SPIN_SWEEP;
            widgets::ring(
                p,
                centre,
                r,
                from,
                sweep,
                Stroke::new(size::ON_AIR_EDGE, t.warn),
            );
        } else {
            // The light: hollow off air, filled on air. On air its fill pulses slowly.
            p.circle_stroke(centre, r, stroke);
            let pulse = if live {
                let phase = (now / f64::from(size::PULSE_S)).fract() as f32;
                let wave = 0.5 + 0.5 * (std::f32::consts::TAU * phase).cos();
                size::PULSE_LOW + (1.0 - size::PULSE_LOW) * wave
            } else {
                1.0
            };
            p.circle_filled(centre, r, dot.gamma_multiply(k * pulse));
        }
        if let Some(left) = left {
            // The time left to press again: a ring around the light that runs out.
            let sweep = std::f32::consts::TAU * left.clamp(0.0, 1.0);
            widgets::ring(
                p,
                centre,
                r + space::XS,
                -std::f32::consts::FRAC_PI_2,
                sweep,
                Stroke::new(size::ON_AIR_EDGE, t.danger),
            );
        }
        let at = egui::pos2(centre.x + r + space::M, rect.center().y);
        let ink = if enabled { t.text } else { t.text_muted };
        p.text(at, Align2::LEFT_CENTER, &w.word, font, ink);
        // Only this button asks for frames: while its light pulses, its ring runs or it is armed.
        if live || going || left.is_some() {
            ui.ctx().request_repaint_after_secs(size::ARM_REPAINT_S);
        }
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
