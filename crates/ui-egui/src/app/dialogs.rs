//! Confirm and rename dialogs.

use super::*;

impl RadioApp {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>, t: &Tokens) {
        add_menu::dialog(ctx, &mut self.add_dialog, &self.view, actions, t);
        if let Some((text, _)) = &self.confirm {
            let text = text.clone();
            let mut answer = None;
            egui::Window::new("Confirm")
                .collapsible(false)
                .resizable(false)
                .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
                .show(ctx, |ui| {
                    ui.label(text);
                    ui.add_space(space::S);
                    ui.horizontal(|ui| {
                        let yes = egui::Button::new(RichText::new("Delete").color(t.text))
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
                && let Some((_, then)) = self.confirm.take()
                && yes
            {
                actions.push(then);
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
