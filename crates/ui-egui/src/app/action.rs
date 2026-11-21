//! The actions that panels ask for, and how the app runs them.
//!
//! Every message shows the count that the source returned, or the error of the source.

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

/// A change that a panel asks for. The app passes it to the view source.
pub(crate) enum Action {
    /// A library item dropped on a target.
    Drop {
        item: u64,
        target: DropTarget,
    },
    /// Files dropped from the file manager onto the library window.
    Import(Vec<String>),
    /// An edit with a message after it.
    Edit(Edit),
    /// An edit that shows a message only when it fails, for faders and settings.
    EditQuiet(Edit),
    Toast(String),
    /// Copy library items.
    CopyItems(Vec<u64>),
    /// Copy timeline rows or playlist items by title. With `cut`, a paste removes the source.
    CopyTitles {
        titles: Vec<(String, Duration)>,
        cut: Option<CutSource>,
    },
    Paste(PasteTo),
    /// Insert copies of these titles at `after`.
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
        /// The label of the button that goes ahead, such as "Remove".
        yes: String,
        then: Box<Action>,
    },
    Rename(String),
    UseAsMain(String),
    /// Open a dialog that adds a stream or the live input.
    AddDialog(AddDialog),
    /// Make a playlist from a folder of the library, and maybe make it main.
    FolderPlaylist {
        path: String,
        main: bool,
    },
}

