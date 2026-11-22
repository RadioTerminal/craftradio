//! The rules editor: a floating window that edits a copy of one playlist's rules.
//!
//! Rules are JavaScript. The Visual tab writes the script from dropdown rows. The Script tab
//! shows the script, and a person can edit it by hand. Apply stores the copy in the view source.
//! Nothing reaches a scheduler yet, and the window says so.

use egui::{Align, ComboBox, DragValue, Layout, RichText, Stroke, TextEdit};
use egui_extras::{Column, TableBuilder};

use crate::app::{FileTarget, Linked, mmss};
use crate::icons::{self, Icon};
use crate::rules_script;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{Constraint, ConstraintKind, Pick, Placement, PlaylistTab, RuleView, Trigger};
use crate::widgets;

const NOT_APPLIED: &str = "preview — rules are not applied to the scheduler yet";
const SCRIPT_NOTE: &str = "Scripts are stored but not run in this build. \
The Visual rules are what plays.";
/// The largest fit tolerance of a fixed rule, in seconds.
const MAX_TOLERANCE_S: f32 = 10.0;
const WATCH_NOTE: &str = "watching starts with the library engine";
const BY_HAND: &str = "edited by hand — visual editing would overwrite it";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Visual,
    Script,
    Items,
}

/// An open editor and its working copy.
pub(crate) struct Editor {
    draft: PlaylistTab,
    mode: Mode,
    applied: bool,
    /// The source is the preview, which applies no rules.
    pub(crate) preview: bool,
}

impl Editor {
    pub fn new(tab: &PlaylistTab) -> Self {
        Self {
            draft: tab.clone(),
            mode: Mode::Visual,
            applied: false,
            preview: true,
        }
    }

    pub fn name(&self) -> &str {
        &self.draft.name
    }

    /// Add dropped files to the working copy. Their length is not known yet.
    /// Add items to the working copy. The source has read them already.
    pub fn add_items(&mut self, items: Vec<(String, std::time::Duration)>) {
        self.draft
            .weights
            .extend(std::iter::repeat_n(1, items.len()));
        self.draft.items.extend(items);
        self.mode = Mode::Items;
    }
}

/// Show the editor while it is open. After Apply, it returns the working copy to store.
pub(crate) fn show(
    ctx: &egui::Context,
    slot: &mut Option<Editor>,
    link: &mut Linked,
    t: &Tokens,
) -> Option<PlaylistTab> {
    let editor = slot.as_mut()?;
    let mut open = true;
    let mut close = false;
    let mut applied = None;
    egui::Window::new(format!("{} — rules", editor.draft.name))
        .id(egui::Id::unique("rules_editor"))
        .open(&mut open)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .resizable(true)
        .default_size([size::EDITOR_W, size::EDITOR_H])
        .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
        .show(ctx, |ui| {
            mode_bar(ui, editor);
            ui.add_space(space::S);
            match editor.mode {
                Mode::Visual => visual(ui, &mut editor.draft, t),
                Mode::Script => script(ui, &mut editor.draft, editor.preview, t),
                Mode::Items => items(ui, &mut editor.draft, link, editor.preview, t),
            }
            ui.add_space(space::M);
            ui.separator();
            ui.horizontal(|ui| {
                // Scripts do not run yet, so the Script tab saves in a neutral style.
                let apply = if editor.mode == Mode::Script && editor.preview {
                    egui::Button::new("Save script (not run yet)")
                } else {
                    egui::Button::new(RichText::new("Apply").color(t.canvas)).fill(t.accent)
                }
                .min_size(egui::vec2(0.0, size::CONTROL_MD));
                if ui.add(apply).clicked() {
                    applied = Some(editor.draft.clone());
                    editor.applied = true;
                }
                let cancel =
                    egui::Button::new("Cancel").min_size(egui::vec2(0.0, size::CONTROL_MD));
                if ui.add(cancel).clicked() {
                    close = true;
                }
                if editor.applied && editor.preview {
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

/// The settings button that opens the rules editor for `name`. It returns true on a click.
pub(crate) fn cog(ui: &mut egui::Ui, name: &str) -> bool {
    let label = format!("{name} rules");
    crate::widgets::icon_button(
        ui,
        Icon::Settings,
        false,
        &label,
        &format!("{name} — rules"),
    )
    .clicked()
}

fn mode_bar(ui: &mut egui::Ui, editor: &mut Editor) {
    ui.horizontal(|ui| {
        let modes = [
            (Mode::Visual, "Visual"),
            (Mode::Script, "Script"),
            (Mode::Items, "Items & source"),
        ];
        for (mode, label) in modes {
            if widgets::selectable(ui, editor.mode == mode, label).clicked() {
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
                        changed |=
                            widgets::selectable_value(ui, &mut tab.order, p, p.label()).changed();
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
                            ui.horizontal(|ui| {
                                changed |= rule_row(ui, i, rule);
                                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                    let name = format!("🗑 Delete rule {}", i + 1);
                                    let del = egui::Button::new(RichText::new("🗑").color(t.danger))
                                        .frame(false);
                                    let r = ui.add(del).on_hover_text(
                                        "Delete this rule. Cancel throws the change away.",
                                    );
                                    r.widget_info(|| {
                                        egui::WidgetInfo::labeled(egui::Role::Button, true, &name)
                                    });
                                    if r.clicked() {
                                        remove = Some(i);
                                    }
                                });
                            });
                        });
                    ui.add_space(space::XS);
                }
            });
        if let Some(i) = remove {
            tab.visual.remove(i);
            changed = true;
        }
        if ui.button(icons::lead(Icon::Plus, "Add rule")).clicked() {
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
                    changed |=
                        widgets::selectable_value(ui, &mut rule.trigger, v, v.label()).changed();
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
                    changed |=
                        widgets::selectable_value(ui, &mut rule.place, v, v.label()).changed();
                }
            });
        if rule.place.is_fixed() {
            let tol = DragValue::new(&mut rule.tolerance_s)
                .range(0.0..=MAX_TOLERANCE_S)
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
                .small_button(icons::glyph(Icon::X).to_string())
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
        if ui
            .small_button(icons::lead(Icon::Plus, "constraint"))
            .clicked()
        {
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
                changed |= widgets::selectable_value(ui, &mut c.kind, v, v.label()).changed();
            }
        });
    if c.kind.has_value() {
        changed |= ui.add(DragValue::new(&mut c.n).range(1..=99)).changed();
    }
    changed
}

