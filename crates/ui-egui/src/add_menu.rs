//! The "+ Add" menu of a playlist: files, a folder, a stream, the live input, or the library.
//! The view source reads the files. The menu only picks paths and sends edits.

use std::time::Duration;

use egui::{DragValue, RichText};

use crate::app::Action;
use crate::context_menu::menu_item;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{AUDIO_EXTENSIONS, Edit, StationView, check_stream_url};
use crate::widgets::icon_button;
use crate::windows::ToolWindow;

/// The planned length of a new stream or live input, in minutes.
const DEFAULT_PLANNED_MIN: u32 = 60;
const MAX_PLANNED_MIN: u32 = 24 * 60;
const SECONDS_PER_MINUTE: u32 = 60;
const MAX_CHANNEL: u32 = 16;
/// The label of the keep-alive switch of a stream.
pub(crate) const KEEP_ALIVE_LABEL: &str =
    "Keep alive — reconnect until the planned end (don't skip)";

/// A dialog for an item that is not a file.
pub(crate) enum AddDialog {
    Url {
        playlist: String,
        at: Option<usize>,
        url: String,
        title: String,
        minutes: u32,
        seconds: u32,
        keep_alive: bool,
    },
    Live {
        playlist: String,
        at: Option<usize>,
        device: String,
        left: u32,
        right: u32,
        minutes: u32,
        seconds: u32,
    },
}

/// The fixed-size "+" button and its menu, for `playlist`.
pub(crate) fn button(
    ui: &mut egui::Ui,
    playlist: &str,
    at: Option<usize>,
    actions: &mut Vec<Action>,
) {
    let name = format!("+ Add to {playlist}");
    let resp = icon_button(ui, "+", false, &name, "Add items");
    egui::Popup::menu(&resp).show(|ui| items(ui, playlist, at, actions));
}

/// The rows of the add menu. Right-click menus use them too.
pub(crate) fn items(
    ui: &mut egui::Ui,
    playlist: &str,
    at: Option<usize>,
    actions: &mut Vec<Action>,
) {
    if menu_item(ui, "Add files…", None) {
        let picked = rfd::FileDialog::new()
            .add_filter("Audio", &AUDIO_EXTENSIONS)
            .pick_files();
        if let Some(files) = picked {
            let paths = files.iter().map(|p| p.display().to_string()).collect();
            actions.push(Action::Edit(Edit::AddFiles {
                playlist: playlist.to_owned(),
                paths,
                at,
            }));
        }
    }
    if menu_item(ui, "Add folder…", None)
        && let Some(dir) = rfd::FileDialog::new().pick_folder()
    {
        actions.push(Action::Edit(Edit::AddFolderItems {
            playlist: playlist.to_owned(),
            path: dir.display().to_string(),
            recursive: false,
        }));
    }
    if menu_item(ui, "Add URL…", None) {
        actions.push(Action::AddDialog(AddDialog::Url {
            playlist: playlist.to_owned(),
            at,
            url: String::new(),
            title: String::new(),
            minutes: DEFAULT_PLANNED_MIN,
            seconds: 0,
            keep_alive: true,
        }));
    }
    if menu_item(ui, "Add live input…", None) {
        actions.push(Action::AddDialog(AddDialog::Live {
            playlist: playlist.to_owned(),
            at,
            device: String::new(),
            left: 1,
            right: 2,
            minutes: DEFAULT_PLANNED_MIN,
            seconds: 0,
        }));
    }
    if menu_item(ui, "From Library…", None) {
        actions.push(Action::OpenWindow(ToolWindow::Library));
    }
}

fn planned(ui: &mut egui::Ui, minutes: &mut u32, seconds: &mut u32, t: &Tokens) {
    ui.label(RichText::new("Planned length").color(t.text_muted));
    ui.horizontal(|ui| {
        ui.add(
            DragValue::new(minutes)
                .range(0..=MAX_PLANNED_MIN)
                .suffix(" min"),
        );
        ui.add(
            DragValue::new(seconds)
                .range(0..=SECONDS_PER_MINUTE - 1)
                .suffix(" s"),
        );
    });
    ui.end_row();
}

