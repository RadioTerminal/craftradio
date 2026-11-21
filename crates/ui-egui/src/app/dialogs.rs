//! Confirm and rename dialogs.

use super::*;

impl RadioApp {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>, t: &Tokens) {
        add_menu::dialog(ctx, &mut self.add_dialog, &self.view, actions, t);
        crate::one_off::show(ctx, &mut self.one_off, &self.view, actions, t);
        if let Some(c) = &self.confirm {
            let text = c.text.clone();
            let yes_label = c.yes.clone();
            let mut answer = None;
            egui::Window::new("Confirm")
                .collapsible(false)
                .resizable(false)
                .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
                .show(ctx, |ui| {
                    ui.label(text);
                    ui.add_space(space::S);
                    ui.horizontal(|ui| {
                        let yes = egui::Button::new(RichText::new(yes_label).color(t.text))
                            .fill(t.danger_fill);
                        if ui.add(yes).clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            if let Some(yes) = answer
                && let Some(c) = self.confirm.take()
                && yes
            {
                actions.push(c.then);
            }
        }
        if let Some((name, count, remove_locked, folder)) = &mut self.clear {
            let mut answer = None;
            egui::Window::new(format!("Clear {name}"))
                .collapsible(false)
                .resizable(false)
                .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
                .show(ctx, |ui| {
                    ui.set_max_width(size::DIALOG_W);
                    ui.label(format!(
                        "Remove all {count} items from {name}? Items on air or loaded on a deck stay. You can undo it."
                    ));
                    ui.checkbox(remove_locked, "Also remove locked items");
                    if *folder {
                        let note = "This playlist follows its folder. Clearing turns off \
                                    \"watch for changes\", or the watch would add the files back.";
                        ui.label(RichText::new(note).color(t.warn));
                    }
                    ui.add_space(space::S);
                    ui.horizontal(|ui| {
                        let yes = egui::Button::new(RichText::new("Clear playlist").color(t.text))
                            .fill(t.danger_fill);
                        if ui.add(yes).clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            if let Some(yes) = answer
                && let Some((playlist, expect_count, remove_locked, _)) = self.clear.take()
                && yes
            {
                actions.push(Action::Edit(Edit::ClearPlaylist {
                    playlist,
                    expect_count,
                    remove_locked,
                }));
            }
        }
        if let Some((from, buffer)) = &mut self.rename {
            let mut answer = None;
            egui::Window::new(format!("Rename {from}"))
                .collapsible(false)
                .resizable(false)
                .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
                .show(ctx, |ui| {
                    let edit =
                        ui.add(egui::TextEdit::singleline(buffer).desired_width(size::FIELD_W));
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        answer = Some(true);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            if let Some(yes) = answer
                && let Some((from, to)) = self.rename.take()
                && yes
            {
                actions.push(Action::Edit(Edit::RenamePlaylist { from, to }));
            }
        }
    }
}