fn highlighted_script(ui: &egui::Ui, code: &str, t: &Tokens) -> egui::text::LayoutJob {
    let code_theme = egui_extras::syntax_highlighting::CodeTheme::dark(theme::CODE);
    let mut job =
        egui_extras::syntax_highlighting::highlight(ui.ctx(), ui.style(), &code_theme, code, "js");
    theme::script_palette(&mut job, t);
    job
}

fn script_job(ui: &egui::Ui, text: &str, width: f32, t: &Tokens) -> egui::text::LayoutJob {
    let key = (
        egui::util::hash(text),
        width.to_bits(),
        egui::util::hash((t.text, t.text_muted, t.field, t.ok, t.warn, t.accent_ink)),
    );
    let cache_id = ui.scope_id().with("script_palette");
    let cached = ui
        .ctx()
        .data_mut(|data| data.get_temp::<((u64, u32, u64), egui::text::LayoutJob)>(cache_id));
    match cached {
        Some((cached_key, job)) if cached_key == key => job,
        _ => {
            let mut job = highlighted_script(ui, text, t);
            job.wrap.max_width = width;
            ui.ctx()
                .data_mut(|data| data.insert_temp(cache_id, (key, job.clone())));
            job
        }
    }
}

fn script(ui: &mut egui::Ui, tab: &mut PlaylistTab, preview: bool, t: &Tokens) {
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
        let job = script_job(ui, text.as_str(), width, t);
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let edit = TextEdit::multiline(&mut tab.script)
        .layouter(&mut layouter)
        .font(theme::mono(theme::CODE))
        .code_editor()
        .desired_rows(size::SCRIPT_ROWS)
        .desired_width(f32::INFINITY);
    if ui.add(edit).changed() {
        tab.script_by_hand = true;
    }
    ui.add_space(space::XS);
    // A warning callout: the script is kept, but it does nothing yet.
    egui::Frame::new()
        .fill(t.warn_fill)
        .stroke(Stroke::new(size::HAIRLINE, t.warn))
        .corner_radius(t.radius)
        .inner_margin(pad::ROW)
        .show(ui, |ui| {
            let text = if preview {
                SCRIPT_NOTE
            } else {
                "The script is what plays: Apply reads its rules into the scheduler."
            };
            let note = RichText::new(icons::lead(Icon::TriangleAlert, text))
                .font(theme::text(theme::LABEL));
            ui.label(note.color(t.warn));
        });
}