fn len_of(minutes: u32, seconds: u32) -> Duration {
    Duration::from_secs(u64::from(minutes * SECONDS_PER_MINUTE + seconds))
}

/// Show the open add dialog. It closes after Add or Cancel.
pub(crate) fn dialog(
    ctx: &egui::Context,
    slot: &mut Option<AddDialog>,
    view: &StationView,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let Some(d) = slot.as_mut() else {
        return;
    };
    let mut close = false;
    let title = match d {
        AddDialog::Url { playlist, .. } => format!("Add URL to {playlist}"),
        AddDialog::Live { playlist, .. } => format!("Add live input to {playlist}"),
    };
    egui::Window::new(title)
        .id(egui::Id::new("add_dialog"))
        .collapsible(false)
        .resizable(false)
        .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
        .show(ctx, |ui| {
            let mut edit = None;
            egui::Grid::new("add_dialog_grid")
                .spacing(egui::vec2(space::M, space::S))
                .show(ui, |ui| match d {
                    AddDialog::Url {
                        playlist,
                        at,
                        url,
                        title,
                        minutes,
                        seconds,
                        keep_alive,
                    } => {
                        let label = ui.label(RichText::new("URL").color(t.text_muted));
                        let field = egui::TextEdit::singleline(url).hint_text("https://");
                        ui.add_sized([size::FIELD_W * 2.0, size::CONTROL_SM], field)
                            .labelled_by(label.id);
                        ui.end_row();
                        let label = ui.label(RichText::new("Title").color(t.text_muted));
                        let field = egui::TextEdit::singleline(title).hint_text("optional");
                        ui.add_sized([size::FIELD_W * 2.0, size::CONTROL_SM], field)
                            .labelled_by(label.id);
                        ui.end_row();
                        planned(ui, minutes, seconds, t);
                        ui.label("");
                        ui.checkbox(keep_alive, KEEP_ALIVE_LABEL);
                        ui.end_row();
                        if let Err(e) = check_stream_url(url) {
                            ui.label("");
                            ui.label(
                                RichText::new(e)
                                    .font(theme::text(theme::CAPTION))
                                    .color(t.danger),
                            );
                            ui.end_row();
                        } else {
                            edit = Some(Edit::AddStream {
                                playlist: playlist.clone(),
                                url: url.trim().to_owned(),
                                title: title.clone(),
                                planned_len: len_of(*minutes, *seconds),
                                keep_alive: *keep_alive,
                                at: *at,
                            });
                        }
                    }
                    AddDialog::Live {
                        playlist,
                        at,
                        device,
                        left,
                        right,
                        minutes,
                        seconds,
                    } => {
                        if device.is_empty() {
                            device.clone_from(&view.device_roles.input);
                        }
                        ui.label(RichText::new("Input device").color(t.text_muted));
                        egui::ComboBox::from_id_salt("add_live_device")
                            .width(size::COMBO_W)
                            .selected_text(device.as_str())
                            .show_ui(ui, |ui| {
                                for d in &view.devices {
                                    ui.selectable_value(device, d.clone(), d);
                                }
                            });
                        ui.end_row();
                        ui.label(RichText::new("Channels").color(t.text_muted));
                        ui.horizontal(|ui| {
                            ui.add(DragValue::new(left).range(1..=MAX_CHANNEL).prefix("L "));
                            ui.add(DragValue::new(right).range(1..=MAX_CHANNEL).prefix("R "));
                        });
                        ui.end_row();
                        planned(ui, minutes, seconds, t);
                        edit = Some(Edit::AddLive {
                            playlist: playlist.clone(),
                            device: device.clone(),
                            channels: (*left, *right),
                            planned_len: len_of(*minutes, *seconds),
                            at: *at,
                        });
                    }
                });
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                let add = egui::Button::new(RichText::new("Add").color(t.canvas)).fill(t.accent);
                if ui.add_enabled(edit.is_some(), add).clicked()
                    && let Some(e) = edit.take()
                {
                    actions.push(Action::Edit(e));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if close {
        *slot = None;
    }
}
