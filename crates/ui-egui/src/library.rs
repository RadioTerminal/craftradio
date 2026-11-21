//! The media library: folders and categories, a search box and a table of files.
//! A person drags a title onto the main pane, a playlist tab or a cart.
//! A folder can become a playlist, and that playlist can become the main playlist.
//! The view source reads folders. The library only asks for a scan and shows the result.

use std::path::Path;

use egui::{Align, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, FileTarget, Linked, Pane, mmss, step};
use crate::context_menu::{self, COPY, menu_item};
use crate::parts;
use crate::settings;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DropTarget, Edit, FolderView, LibraryItem, MetaTarget, Place, StationView};

/// The categories of a new library.
const DEFAULT_CATEGORIES: [&str; 5] = ["Music", "Jingles", "Ads", "Beds", "Shows"];

/// The drag payload: the id of a library item.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LibraryDrag(pub u64);

/// The last part of a path, for a folder name. This reads no files.
fn folder_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// What the tree selects.
#[derive(Clone, PartialEq, Eq)]
enum Pick {
    All,
    Category(String),
    Folder(String),
}

/// The view state of the window: docking, search, tree selection and draft fields.
pub(crate) struct State {
    pub docked: bool,
    search: String,
    pick: Pick,
    settings: bool,
    new_folder: String,
    new_category: String,
    /// The row that the arrow keys move from.
    selected: Option<u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            docked: false,
            search: String::new(),
            pick: Pick::All,
            settings: false,
            new_folder: String::new(),
            new_category: String::new(),
            selected: None,
        }
    }
}

impl State {
    /// Show all files that match `text`.
    pub fn show_search(&mut self, text: &str) {
        text.clone_into(&mut self.search);
        self.pick = Pick::All;
    }

    /// Ask the source to read a folder, and show that folder.
    fn add_folder(&mut self, path: &str, actions: &mut Vec<Action>) {
        let path = path.trim().to_owned();
        if path.is_empty() {
            return;
        }
        self.pick = Pick::Folder(path.clone());
        actions.push(Action::Edit(Edit::AddFolder { path }));
    }
}

/// The items that pass the tree and the search filter.
fn visible<'a>(view: &'a StationView, state: &State) -> Vec<&'a LibraryItem> {
    let needle = state.search.to_lowercase();
    let folder = match &state.pick {
        Pick::Folder(p) => view.folders.iter().find(|f| f.path == *p),
        _ => None,
    };
    let pool: Vec<&LibraryItem> = match (&state.pick, folder) {
        (Pick::Folder(_), Some(f)) => f.files.iter().collect(),
        (Pick::Folder(_), None) => Vec::new(),
        (Pick::Category(c), _) => view.library.iter().filter(|i| i.category == *c).collect(),
        (Pick::All, _) => view.library.iter().collect(),
    };
    pool.into_iter()
        .filter(|i| {
            needle.is_empty()
                || i.title.to_lowercase().contains(&needle)
                || i.artist
                    .as_ref()
                    .is_some_and(|a| a.to_lowercase().contains(&needle))
        })
        .collect()
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    // In its own native window, the library takes file drops itself.
    // Inside the main window, the app routes them.
    let own_window = ui.ctx().viewport_id() != egui::ViewportId::ROOT;
    let files = ui.input(|i| {
        (!i.raw.hovered_files.is_empty() || !i.raw.dropped_files.is_empty())
            && i.pointer
                .latest_pos()
                .is_some_and(|p| ui.max_rect().contains(p))
    });
    if own_window {
        let dropped: Vec<String> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().display().to_string())
                .collect()
        });
        if !dropped.is_empty() {
            actions.push(Action::Import(dropped));
        }
    } else if files {
        link.hover_files(FileTarget::Library);
    }

    let mut open = state.settings;
    let mut docked = state.docked;
    let mut add = false;
    settings::header(ui, "Library", &mut open, |ui| {
        let label = if docked {
            "Undock to a window"
        } else {
            "Dock into main window"
        };
        if ui.selectable_label(docked, label).clicked() {
            docked = !docked;
        }
        if ui.button("+ Add folder…").clicked() {
            add = true;
        }
    });
    state.settings = open;
    state.docked = docked;
    if add && let Some(dir) = rfd::FileDialog::new().pick_folder() {
        state.add_folder(&dir.display().to_string(), actions);
    }
    if state.settings {
        settings::section(ui, "Library", t, |ui| {
            library_settings(ui, view, state, actions, t)
        });
    }
    ui.horizontal(|ui| {
        let name = ui.label("Search");
        let edit = egui::TextEdit::singleline(&mut state.search).hint_text("title or artist");
        ui.add(edit.desired_width(size::FIELD_W))
            .labelled_by(name.id);
        let items = visible(view, state);
        let count = format!(
            "{} of {} files · drag a title onto a pane, a playlist or a cart",
            items.len(),
            view.library.len()
        );
        ui.label(
            RichText::new(count)
                .font(theme::text(theme::CAPTION))
                .color(t.text_muted),
        );
    });
    if let Pick::Folder(path) = &state.pick
        && let Some(folder) = view.folders.iter().find(|f| f.path == *path)
    {
        folder_header(ui, folder, actions, t);
    }
    ui.add_space(space::S);
    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(size::TREE_W, ui.available_height()), |ui| {
            ui.set_width(size::TREE_W);
            tree(ui, view, state, actions, t);
        });
        ui.separator();
        ui.vertical(|ui| table(ui, view, state, link, actions, t));
    });
}