/// The message after a successful edit that changed `n` items.
fn describe(edit: &Edit, n: usize) -> String {
    let what = match edit {
        Edit::Insert { .. } => format!("added {n} items"),
        Edit::Remove(_) => format!("Removed {n} rows"),
        Edit::Move { .. } => format!("moved {n} rows"),
        Edit::ToggleLock(_) => format!("changed the lock of {n} rows"),
        Edit::RenamePlaylist { from, to } => format!("renamed {from} to {to}"),
        Edit::DuplicatePlaylist(name) => format!("made a copy of {name}"),
        Edit::DeletePlaylist(name) => format!("deleted {name}"),
        Edit::RemovePlaylistItem { playlist, .. } => format!("removed {n} items from {playlist}"),
        Edit::MovePlaylistItem { .. } => format!("moved {n} items"),
        Edit::SetMeta(_) => format!("changed the tags of {n} items; tags are not written to files"),
        Edit::Deck { deck, command } => {
            let name = ['A', 'B', 'C', 'D'].get(*deck).copied().unwrap_or('?');
            match command {
                DeckCommand::PlayPause => {
                    format!("deck {name} paused or plays again here only; the timeline keeps time")
                }
                DeckCommand::Fade(s) => {
                    format!("deck {name} fades out over {s:.1} s, then the next item starts")
                }
                DeckCommand::Skip => format!("deck {name} skipped to the next item"),
            }
        }
        Edit::AssignCart {
            cart,
            item: Some(_),
        } => format!("assigned cart {}", cart + 1),
        Edit::AssignCart { cart, item: None } => format!("cleared cart {}", cart + 1),
        Edit::SetTiming { .. } => "changed the timing; the scheduler is not connected".to_owned(),
        Edit::SetPlaylistEnabled { playlist, enabled } => {
            let state = if *enabled { "on" } else { "off" };
            format!("{playlist} is {state}; the scheduler is not connected")
        }
        Edit::SetWeight { weight, .. } => format!("set the weight to ×{weight}"),
        Edit::SetStrip { strip, .. } => format!("changed strip {strip}"),
        Edit::SetIcecast(ice) => format!(
            "saved the mount {}:{}{} · Icecast stays not connected",
            ice.host, ice.port, ice.mount
        ),
        Edit::SetDevice { device, .. } => format!("picked {device}"),
        Edit::AddCartPage { name } => format!("added the page {name}"),
        Edit::RenameCartPage { name, .. } => format!("renamed the page to {name}"),
        Edit::RemoveCartPage { .. } => "removed the page".to_owned(),
        Edit::SetCart { cart, .. } => format!("changed cart {}", cart + 1),
        Edit::AddFolder { path } => format!("read {n} audio files from {path}"),
        Edit::RemoveFolder { path } => format!("removed {path} from the Library"),
        Edit::Rescan { folder } => format!("read {n} audio files from {folder}"),
        Edit::SetPartSetting { part, key, .. } => format!("changed {part} {key}"),
        Edit::Undo => "undid the last edit".to_owned(),
        Edit::Redo => "did the edit again".to_owned(),
        Edit::SetMode(mode) => format!(
            "mode is {} here only; the scheduler is not connected",
            mode.label()
        ),
        Edit::StopAfterCurrent(true) => {
            "the station stops after the item on air (here only)".to_owned()
        }
        Edit::StopAfterCurrent(false) => "the station plays on after the item on air".to_owned(),
        Edit::AddFiles { playlist, .. } | Edit::AddFolderItems { playlist, .. } => {
            format!("added {n} files to {playlist}")
        }
        Edit::AddStream { playlist, .. } => format!("added {n} stream to {playlist}"),
        Edit::AddLive { playlist, .. } => format!("added {n} live input to {playlist}"),
    };
    format!("preview — {what} (not saved)")
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

    /// Run an edit. A loud edit shows its message; a quiet edit shows only an error.
    pub(super) fn run_edit(&mut self, ctx: &egui::Context, edit: Edit, loud: bool) {
        match self.source.edit(edit.clone()) {
            Ok(n) if loud => {
                self.toast(ctx, describe(&edit, n));
                // A remove offers Undo in the toast, next to what it removed.
                self.toast_undo = matches!(
                    edit,
                    Edit::Remove(_)
                        | Edit::RemovePlaylistItem { .. }
                        | Edit::DeletePlaylist(_)
                        | Edit::RemoveCartPage { .. }
                );
            }
            Ok(_) => {}
            Err(e) => self.toast(ctx, e),
        }
    }

    pub(super) fn act(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Drop { item, target } => {
                let place = match &target {
                    DropTarget::Main { .. } => self.view.main_playlist.clone(),
                    DropTarget::Playlist(name) => name.clone(),
                    DropTarget::Cart(i) => format!("cart {}", i + 1),
                };
                let text = match self.source.drop_item(item, target) {
                    Ok(n) => format!("preview — added {n} items to {place} (not saved)"),
                    Err(e) => e,
                };
                self.toast(ctx, text);
            }
            Action::Import(paths) => {
                let text = match self.source.import_files(paths, "Unsorted") {
                    Ok(i) => format!(
                        "preview — added {} files to Unsorted (not saved){}",
                        i.ids.len(),
                        drops::skipped_note(i.skipped)
                    ),
                    Err(e) => e,
                };
                self.toast(ctx, text);
            }
            Action::Edit(edit) => self.run_edit(ctx, edit, true),
            Action::EditQuiet(edit) => self.run_edit(ctx, edit, false),
            Action::Toast(text) => self.toast(ctx, text),
            Action::CopyItems(items) => {
                let n = items.len();
                self.clip = Clipboard {
                    items,
                    cut: false,
                    source: None,
                };
                self.toast(ctx, format!("copied {n} items"));
            }
            Action::CopyTitles { titles, cut } => {
                let items: Vec<u64> = titles
                    .iter()
                    .filter_map(|(t, l)| self.source.library_ref(t, *l))
                    .collect();
                let n = items.len();
                let verb = if cut.is_some() { "cut" } else { "copied" };
                self.clip = Clipboard {
                    items,
                    cut: cut.is_some(),
                    source: cut,
                };
                self.toast(ctx, format!("{verb} {n} items"));
            }
            Action::Paste(to) => self.paste(ctx, to),
            Action::Duplicate { titles, after } => {
                let items: Vec<u64> = titles
                    .iter()
                    .filter_map(|(t, l)| self.source.library_ref(t, *l))
                    .collect();
                let text = match self.insert_items(&items, after) {
                    Ok(n) => format!("preview — added {n} copies (not saved)"),
                    Err(e) => e,
                };
                self.toast(ctx, text);
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
            Action::Confirm { text, yes, then } => {
                self.confirm = Some(Confirm {
                    text,
                    yes,
                    then: *then,
                })
            }
            Action::Rename(name) => self.rename = Some((name.clone(), name)),
            Action::AddDialog(d) => self.add_dialog = Some(d),
            Action::UseAsMain(name) => {
                let text = match self.source.use_as_main(&name) {
                    Ok(()) => format!("preview — {name} is the main playlist now (not saved)"),
                    Err(e) => e,
                };
                self.toast(ctx, text);
            }
            Action::FolderPlaylist { path, main } => {
                let folder = FolderSource {
                    path,
                    recursive: false,
                    watch: false,
                };
                match self.source.playlist_from_folder(folder) {
                    Ok(name) if main => self.act(ctx, Action::UseAsMain(name)),
                    Ok(name) => {
                        self.refresh(true);
                        let n = self
                            .view
                            .tabs
                            .iter()
                            .find(|t| t.name == name)
                            .map_or(0, |t| t.items.len());
                        self.toast(
                            ctx,
                            format!("preview — made playlist {name} from {n} files (not saved)"),
                        );
                    }
                    Err(e) => self.toast(ctx, e),
                }
            }
        }
    }
}
