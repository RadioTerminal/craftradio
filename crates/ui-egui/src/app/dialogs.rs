//! Confirm and rename dialogs.

use super::*;
use crate::chrome::{Answer, Dialog, Tone};
use crate::icons::Icon;

impl RadioApp {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>, t: &Tokens) {
        add_menu::dialog(ctx, &mut self.add_dialog, &self.view, actions, t);
        crate::one_off::show(ctx, &mut self.one_off, &self.view, actions, t);
        if let Some(c) = &self.confirm {
            let text = c.text.clone();
            let yes_label = c.yes.clone();
            let answer = Dialog::new("confirm", Icon::TriangleAlert, "Confirm")
                .ok(&yes_label, true)
                .tone(Tone::Danger)
                .show(
                    ctx,
                    t,
                    |ui| {
                        ui.add(egui::Label::new(text).wrap());
                    },
                    |_| {},
                );
            if let Some(answer) = answer
                && let Some(c) = self.confirm.take()
                && answer == Answer::Ok
            {
                actions.push(c.then);
            }
        }
        if let Some((name, count, remove_locked, folder)) = &mut self.clear {
            let title = format!("Clear {name}");
            let answer = Dialog::new("clear", Icon::Trash2, &title)
                .ok("Clear playlist", true)
                .tone(Tone::Danger)
                .show(ctx, t, |ui| {
                    ui.add(
                        egui::Label::new(format!(
                            "Remove all {count} items from {name}? Items on air or loaded on a deck stay. You can undo it."
                        ))
                        .wrap(),
                    );
                    ui.checkbox(remove_locked, "Also remove locked items");
                    if *folder {
                        let note = "This playlist follows its folder. Clearing turns off \
                                    \"watch for changes\", or the watch would add the files back.";
                        ui.add(egui::Label::new(RichText::new(note).color(t.warn)).wrap());
                    }
                }, |_| {});
            if let Some(answer) = answer
                && let Some((playlist, expect_count, remove_locked, _)) = self.clear.take()
                && answer == Answer::Ok
            {
                actions.push(Action::Edit(Edit::ClearPlaylist {
                    playlist,
                    expect_count,
                    remove_locked,
                }));
            }
        }
        if let Some((from, buffer)) = &mut self.rename {
            let title = format!("Rename {from}");
            let mut enter = false;
            let mut answer = Dialog::new("rename", Icon::Pencil, &title)
                .ok("Rename", !buffer.trim().is_empty())
                .show(
                    ctx,
                    t,
                    |ui| {
                        let edit = ui.add(
                            egui::TextEdit::singleline(buffer).desired_width(ui.available_width()),
                        );
                        enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    },
                    |_| {},
                );
            if enter && !buffer.trim().is_empty() {
                answer = Some(Answer::Ok);
            }
            if let Some(answer) = answer
                && let Some((from, to)) = self.rename.take()
                && answer == Answer::Ok
            {
                actions.push(Action::Edit(Edit::RenamePlaylist { from, to }));
            }
        }
    }
}