/// The file count of a folder. A scan that a limit stopped says so.
fn count_text(folder: &FolderView) -> String {
    let n = folder.files.len();
    if folder.truncated {
        format!("{n} of more — truncated")
    } else {
        format!("{n} audio files")
    }
}

/// The two folder actions, as menu rows.
fn folder_actions(ui: &mut egui::Ui, folder: &FolderView, actions: &mut Vec<Action>) {
    for (label, main) in [
        ("Create playlist from folder", false),
        ("Use as main playlist", true),
    ] {
        if menu_item(ui, label, None) {
            actions.push(Action::FolderPlaylist {
                path: folder.path.clone(),
                main,
            });
        }
    }
}

fn folder_header(ui: &mut egui::Ui, folder: &FolderView, actions: &mut Vec<Action>, t: &Tokens) {
    ui.horizontal(|ui| {
        let text = format!("📁 {} · {}", folder.path, count_text(folder));
        ui.label(RichText::new(text).font(theme::text(theme::LABEL)));
        if !folder.files.is_empty() {
            for (label, main) in [
                ("Create playlist from folder", false),
                ("Use as main playlist", true),
            ] {
                if ui.button(label).clicked() {
                    actions.push(Action::FolderPlaylist {
                        path: folder.path.clone(),
                        main,
                    });
                }
            }
        }
    });
    for e in &folder.errors {
        ui.label(
            RichText::new(e)
                .font(theme::text(theme::CAPTION))
                .color(t.danger),
        );
    }
}

