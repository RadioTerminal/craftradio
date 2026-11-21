//! The media library: folders and categories, a search box and a table of files.
//! A person drags a title onto the main pane, a playlist tab or a cart.
//! A folder can become a playlist, and that playlist can become the main playlist.

use std::path::Path;

use egui::{Align, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, FileTarget, Linked, mmss, row_fields};
use crate::context_menu::{self, COPY, menu_item};
use crate::folders;
use crate::settings;
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DropTarget, Edit, LibraryItem, Place, StationView};

/// The drag payload: the id of a library item.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LibraryDrag(pub u64);

/// A folder that a person added, with the result of reading it.
struct Folder {
    path: String,
    files: Result<Vec<String>, String>,
}

impl Folder {
    fn name(&self) -> &str {
        folders::name_of(&self.path)
    }
}

/// What the tree selects.
#[derive(Clone, PartialEq, Eq)]
enum Pick {
    All,
    Category(String),
    Folder(String),
}

/// The library state. It is local to the window.
pub(crate) struct State {
    pub docked: bool,
    search: String,
    pick: Pick,
    settings: bool,
    folders: Vec<Folder>,
    new_folder: String,
    categories: Vec<String>,
    new_category: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            docked: false,
            search: String::new(),
            pick: Pick::All,
            settings: false,
            folders: Vec::new(),
            new_folder: String::new(),
            categories: ["Music", "Jingles", "Ads", "Beds", "Shows"]
                .iter()
                .map(|c| (*c).to_owned())
                .collect(),
            new_category: String::new(),
        }
    }
}

impl State {
    /// Show all files that match `text`.
    pub fn show_search(&mut self, text: &str) {
        text.clone_into(&mut self.search);
        self.pick = Pick::All;
    }

    pub fn remove_folder(&mut self, path: &str) {
        self.folders.retain(|f| f.path != path);
        if self.pick == Pick::Folder(path.to_owned()) {
            self.pick = Pick::All;
        }
    }

    /// Read a folder and add it. A failure shows next to the folder.
    fn add_folder(&mut self, path: &str, actions: &mut Vec<Action>) {
        let path = path.trim().to_owned();
        if path.is_empty() || self.folders.iter().any(|f| f.path == path) {
            return;
        }
        let files = folders::list_audio(Path::new(&path), false);
        let folder = Folder { path, files };
        if let Ok(files) = &folder.files {
            actions.push(Action::FolderFiles {
                name: folder.name().to_owned(),
                files: files.clone(),
            });
        }
        self.pick = Pick::Folder(folder.path.clone());
        self.folders.push(folder);
    }

    fn rescan(&mut self, path: &str, actions: &mut Vec<Action>) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.path == path) {
            f.files = folders::list_audio(Path::new(&f.path), false);
            if let Ok(files) = &f.files {
                actions.push(Action::FolderFiles {
                    name: f.name().to_owned(),
                    files: files.clone(),
                });
            }
        }
    }
}

/// The items that pass the tree and the search filter.
fn visible<'a>(view: &'a StationView, state: &State) -> Vec<&'a LibraryItem> {
    let needle = state.search.to_lowercase();
    view.library
        .iter()
        .filter(|i| match &state.pick {
            Pick::All => true,
            Pick::Category(c) => *c == i.category,
            Pick::Folder(p) => Path::new(&i.path).parent() == Some(Path::new(p)),
        })
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
        let dropped: Vec<std::path::PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            let (audio, _) = folders::expand_dropped(&dropped);
            actions.push(Action::Import(audio));
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
            library_settings(ui, state, actions, t)
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
    if let Pick::Folder(path) = state.pick.clone() {
        folder_header(ui, state, &path, actions, t);
    }
    ui.add_space(space::S);
    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(size::TREE_W, ui.available_height()), |ui| {
            ui.set_width(size::TREE_W);
            tree(ui, view, state, actions, t);
        });
        ui.separator();
        ui.vertical(|ui| table(ui, view, state, actions, t));
    });
}

/// The two folder actions, as menu rows.
fn folder_actions(ui: &mut egui::Ui, folder: &Folder, actions: &mut Vec<Action>) {
    let Ok(files) = &folder.files else {
        return;
    };
    for (label, main) in [
        ("Create playlist from folder", false),
        ("Use as main playlist", true),
    ] {
        if menu_item(ui, label, None) {
            actions.push(Action::FolderPlaylist {
                path: folder.path.clone(),
                files: files.clone(),
                main,
            });
        }
    }
}