/// The items of the playlist, and the folder behind it. Files from the file manager drop here.
fn items(ui: &mut egui::Ui, tab: &mut PlaylistTab, link: &mut Linked, preview: bool, t: &Tokens) {
    if let Some(src) = &mut tab.source {
        caption(ui, "FOLDER", t);
        ui.label(RichText::new(&src.path).font(theme::mono(theme::CODE)));
        ui.checkbox(&mut src.recursive, "Include subfolders");
        ui.horizontal(|ui| {
            ui.checkbox(&mut src.watch, "Watch for changes");
            if src.watch && preview {
                let note = RichText::new(WATCH_NOTE).font(theme::text(theme::CAPTION));
                ui.label(note.color(t.text_muted));
            }
        });
        ui.add_space(space::S);
    }
    caption(ui, &format!("ITEMS ({})", tab.items.len()), t);
    let frame = egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
        .corner_radius(t.radius)
        .inner_margin(pad::TIGHT);
    let zone = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut remove = None;
        let side = size::CONTROL_SM;
        TableBuilder::new(ui)
            .id_salt("editor_items")
            .cell_layout(Layout::left_to_right(Align::Center))
            .max_scroll_height(size::EDITOR_ITEMS_H)
            .auto_shrink([false, true])
            .column(Column::remainder().clip(true))
            .column(Column::auto())
            .column(Column::exact(side))
            .body(|body| {
                body.rows(side, tab.items.len(), |mut row| {
                    let i = row.index();
                    let (name, len) = &tab.items[i];
                    row.col(|ui| {
                        let title = RichText::new(name).font(theme::text(theme::SMALL));
                        ui.add(egui::Label::new(title).truncate())
                            .on_hover_text(name);
                    });
                    row.col(|ui| {
                        let len = RichText::new(mmss(*len)).font(theme::mono(theme::CODE));
                        ui.label(len.color(t.text_muted));
                    });
                    row.col(|ui| {
                        let resp = widgets::icon_button_sized(
                            ui,
                            Icon::X,
                            false,
                            &format!("Remove {name}"),
                            &format!("Remove {name} from this playlist"),
                            side,
                        );
                        if resp.clicked() {
                            remove = Some(i);
                        }
                    });
                });
            });
        if let Some(i) = remove {
            tab.items.remove(i);
            if i < tab.weights.len() {
                tab.weights.remove(i);
            }
            if i < tab.ids.len() {
                tab.ids.remove(i);
            }
        }
        let hint = "Drop files or a folder from the file manager here to add them";
        caption(ui, hint, t);
    });
    let rect = zone.response.rect;
    let over = ui.input(|i| {
        (!i.raw.hovered_files.is_empty() || !i.raw.dropped_files.is_empty())
            && i.pointer.latest_pos().is_some_and(|p| rect.contains(p))
    });
    if over {
        link.hover_files(FileTarget::Editor);
        let edge = Stroke::new(size::HAIRLINE * 2.0, t.accent);
        ui.painter()
            .rect_stroke(rect, t.radius, edge, egui::StrokeKind::Outside);
    }
}

#[cfg(test)]
mod highlighting_tests {
    #[test]
    fn cached_script_changes_colours_when_the_palette_changes() {
        let ctx = egui::Context::default();
        let mut colours = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let text = "const count = 3; rule({name: 'Test'});";
            let dark = super::script_job(
                ui,
                text,
                crate::theme::size::COMBO_W,
                &crate::theme::Tokens::DARK,
            );
            let mut other_palette = crate::theme::Tokens::DARK;
            std::mem::swap(&mut other_palette.ok, &mut other_palette.warn);
            let light = super::script_job(ui, text, crate::theme::size::COMBO_W, &other_palette);
            colours = Some((
                dark.sections
                    .iter()
                    .map(|s| s.format.color)
                    .collect::<Vec<_>>(),
                light
                    .sections
                    .iter()
                    .map(|s| s.format.color)
                    .collect::<Vec<_>>(),
            ));
        });
        output.textures_delta.clear();
        let (dark, light) = colours.unwrap();
        assert_ne!(
            dark, light,
            "cached colours must follow the current palette"
        );
    }

    #[test]
    fn javascript_has_multiple_colours() {
        let ctx = egui::Context::default();
        let mut colours = std::collections::HashSet::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = super::highlighted_script(
                ui,
                "// a rule\nconst take = 3; rule({name: \"Day\"});",
                &crate::theme::Tokens::DARK,
            );
            colours.extend(job.sections.iter().map(|section| section.format.color));
        });
        output.textures_delta.clear();
        assert!(
            colours.len() > 1,
            "JavaScript tokens must have different colours"
        );
    }
}
