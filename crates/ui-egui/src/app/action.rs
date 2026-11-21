//! The actions that panels ask for, and how the app runs them.

use super::*;

/// Where a paste goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PasteTo {
    Main(Place),
    Playlist(String),
    Cart(usize),
}

/// Where files from the file manager go when a person drops them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileTarget {
    Main(Place),
    Playlist(String),
    Editor,
    Library,
}

/// A change that a panel asks for. The app passes it to the view source or keeps it locally.
pub(crate) enum Action {
    /// A library item dropped on a target.
    Drop {
        item: u64,
        target: DropTarget,
    },
    /// Files dropped from the file manager onto the library.
    Import(Vec<String>),
    Edit(Edit),
    Toast(String),
    /// Copy library items.
    CopyItems(Vec<u64>),
    /// Copy timeline rows or playlist items by title. With `cut`, a paste removes `rows`.
    CopyTitles {
        titles: Vec<(String, Duration)>,
        rows: Vec<u64>,
        cut: bool,
    },
    Paste(PasteTo),
    /// Insert copies of these titles after the row `after`.
    Duplicate {
        titles: Vec<(String, Duration)>,
        after: Place,
    },
    OpenRules(String),
    OpenWindow(ToolWindow),
    ShowInLibrary(String),
    /// Open the metadata editor for these items.
    Metadata(Vec<MetaTarget>),
    /// Open the metadata editor for a playlist item, by title.
    MetadataTitle(String, Duration),
    Confirm {
        text: String,
        then: Box<Action>,
    },
    Rename(String),
    UseAsMain(String),
    FolderPlaylist {
        path: String,
        files: Vec<String>,
        main: bool,
    },
    FolderFiles {
        name: String,
        files: Vec<String>,
    },
    RemoveFolder(String),
}

impl RadioApp {
    /// Pass the actions of this frame on.
    pub(super) fn run(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        for action in actions {
            self.act(ctx, action);
        }
        self.refresh(true);
    }

    pub(super) fn act(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Drop { item, target } => {
                let title = self
                    .view
                    .library
                    .iter()
                    .find(|l| l.id == item)
                    .map_or_else(|| "item".to_owned(), |l| l.title.clone());
                let place = match &target {
                    DropTarget::Main { .. } => self.view.main_playlist.clone(),
                    DropTarget::Playlist(name) => name.clone(),
                    DropTarget::Cart(i) => format!("cart {}", i + 1),
                };
                let text = if self.source.drop_item(item, target) {
                    format!("preview — added {title} to {place} (not saved)")
                } else {
                    format!("preview — {title} does not fit in {place}")
                };
                self.toast(ctx, text);
            }
            Action::Import(paths) => {
                let n = paths.len();
                self.source.import_files(paths, "Unsorted");
                self.toast(
                    ctx,
                    format!("preview — added {n} files to Unsorted (not saved)"),
                );
            }
            Action::Edit(edit) => {
                let result = self.source.edit(edit);
                self.report(ctx, result);
            }
            Action::Toast(text) => self.toast(ctx, text),
            Action::CopyItems(items) => {
                let n = items.len();
                self.clip = Clipboard {
                    items,
                    cut: false,
                    rows: Vec::new(),
                };
                self.toast(ctx, format!("copied {n} items"));
            }
            Action::CopyTitles { titles, rows, cut } => {
                let items: Vec<u64> = titles
                    .iter()
                    .filter_map(|(t, l)| self.source.library_ref(t, *l))
                    .collect();
                let n = items.len();
                self.clip = Clipboard { items, cut, rows };
                let verb = if cut { "cut" } else { "copied" };
                self.toast(ctx, format!("{verb} {n} items"));
            }
            Action::Paste(to) => self.paste(ctx, to),
            Action::Duplicate { titles, after } => {
                let items: Vec<u64> = titles
                    .iter()
                    .filter_map(|(t, l)| self.source.library_ref(t, *l))
                    .collect();
                let result = self.insert_items(&items, after);
                self.report(ctx, result);
            }
            Action::OpenRules(name) => {
                let tab = if self.view.main.name == name {
                    Some(self.view.main.clone())
                } else {
                    self.view.tabs.iter().find(|t| t.name == name).cloned()
                };
                if let Some(tab) = tab {
                    self.editor = Some(Editor::new(&tab));
                }
            }
            Action::OpenWindow(w) => {
                if w == ToolWindow::Library && self.library.docked {
                    return;
                }
                self.open.set(w, true);
            }
            Action::ShowInLibrary(title) => {
                let short = title.split(" – ").last().unwrap_or(&title).to_owned();
                self.library.show_search(&short);
                if !self.library.docked {
                    self.open.set(ToolWindow::Library, true);
                }
            }
            Action::Metadata(targets) => self.open_meta(targets),
            Action::MetadataTitle(title, len) => {
                if let Some(id) = self.source.library_ref(&title, len) {
                    self.refresh(true);
                    self.open_meta(vec![MetaTarget::Library(id)]);
                }
            }
            Action::Confirm { text, then } => self.confirm = Some((text, *then)),
            Action::Rename(name) => self.rename = Some((name.clone(), name)),
            Action::UseAsMain(name) => {
                let text = if self.source.use_as_main(&name) {
                    format!("preview — {name} is the main playlist now (not saved)")
                } else {
                    format!("preview — {name} has no items to play")
                };
                self.toast(ctx, text);
            }
            Action::FolderPlaylist { path, files, main } => {
                let n = files.len();
                let folder = FolderSource {
                    path,
                    recursive: false,
                    watch: false,
                };
                match self.source.playlist_from_folder(folder, files) {
                    Some(name) if main => self.act(ctx, Action::UseAsMain(name)),
                    Some(name) => self.toast(
                        ctx,
                        format!("preview — made playlist {name} from {n} files (not saved)"),
                    ),
                    None => self.toast(
                        ctx,
                        "preview — this source cannot make playlists".to_owned(),
                    ),
                }
            }
            Action::FolderFiles { name, files } => {
                self.source.import_files(files, &name);
            }
            Action::RemoveFolder(path) => {
                self.library.remove_folder(&path);
                self.toast(
                    ctx,
                    format!("preview — removed {path} from the Library (not saved)"),
                );
            }
        }
    }
}