fn folder_header(
    ui: &mut egui::Ui,
    state: &State,
    path: &str,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let Some(folder) = state.folders.iter().find(|f| f.path == path) else {
        return;
    };
    ui.horizontal(|ui| match &folder.files {
        Ok(files) => {
            let text = format!("📁 {} · {} audio files", folder.path, files.len());
            ui.label(RichText::new(text).font(theme::text(theme::LABEL)));
            for (label, main) in [
                ("Create playlist from folder", false),
                ("Use as main playlist", true),
            ] {
                if ui.button(label).clicked() {
                    actions.push(Action::FolderPlaylist {
                        path: folder.path.clone(),
                        files: files.clone(),
                        main,
                    });
                }
            }
        }
        Err(e) => {
            let text = format!("📁 {} · {e}", folder.path);
            ui.label(
                RichText::new(text)
                    .font(theme::text(theme::LABEL))
                    .color(t.danger),
            );
        }
    });
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
            let folder_names: Vec<String> =
                state.folders.iter().map(|f| f.name().to_owned()).collect();
            let mut cats = state.categories.clone();
            for item in &view.library {
                if !cats.contains(&item.category) && !folder_names.contains(&item.category) {
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
            let mut rescan = None;
            let mut remove = None;
            for folder in &state.folders {
                let on = state.pick == Pick::Folder(folder.path.clone());
                let (text, colour) = match &folder.files {
                    Ok(files) => (format!("📁 {} ({})", folder.name(), files.len()), t.text),
                    Err(_) => (format!("📁 {} (cannot read)", folder.name()), t.danger),
                };
                let r = ui.selectable_label(on, RichText::new(text).color(colour));
                if r.clicked() {
                    state.pick = Pick::Folder(folder.path.clone());
                }
                r.context_menu(|ui| {
                    folder_actions(ui, folder, actions);
                    if menu_item(ui, "Rescan", None) {
                        rescan = Some(folder.path.clone());
                    }
                    if context_menu::danger_item(ui, "Remove folder…", t) {
                        remove = Some(folder.path.clone());
                    }
                });
            }
            if let Some(p) = rescan {
                state.rescan(&p, actions);
            }
            if let Some(p) = remove {
                actions.push(Action::Confirm {
                    text: format!(
                        "Remove the folder {p} from the Library? The files stay on disk."
                    ),
                    then: Box::new(Action::RemoveFolder(p)),
                });
            }
        });
}

fn table(
    ui: &mut egui::Ui,
    view: &StationView,
    state: &State,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let items = visible(view, state);
    let head = |s: &str| {
        RichText::new(s)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    TableBuilder::new(ui)
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
        let artist = item.artist.clone().unwrap_or_else(|| "—".to_owned());
        let extra = [
            ("Artist", artist),
            ("Category", item.category.clone()),
            ("Path", item.path.clone()),
        ];
        actions.push(Action::Properties {
            title: item.title.clone(),
            fields: row_fields(&item.title, item.len, &extra),
        });
    }
}

fn library_settings(ui: &mut egui::Ui, state: &mut State, actions: &mut Vec<Action>, t: &Tokens) {
    settings::label(ui, "Folders (the preview reads only file names)", t);
    let mut remove = None;
    for f in &state.folders {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&f.path).font(theme::mono(theme::CODE)));
            if let Err(e) = &f.files {
                ui.label(
                    RichText::new(e)
                        .font(theme::text(theme::CAPTION))
                        .color(t.danger),
                );
            }
            if ui.small_button("Remove folder").clicked() {
                remove = Some(f.path.clone());
            }
        });
    }
    if let Some(p) = remove {
        state.remove_folder(&p);
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
    let mut remove = None;
    for (i, c) in state.categories.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(c).font(theme::mono(theme::CODE)));
            if ui.small_button("Remove category").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        state.categories.remove(i);
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.new_category).desired_width(size::FIELD_W));
        if ui.button("+ Add category").clicked() && !state.new_category.trim().is_empty() {
            let c = state.new_category.trim().to_owned();
            state.categories.push(c);
            state.new_category.clear();
        }
    });
}
