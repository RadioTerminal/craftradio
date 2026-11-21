//! The media library: a category tree, a search box and a table of files.
//! A person drags a title onto the main pane, a playlist tab or a cart.
//! Files from the file manager drop onto the library as "Unsorted".

use egui::{Align, Layout, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, mmss};
use crate::settings;
use crate::theme::{self, Tokens, size, space};
use crate::view::{LibraryItem, StationView};

/// The drag payload: the id of a library item.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LibraryDrag(pub u64);

/// The library state. It is local to the window.
pub(crate) struct State {
    pub docked: bool,
    search: String,
    category: Option<String>,
    settings: bool,
    folders: Vec<String>,
    new_folder: String,
    categories: Vec<String>,
    new_category: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            docked: false,
            search: String::new(),
            category: None,
            settings: false,
            folders: vec!["~/Music/Station".to_owned(), "~/Music/Jingles".to_owned()],
            new_folder: String::new(),
            categories: ["Music", "Jingles", "Ads", "Beds", "Shows"]
                .iter()
                .map(|c| (*c).to_owned())
                .collect(),
            new_category: String::new(),
        }
    }
}

/// The items that pass the category and the search filter.
fn visible<'a>(view: &'a StationView, state: &State) -> Vec<&'a LibraryItem> {
    let needle = state.search.to_lowercase();
    view.library
        .iter()
        .filter(|i| state.category.as_ref().is_none_or(|c| *c == i.category))
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
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let dropped: Vec<String> = ui.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().display().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    });
    if !dropped.is_empty() {
        actions.push(Action::Import(dropped));
    }

    let mut open = state.settings;
    let mut docked = state.docked;
    settings::header(ui, "Library", &mut open, |ui| {
        let label = if docked {
            "Undock to a window"
        } else {
            "Dock into main window"
        };
        if ui.selectable_label(docked, label).clicked() {
            docked = !docked;
        }
    });
    state.settings = open;
    state.docked = docked;
    if state.settings {
        settings::section(ui, "Library", t, |ui| library_settings(ui, state, t));
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
    ui.add_space(space::S);
    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(size::TREE_W, ui.available_height()), |ui| {
            ui.set_width(size::TREE_W);
            tree(ui, view, state);
        });
        ui.separator();
        ui.vertical(|ui| table(ui, view, state, t));
    });
}

fn tree(ui: &mut egui::Ui, view: &StationView, state: &mut State) {
    egui::CollapsingHeader::new("Folders")
        .default_open(true)
        .show(ui, |ui| {
            if ui
                .selectable_label(state.category.is_none(), "All files")
                .clicked()
            {
                state.category = None;
            }
            let mut cats = state.categories.clone();
            for item in &view.library {
                if !cats.contains(&item.category) {
                    cats.push(item.category.clone());
                }
            }
            for c in cats {
                let n = view.library.iter().filter(|i| i.category == c).count();
                let on = state.category.as_deref() == Some(c.as_str());
                if ui.selectable_label(on, format!("{c} ({n})")).clicked() {
                    state.category = Some(c);
                }
            }
        });
}

fn table(ui: &mut egui::Ui, view: &StationView, state: &State, t: &Tokens) {
    let items = visible(view, state);
    let head = |s: &str| {
        RichText::new(s)
            .font(theme::text(theme::CAPTION))
            .color(t.text_muted)
    };
    TableBuilder::new(ui)
        .id_salt("library_table")
        .striped(true)
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
                    let id = egui::Id::new(("library_item", item.id));
                    ui.dnd_drag_source(id, LibraryDrag(item.id), |ui| {
                        let text = RichText::new(format!("☰ {}", item.title))
                            .font(theme::text(theme::SMALL));
                        ui.add(egui::Label::new(text).truncate().selectable(false));
                    });
                });
                row.col(|ui| {
                    let artist = item.artist.as_deref().unwrap_or("—");
                    ui.add(
                        egui::Label::new(RichText::new(artist).font(theme::text(theme::SMALL)))
                            .truncate(),
                    );
                });
                row.col(|ui| {
                    let len = if item.len.is_zero() {
                        "—".to_owned()
                    } else {
                        mmss(item.len)
                    };
                    ui.label(
                        RichText::new(len)
                            .font(theme::mono(theme::CODE))
                            .color(t.text_muted),
                    );
                });
                row.col(|ui| {
                    ui.label(RichText::new(&item.category).font(theme::text(theme::CAPTION)));
                });
                row.col(|ui| {
                    let path = RichText::new(&item.path)
                        .font(theme::mono(theme::CAPTION))
                        .color(t.text_muted);
                    ui.add(egui::Label::new(path).truncate());
                });
            });
        });
}

fn library_settings(ui: &mut egui::Ui, state: &mut State, t: &Tokens) {
    settings::label(
        ui,
        "Watched folders (the preview does not read the disk)",
        t,
    );
    list_editor(ui, "folder", &mut state.folders, &mut state.new_folder);
    ui.add_space(space::S);
    settings::label(ui, "Categories", t);
    list_editor(
        ui,
        "category",
        &mut state.categories,
        &mut state.new_category,
    );
}

fn list_editor(ui: &mut egui::Ui, what: &str, list: &mut Vec<String>, new: &mut String) {
    let mut remove = None;
    for (i, entry) in list.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(entry).font(theme::mono(theme::CODE)));
            if ui.small_button(format!("Remove {what}")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        list.remove(i);
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(new).desired_width(size::FIELD_W));
        if ui.button(format!("+ Add {what}")).clicked() && !new.trim().is_empty() {
            list.push(new.trim().to_owned());
            new.clear();
        }
    });
}
