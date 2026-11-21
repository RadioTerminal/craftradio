//! The rules editor: a floating window that edits a copy of one playlist's rules.
//!
//! Rules are JavaScript. The Visual tab writes the script from dropdown rows. The Script tab
//! shows the script, and a person can edit it by hand. Apply stores the copy in the view source.
//! Nothing reaches a scheduler yet, and the window says so.

use egui::{Align, ComboBox, DragValue, Layout, RichText, Stroke, TextEdit};

use crate::rules_script;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{
    Constraint, ConstraintKind, Pick, Placement, PlaylistTab, RuleView, Trigger, ViewSource,
};

const NOT_APPLIED: &str = "preview — rules are not applied to the scheduler yet";
const SCRIPT_NOTE: &str = "Script edits take effect when the script engine lands. \
Visual view shows the last generated rules.";
const BY_HAND: &str = "edited by hand — visual editing would overwrite it";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Visual,
    Script,
}

/// An open editor and its working copy.
pub(crate) struct Editor {
    draft: PlaylistTab,
    mode: Mode,
    applied: bool,
}

impl Editor {
    pub fn new(tab: &PlaylistTab) -> Self {
        Self {
            draft: tab.clone(),
            mode: Mode::Visual,
            applied: false,
        }
    }
}

/// Show the editor while it is open. It returns true after Apply.
pub(crate) fn show(
    ctx: &egui::Context,
    slot: &mut Option<Editor>,
    source: &mut dyn ViewSource,
    t: &Tokens,
) -> bool {
    let Some(editor) = slot.as_mut() else {
        return false;
    };
    let mut open = true;
    let mut close = false;
    let mut applied = false;
    egui::Window::new(format!("{} — rules", editor.draft.name))
        .id(egui::Id::new("rules_editor"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([size::EDITOR_W, size::EDITOR_H])
        .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
        .show(ctx, |ui| {
            mode_bar(ui, editor);
            ui.add_space(space::S);
            match editor.mode {
                Mode::Visual => visual(ui, &mut editor.draft, t),
                Mode::Script => script(ui, &mut editor.draft, t),
            }
            ui.add_space(space::M);
            ui.separator();
            ui.horizontal(|ui| {
                let apply = egui::Button::new(RichText::new("Apply").color(t.canvas))
                    .fill(t.accent)
                    .min_size(egui::vec2(0.0, size::CONTROL_MD));
                if ui.add(apply).clicked() {
                    applied = source.apply_rules(editor.draft.clone());
                    editor.applied = true;
                }
                let cancel =
                    egui::Button::new("Cancel").min_size(egui::vec2(0.0, size::CONTROL_MD));
                if ui.add(cancel).clicked() {
                    close = true;
                }
                if editor.applied {
                    // Apply only stores the copy. The note must never read as success.
                    let note = RichText::new(NOT_APPLIED).font(theme::text(theme::LABEL));
                    ui.label(note.color(t.warn));
                }
            });
        });
    if close || !open {
        *slot = None;
    }
    applied
}

/// The ⚙ button that opens the rules editor for `name`. It returns true on a click.
pub(crate) fn cog(ui: &mut egui::Ui, name: &str) -> bool {
    let icon = RichText::new("⚙").font(theme::text(theme::CARD));
    let resp = ui.button(icon);
    let label = format!("⚙ {name} rules");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    resp.on_hover_text(format!("{name} — rules")).clicked()
}

fn mode_bar(ui: &mut egui::Ui, editor: &mut Editor) {
    ui.horizontal(|ui| {
        for (mode, label) in [(Mode::Visual, "Visual"), (Mode::Script, "Script")] {
            if ui.selectable_label(editor.mode == mode, label).clicked() {
                editor.mode = mode;
            }
        }
    });
}

fn caption(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.label(
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
}

fn visual(ui: &mut egui::Ui, tab: &mut PlaylistTab, t: &Tokens) {
    if tab.script_by_hand {
        egui::Frame::new()
            .fill(t.field)
            .stroke(Stroke::new(size::HAIRLINE, t.warn))
            .corner_radius(t.radius)
            .inner_margin(pad::ROW)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(BY_HAND).color(t.warn));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Overwrite").clicked() {
                            tab.script_by_hand = false;
                            rules_script::refresh(tab);
                        }
                    });
                });
            });
        ui.add_space(space::S);
    }
    let mut changed = false;
    ui.add_enabled_ui(!tab.script_by_hand, |ui| {
        ui.horizontal(|ui| {
            ui.label("Pick order");
            ComboBox::from_id_salt("pick_order")
                .width(size::COMBO_W)
                .selected_text(tab.order.label())
                .show_ui(ui, |ui| {
                    for p in Pick::ALL {
                        changed |= ui.selectable_value(&mut tab.order, p, p.label()).changed();
                    }
                });
        });
        ui.add_space(space::S);
        caption(ui, "RULES · trigger · placement · take · constraints", t);
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("rules_rows")
            .max_height(size::EDITOR_ROWS_H)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (i, rule) in tab.visual.iter_mut().enumerate() {
                    egui::Frame::new()
                        .fill(t.field)
                        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
                        .corner_radius(t.radius)
                        .inner_margin(pad::ROW)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            changed |= rule_row(ui, i, rule);
                            if ui.button(format!("Delete rule {}", i + 1)).clicked() {
                                remove = Some(i);
                            }
                        });
                    ui.add_space(space::XS);
                }
            });
        if let Some(i) = remove {
            tab.visual.remove(i);
            changed = true;
        }
        if ui.button("+ Add rule").clicked() {
            tab.visual.push(RuleView::new());
            changed = true;
        }
    });
    if changed {
        rules_script::refresh(tab);
    }
}