fn tree(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    egui::CollapsingHeader::new("Folders")
        .default_open(true)
        .show(ui, |ui| {
            if ui
                .selectable_label(state.pick == Pick::All, "All files")
                .clicked()
            {
                state.pick = Pick::All;
            }
            let folder_names: Vec<&str> =
                view.folders.iter().map(|f| folder_name(&f.path)).collect();
            let mut cats = parts::strings(view, parts::LIBRARY, "categories", &DEFAULT_CATEGORIES);
            for item in &view.library {
                if !cats.contains(&item.category) && !folder_names.contains(&item.category.as_str())
                {
                    cats.push(item.category.clone());
                }
            }
            for c in cats {
                let n = view.library.iter().filter(|i| i.category == c).count();
                let on = state.pick == Pick::Category(c.clone());
                if ui.selectable_label(on, format!("{c} ({n})")).clicked() {
                    state.pick = Pick::Category(c);
                }
            }
            for folder in &view.folders {
                let on = state.pick == Pick::Folder(folder.path.clone());
                let colour = if folder.errors.is_empty() {
                    t.text
                } else {
                    t.danger
                };
                let text = format!("📁 {} ({})", folder_name(&folder.path), count_text(folder));
                let r = ui.selectable_label(on, RichText::new(text).color(colour));
                if r.clicked() {
                    state.pick = Pick::Folder(folder.path.clone());
                }
                r.context_menu(|ui| {
                    folder_actions(ui, folder, actions);
                    if menu_item(ui, "Rescan", None) {
                        actions.push(Action::Edit(Edit::Rescan {
                            folder: folder.path.clone(),
                        }));
                    }
                    if context_menu::danger_item(ui, "Remove folder…", t) {
                        actions.push(Action::Confirm {
                            text: format!(
                                "Remove the folder {} from the Library? The files stay on disk.",
                                folder.path
                            ),
                            yes: "Remove folder".to_owned(),
                            then: Box::new(Action::Edit(Edit::RemoveFolder {
                                path: folder.path.clone(),
                            })),
                        });
                    }
                });
            }
        });
}

fn table(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    link: &mut Linked,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let items = visible(view, state);
    let order: Vec<u64> = items.iter().map(|i| i.id).collect();
    let mut scroll = None;
    if link.focus == Pane::Library
        && link.keys.step != 0
        && let Some(id) = step(&order, state.selected, link.keys.step)
    {
        state.selected = Some(id);
        scroll = order.iter().position(|o| *o == id);
    }
    let selected = state.selected;
    let mut clicked = None;
    let head = |s: &str| {
        RichText::new(s)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    let mut builder = TableBuilder::new(ui);
    if let Some(i) = scroll {
        builder = builder.scroll_to_row(i, Some(Align::Center));
    }
    builder
        .id_salt("library_table")
        .striped(true)
        .sense(Sense::click_and_drag())
        .auto_shrink(false)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(size::COL_TITLE).clip(true))
        .column(Column::exact(size::COL_ARTIST).clip(true))
        .column(Column::exact(size::COL_LEN))
        .column(Column::exact(size::COL_CATEGORY))
        .column(Column::remainder().clip(true))
        .header(size::DIVIDER_ROW, |mut h| {
            for name in ["TITLE", "ARTIST", "LENGTH", "CATEGORY", "PATH"] {
                h.col(|ui| {
                    ui.label(head(name));
                });
            }
        })
        .body(|body| {
            body.rows(size::ROW, items.len(), |mut row| {
                let Some(item) = items.get(row.index()) else {
                    return;
                };
                row.set_selected(selected == Some(item.id));
                row.col(|ui| {
                    let text = RichText::new(format!("☰ {}", item.title));
                    let label = egui::Label::new(text.font(theme::text(theme::SMALL)));
                    ui.add(label.truncate());
                });
                row.col(|ui| {
                    let artist = item.artist.as_deref().unwrap_or("—");
                    let text = RichText::new(artist).font(theme::text(theme::SMALL));
                    ui.add(egui::Label::new(text).truncate());
                });
                row.col(|ui| {
                    let text = RichText::new(mmss(item.len)).font(theme::mono(theme::CODE));
                    ui.label(text.color(t.text_muted));
                });
                row.col(|ui| {
                    ui.label(RichText::new(&item.category).font(theme::text(theme::CAPTION)));
                });
                row.col(|ui| {
                    let path = RichText::new(&item.path).font(theme::mono(theme::CAPTION));
                    ui.add(egui::Label::new(path.color(t.text_muted)).truncate());
                });
                let resp = row.response();
                if resp.clicked() || resp.secondary_clicked() {
                    clicked = Some(item.id);
                }
                // The whole row is the drag handle. A label follows the pointer while it moves.
                resp.dnd_set_drag_payload(LibraryDrag(item.id));
                if resp.dragged()
                    && let Some(pos) = resp.ctx.pointer_interact_pos()
                {
                    egui::Area::new(egui::Id::new(("library_drag", item.id)))
                        .order(egui::Order::Tooltip)
                        .fixed_pos(pos + egui::vec2(space::M, space::M))
                        .show(&resp.ctx, |ui| {
                            theme::card_frame(t).inner_margin(pad::ROW).show(ui, |ui| {
                                ui.label(format!("☰ {}", item.title));
                            });
                        });
                }
                resp.context_menu(|ui| item_menu(ui, view, item, actions));
            });
        });
    if let Some(id) = clicked {
        state.selected = Some(id);
        link.focus = Pane::Library;
    }
}

