//! The metadata editor: the tags of one or more items, and the exact stream title.
//!
//! With several items, a field shows a value only when all items share it.
//! Apply changes only the fields that the person edited.

use crate::numbers;
use crate::widgets;
use egui::{RichText, TextEdit};

use crate::chrome::{Answer, Dialog};
use crate::icons::Icon;
use crate::theme::{self, Tokens, size, space};
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
    picture_touched: bool,
    /// True when the items do not share one picture.
    picture_mixed: bool,
    picture_check: crate::view::PictureCheck,
    keep_alive_touched: bool,
    trim_touched: bool,
    /// True when the items do not share one gain trim.
    trim_mixed: bool,
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
        let picture_mixed = targets.iter().any(|(_, m)| m.picture != draft.picture);
        if picture_mixed {
            draft.picture.clear();
        }
        let first_trim = draft.gain_trim_db;
        let trim_mixed = targets.iter().any(|(_, m)| m.gain_trim_db != first_trim);
        Self {
            targets,
            draft,
            mixed,
            touched: [false; 6],
            publish_touched: false,
            custom_touched: false,
            picture_touched: false,
            picture_mixed,
            picture_check: crate::view::PictureCheck::default(),
            keep_alive_touched: false,
            trim_touched: false,
            trim_mixed,
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

    /// Why the edited picture is refused. The pictures that nobody edited are not checked.
    fn picture_error(&self) -> Option<&'static str> {
        if self.picture_touched {
            self.picture_check.of(&self.draft.picture).err()
        } else {
            None
        }
    }

    /// True while the empty path stands for pictures that differ, which Clear can still
    /// take from every item.
    fn pictures_differ(&self) -> bool {
        self.picture_mixed && !self.picture_touched
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
        if self.picture_touched {
            out.picture.clone_from(&self.draft.picture);
        }
        // Keep-alive applies only to streams.
        if self.keep_alive_touched && out.keep_alive.is_some() {
            out.keep_alive = self.draft.keep_alive;
        }
        // The trim applies only to station files.
        if self.trim_touched && out.gain_trim_db.is_some() {
            out.gain_trim_db = self.draft.gain_trim_db;
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
    let mut close = false;
    let mut applied = None;
    let title = ed.title();
    let ok_label = match ed.targets.len() {
        1 => "Apply to this item".to_owned(),
        n => format!("Apply to {n} items"),
    };
    let (note_shown, base_preview) = (ed.note, ed.base_preview);
    let can_apply = ed.picture_error().is_none();
    let mut write_pressed = false;
    let answer = Dialog::new("meta_editor", Icon::Tag, &title)
        .ok(&ok_label, can_apply)
        .width(size::EDITOR_W)
        .show(
            ctx,
            t,
            |ui| {
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
                        if widgets::selectable(ui, ed.draft.publish == mode, mode.label()).clicked()
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
                ui.add_space(space::S);
                caption(ui, "PICTURE", t);
                ui.horizontal_top(|ui| {
                    let label = ui.label(RichText::new("Picture").color(t.text_muted));
                    let differ = ed.pictures_differ();
                    let hint = if differ {
                        "(mixed)"
                    } else {
                        "path to a PNG, JPEG or WebP picture"
                    };
                    // A picture that nobody edited is not judged, so it never blocks Apply.
                    let problem = ed.picture_error();
                    let path = &mut ed.draft.picture;
                    if widgets::picture_field(ui, path, hint, label.id, problem, differ, t) {
                        ed.picture_touched = true;
                    }
                });
                caption(
                    ui,
                    "Listeners see it as the cover when the title is shown.",
                    t,
                );
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
                if let Some(trim) = ed.draft.gain_trim_db.as_mut() {
                    ui.add_space(space::S);
                    caption(ui, "LOUDNESS", t);
                    ui.horizontal(|ui| {
                        let label = ui.label(RichText::new("Gain trim").color(t.text_muted));
                        let (lo, hi) = radio_model::loudness_range::TRIM_DB;
                        let field = numbers::field(
                            ui,
                            egui::DragValue::new(trim)
                                .range(lo..=hi)
                                .speed(0.1)
                                .max_decimals(1)
                                .suffix(" dB"),
                        );
                        if ui
                            .add_sized([size::FIELD_W, size::CONTROL_SM], field)
                            .labelled_by(label.id)
                            .changed()
                        {
                            ed.trim_touched = true;
                        }
                    });
                    let note = if ed.trim_mixed && !ed.trim_touched {
                        "The items have different trims. A change sets one trim on all of them."
                    } else {
                        "On top of the loudness gain. The peak guard and the caps still apply."
                    };
                    caption(ui, note, t);
                }
                ui.add_space(space::S);
                caption(ui, "Cue points can't be edited in this build.", t);
            },
            |ui| {
                if ui.button("Write tags to file").clicked() {
                    write_pressed = true;
                }
                if note_shown {
                    let note = if base_preview {
                        NOT_WRITTEN
                    } else {
                        "Writing tags to files is not in this build yet"
                    };
                    ui.label(RichText::new(note).color(t.warn));
                }
            },
        );
    if write_pressed {
        ed.note = true;
    }
    match answer {
        Some(Answer::Ok) => {
            let out = ed.targets.iter().map(|(k, m)| (*k, ed.merged(m))).collect();
            applied = Some(out);
            close = true;
        }
        Some(Answer::Cancel) => close = true,
        None => {}
    }
    if close {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::ViewSource;

    fn editor(pictures: [&str; 2]) -> MetaEditor {
        let tz = jiff::tz::TimeZone::get("Europe/Ljubljana").expect("tz");
        let ts = "2026-11-21T13:58:30Z".parse().expect("ts");
        let base = crate::MockSource::new(tz).snapshot(ts).rows[0].clone();
        let targets = pictures
            .iter()
            .zip(1..)
            .map(|(p, id)| {
                let meta = ItemMeta {
                    title: format!("Show {id}"),
                    picture: (*p).to_owned(),
                    ..ItemMeta::default()
                };
                (MetaTarget::Row(id), meta)
            })
            .collect();
        MetaEditor::new(targets, base)
    }

    #[test]
    fn an_untouched_picture_keeps_each_items_own() {
        let mut ed = editor(["/a.png", "/b.png"]);
        ed.draft.title = "Same".to_owned();
        ed.touched[0] = true;
        let merged: Vec<_> = ed.targets.iter().map(|(_, m)| ed.merged(m)).collect();
        assert_eq!(merged[0].picture, "/a.png");
        assert_eq!(merged[1].picture, "/b.png");
        assert!(ed.picture_mixed, "the items disagree");

        ed.draft.picture = "/new.webp".to_owned();
        ed.picture_touched = true;
        let merged: Vec<_> = ed.targets.iter().map(|(_, m)| ed.merged(m)).collect();
        assert!(merged.iter().all(|m| m.picture == "/new.webp"));
    }

    #[test]
    fn pictures_that_differ_can_all_be_cleared() {
        let mut ed = editor(["/a.png", "/b.png"]);
        assert!(ed.draft.picture.is_empty(), "the field shows no path");
        assert!(ed.pictures_differ(), "yet Clear has something to clear");
        // What Clear does to the field.
        ed.draft.picture.clear();
        ed.picture_touched = true;
        assert!(
            ed.targets
                .iter()
                .all(|(_, m)| ed.merged(m).picture.is_empty())
        );
        assert!(!ed.pictures_differ(), "then there is nothing left to clear");

        let same = editor(["/a.png", "/a.png"]);
        assert!(!same.pictures_differ());
    }

    #[test]
    fn a_picture_that_is_not_one_blocks_save_only_once_edited() {
        let mut ed = editor(["/a.mp3", "/a.mp3"]);
        assert!(ed.picture_error().is_none(), "nobody edited it");
        ed.draft.picture = "/notes.txt".to_owned();
        ed.picture_touched = true;
        assert!(ed.picture_error().is_some());
        ed.draft.picture.clear();
        assert!(ed.picture_error().is_none(), "clearing is always fine");
    }
}