/// One rule as a row of dropdowns and number fields. It returns true on a change.
fn rule_row(ui: &mut egui::Ui, i: usize, rule: &mut RuleView) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ComboBox::from_id_salt(("trigger", i))
            .width(size::COMBO_W)
            .selected_text(rule.trigger.label())
            .show_ui(ui, |ui| {
                for v in Trigger::ALL {
                    changed |= ui
                        .selectable_value(&mut rule.trigger, v, v.label())
                        .changed();
                }
            });
        match rule.trigger {
            Trigger::EveryTracks => {
                changed |= ui
                    .add(DragValue::new(&mut rule.tracks).range(1..=20))
                    .changed();
            }
            Trigger::AtMinutes => {
                for m in &mut rule.minutes {
                    let field = DragValue::new(m).range(0..=59).prefix(":");
                    changed |= ui.add(field).changed();
                }
                if rule.minutes.len() > 1
                    && ui
                        .small_button("−")
                        .on_hover_text("Remove minute")
                        .clicked()
                {
                    rule.minutes.pop();
                    changed = true;
                }
                if ui.small_button("+").on_hover_text("Add minute").clicked() {
                    rule.minutes
                        .push(rule.minutes.last().map_or(0, |m| (m + 30) % 60));
                    changed = true;
                }
            }
            Trigger::Daily => {
                changed |= ui
                    .add(DragValue::new(&mut rule.time.0).range(0..=23))
                    .changed();
                ui.label(":");
                changed |= ui
                    .add(DragValue::new(&mut rule.time.1).range(0..=59))
                    .changed();
            }
        }
        ui.label("·");
        ComboBox::from_id_salt(("place", i))
            .width(size::COMBO_W)
            .selected_text(rule.place.label())
            .show_ui(ui, |ui| {
                for v in Placement::ALL {
                    changed |= ui.selectable_value(&mut rule.place, v, v.label()).changed();
                }
            });
        if rule.place.is_fixed() {
            let tol = DragValue::new(&mut rule.tolerance_s)
                .range(0.0..=10.0)
                .speed(0.1)
                .prefix("± ")
                .suffix(" s");
            changed |= ui.add(tol).changed();
        }
        ui.label("· take");
        changed |= ui
            .add(DragValue::new(&mut rule.take).range(1..=10))
            .changed();
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("constraints");
        let mut drop = None;
        for (k, c) in rule.constraints.iter_mut().enumerate() {
            changed |= constraint(ui, (i, k), c);
            if ui
                .small_button("✖")
                .on_hover_text("Remove constraint")
                .clicked()
            {
                drop = Some(k);
            }
        }
        if let Some(k) = drop {
            rule.constraints.remove(k);
            changed = true;
        }
        if ui.small_button("+ constraint").clicked() {
            rule.constraints.push(Constraint {
                kind: ConstraintKind::NotAdjacentToSelf,
                n: 1,
            });
            changed = true;
        }
    });
    changed
}

fn constraint(ui: &mut egui::Ui, id: (usize, usize), c: &mut Constraint) -> bool {
    let mut changed = false;
    ComboBox::from_id_salt(("constraint", id))
        .width(size::COMBO_W)
        .selected_text(c.kind.label())
        .show_ui(ui, |ui| {
            for v in ConstraintKind::ALL {
                changed |= ui.selectable_value(&mut c.kind, v, v.label()).changed();
            }
        });
    if c.kind.has_value() {
        changed |= ui.add(DragValue::new(&mut c.n).range(1..=99)).changed();
    }
    changed
}

fn script(ui: &mut egui::Ui, tab: &mut PlaylistTab, t: &Tokens) {
    let edit = TextEdit::multiline(&mut tab.script)
        .font(theme::mono(theme::CODE))
        .code_editor()
        .desired_rows(size::SCRIPT_ROWS)
        .desired_width(f32::INFINITY);
    if ui.add(edit).changed() {
        tab.script_by_hand = true;
    }
    ui.add_space(space::XS);
    caption(ui, SCRIPT_NOTE, t);
}
