//! The metadata editor: the tags of one or more items, and the exact stream title.
//!
//! With several items, a field shows a value only when all items share it.
//! Apply changes only the fields that the person edited.

use egui::{RichText, TextEdit};

use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{ItemMeta, MetaTarget, PLACEHOLDERS, PublishMode, Row, ViewSource};

const NOT_WRITTEN: &str = "preview — not written";
const FIELDS: [&str; 6] = ["Title", "Artist", "Album", "Year", "Genre", "Comment"];

fn field(m: &mut ItemMeta, i: usize) -> &mut String {
    match i {
        0 => &mut m.title,
        1 => &mut m.artist,
        2 => &mut m.album,
        3 => &mut m.year,
        4 => &mut m.genre,
        _ => &mut m.comment,
    }
}

/// An open metadata editor.
pub(crate) struct MetaEditor {
    targets: Vec<(MetaTarget, ItemMeta)>,
    draft: ItemMeta,
    /// True for each text field where the items do not agree.
    mixed: [bool; 6],
    touched: [bool; 6],
    publish_touched: bool,
    custom_touched: bool,
    keep_alive_touched: bool,
    /// The row whose place in the timeline gives the preview line its context.
    base: Row,
    note: bool,
    /// The source is the preview.
    pub(crate) base_preview: bool,
}

impl MetaEditor {
    pub fn new(targets: Vec<(MetaTarget, ItemMeta)>, base: Row) -> Self {
        let mut draft = targets.first().map(|(_, m)| m.clone()).unwrap_or_default();
        let mut mixed = [false; 6];
        for (i, m) in mixed.iter_mut().enumerate() {
            let first = field(&mut draft, i).clone();
            *m = targets
                .iter()
                .any(|(_, t)| *field(&mut t.clone(), i) != first);
            if *m {
                field(&mut draft, i).clear();
            }
        }
        Self {
            targets,
            draft,
            mixed,
            touched: [false; 6],
            publish_touched: false,
            custom_touched: false,
            keep_alive_touched: false,
            base,
            note: false,
            base_preview: true,
        }
    }

    fn title(&self) -> String {
        match self.targets.as_slice() {
            [(_, m)] => format!("{} — metadata", m.title),
            many => format!("{} items — metadata", many.len()),
        }
    }

    /// The tags of `meta` with the edited fields of the draft.
    fn merged(&self, meta: &ItemMeta) -> ItemMeta {
        let mut out = meta.clone();
        let mut draft = self.draft.clone();
        for (i, touched) in self.touched.iter().enumerate() {
            if *touched {
                *field(&mut out, i) = field(&mut draft, i).clone();
            }
        }
        if self.publish_touched {
            out.publish = self.draft.publish;
        }
        if self.custom_touched {
            out.custom.clone_from(&self.draft.custom);
        }
        // Keep-alive applies only to streams.
        if self.keep_alive_touched && out.keep_alive.is_some() {
            out.keep_alive = self.draft.keep_alive;
        }
        out
    }
}

/// Show the editor while it is open. After Apply, it returns the new tags of each item.
pub(crate) fn show(
    ctx: &egui::Context,
    slot: &mut Option<MetaEditor>,
    source: &dyn ViewSource,
    t: &Tokens,
) -> Option<Vec<(MetaTarget, ItemMeta)>> {
    let ed = slot.as_mut()?;
    let mut open = true;
    let mut close = false;
    let mut applied = None;
    egui::Window::new(ed.title())
        .id(egui::Id::new("meta_editor"))
        .open(&mut open)
        .collapsible(false)
        .default_width(size::EDITOR_W)
        .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
        .show(ctx, |ui| {
            caption(ui, "TAGS", t);
            egui::Grid::new("meta_fields")
                .spacing(egui::vec2(space::M, space::XS))
                .show(ui, |ui| {
                    for (i, name) in FIELDS.iter().enumerate() {
                        let label = ui.label(RichText::new(*name).color(t.text_muted));
                        let hint = if ed.mixed[i] && !ed.touched[i] {
                            "(mixed)"
                        } else {
                            ""
                        };
                        let edit = TextEdit::singleline(field(&mut ed.draft, i))
                            .hint_text(hint)
                            .desired_width(size::FIELD_W * 2.0);
                        if ui
                            .add_sized([size::FIELD_W * 2.0, size::CONTROL_SM], edit)
                            .labelled_by(label.id)
                            .changed()
                        {
                            ed.touched[i] = true;
                        }
                        ui.end_row();
                    }
                });
            ui.add_space(space::M);
            caption(ui, "STREAM TITLE", t);
            ui.horizontal(|ui| {
                for mode in PublishMode::ALL {
                    if ui
                        .selectable_label(ed.draft.publish == mode, mode.label())
                        .clicked()
                    {
                        ed.draft.publish = mode;
                        ed.publish_touched = true;
                    }
                }
            });
            ui.add_enabled_ui(ed.draft.publish == PublishMode::Custom, |ui| {
                ui.horizontal(|ui| {
                    let label = ui.label(RichText::new("Custom text").color(t.text_muted));
                    let edit = TextEdit::singleline(&mut ed.draft.custom)
                        .hint_text("{artist} – {title}")
                        .desired_width(size::FIELD_W * 2.0);
                    if ui
                        .add_sized([size::FIELD_W * 2.0, size::CONTROL_SM], edit)
                        .labelled_by(label.id)
                        .changed()
                    {
                        ed.custom_touched = true;
                    }
                });
                caption(ui, &format!("Placeholders: {}", PLACEHOLDERS.join(" ")), t);
            });
            let first = ed
                .targets
                .first()
                .map(|(_, m)| ed.merged(m))
                .unwrap_or_default();
            // The source makes the stream title, so the preview and the stream always agree.
            let row = Row {
                meta: first,
                ..ed.base.clone()
            };
            let text = source.stream_title(&row);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Icecast gets:").color(t.text_muted));
                let shown = RichText::new(format!("“{text}”")).font(theme::mono(theme::CODE));
                ui.label(shown.color(t.accent_ink));
            });
            if ed.targets.len() > 1 {
                caption(ui, "The line shows the first selected item.", t);
            }
            if let Some(keep) = ed.draft.keep_alive.as_mut() {
                ui.add_space(space::S);
                caption(ui, "STREAM", t);
                if ui
                    .checkbox(keep, crate::add_menu::KEEP_ALIVE_LABEL)
                    .changed()
                {
                    ed.keep_alive_touched = true;
                }
            }
            ui.add_space(space::S);
            caption(ui, "Cue points can't be edited in this build.", t);
            ui.separator();
            ui.horizontal(|ui| {
                let apply = egui::Button::new(
                    RichText::new(match ed.targets.len() {
                        1 => "Apply to this item".to_owned(),
                        n => format!("Apply to {n} items"),
                    })
                    .color(t.canvas),
                )
                .fill(t.accent);
                if ui.add(apply).clicked() {
                    let out = ed.targets.iter().map(|(k, m)| (*k, ed.merged(m))).collect();
                    applied = Some(out);
                    close = true;
                }
                if ui.button("Write tags to file").clicked() {
                    ed.note = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ed.note {
                    let note = if ed.base_preview {
                        NOT_WRITTEN
                    } else {
                        "Writing tags to files is not in this build yet"
                    };
                    ui.label(RichText::new(note).color(t.warn));
                }
            });
        });
    if close || !open {
        *slot = None;
    }
    applied
}

fn caption(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.label(
        RichText::new(text)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted),
    );
}