fn item_menu(ui: &mut egui::Ui, view: &StationView, item: &LibraryItem, actions: &mut Vec<Action>) {
    if menu_item(ui, &format!("Add to {}", view.main_playlist), None) {
        actions.push(Action::Edit(Edit::Insert {
            item: item.id,
            at: Place::End,
        }));
    }
    ui.menu_button("Add to playlist", |ui| {
        for tab in &view.tabs {
            if menu_item(ui, &tab.name, None) {
                actions.push(Action::Drop {
                    item: item.id,
                    target: DropTarget::Playlist(tab.name.clone()),
                });
            }
        }
    });
    ui.menu_button("Assign to cart", |ui| {
        let free = view.carts.iter().position(String::is_empty);
        if context_menu::menu_item_if(ui, free.is_some(), "First free cart", None)
            && let Some(cart) = free
        {
            actions.push(Action::Edit(Edit::AssignCart {
                cart,
                item: Some(item.id),
            }));
        }
        ui.separator();
        for (cart, label) in view.carts.iter().enumerate() {
            let shown = if label.is_empty() { "empty" } else { label };
            if menu_item(ui, &format!("Cart {} · {shown}", cart + 1), None) {
                actions.push(Action::Edit(Edit::AssignCart {
                    cart,
                    item: Some(item.id),
                }));
            }
        }
    });
    if menu_item(ui, "Copy", Some(COPY)) {
        actions.push(Action::CopyItems(vec![item.id]));
    }
    ui.separator();
    if menu_item(ui, "Reveal in folder", None) {
        let text = format!(
            "preview — {} · revealing needs the desktop integration",
            item.path
        );
        actions.push(Action::Toast(text));
    }
    if menu_item(ui, "Properties", None) {
        actions.push(Action::Metadata(vec![MetaTarget::Library(item.id)]));
    }
}

fn library_settings(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &mut State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    settings::label(ui, "Folders (the source reads only file names)", t);
    for f in &view.folders {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&f.path).font(theme::mono(theme::CODE)));
            ui.label(
                RichText::new(count_text(f))
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
            if ui.small_button("Remove folder").clicked() {
                actions.push(Action::Edit(Edit::RemoveFolder {
                    path: f.path.clone(),
                }));
            }
        });
        for e in &f.errors {
            ui.label(
                RichText::new(e)
                    .font(theme::text(theme::CAPTION))
                    .color(t.danger),
            );
        }
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.new_folder).desired_width(size::FIELD_W));
        if ui.button("+ Add folder by path").clicked() {
            let path = std::mem::take(&mut state.new_folder);
            state.add_folder(&path, actions);
        }
    });
    ui.add_space(space::S);
    settings::label(ui, "Categories", t);
    let cats = parts::strings(view, parts::LIBRARY, "categories", &DEFAULT_CATEGORIES);
    let mut next = cats.clone();
    for (i, c) in cats.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(c).font(theme::mono(theme::CODE)));
            if ui.small_button("Remove category").clicked() {
                next.remove(i);
            }
        });
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.new_category).desired_width(size::FIELD_W));
        if ui.button("+ Add category").clicked() && !state.new_category.trim().is_empty() {
            next.push(state.new_category.trim().to_owned());
            state.new_category.clear();
        }
    });
    if next != cats {
        actions.push(parts::set(parts::LIBRARY, "categories", next));
    }
}
