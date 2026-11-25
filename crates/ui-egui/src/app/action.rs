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
    /// A toast that shows for this many seconds, not the usual time.
    ToastFor(String, f64),
    /// Show this range of the as-played log in History, or the last hours for `None`.
    HistoryRange(Option<(jiff::Timestamp, jiff::Timestamp)>),
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
    /// Open Preferences on a page.
    OpenPreferences(crate::preferences::Page),
    /// Open the Keyboard Shortcuts dialog.
    OpenShortcuts,
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
    /// Open the dialog that adds or changes a one-off of Upcoming.
    OneOff(Box<crate::one_off::OneOffDialog>),
    /// Select every row of the Main list.
    SelectAllRows,
    /// Ask before removing every item of a playlist.
    ClearPlaylist(String),
    /// Count the duplicates of a playlist, then ask before removing them.
    RemoveDuplicates(String),
    /// Select a timeline row in Main and Upcoming, and scroll both to it.
    SelectRow(u64),
    /// Make a playlist from a folder of the library, and maybe make it main.
    FolderPlaylist {
        path: String,
        main: bool,
    },
}

/// The message after a successful edit that changed `n` items.
fn describe(edit: &Edit, n: usize, preview: bool) -> String {
    if preview {
        describe_preview(edit, n)
    } else {
        describe_real(edit, n)
    }
}

/// The message after an edit of a real station.
fn describe_real(edit: &Edit, n: usize) -> String {
    let deck_name = |d: usize| ['A', 'B', 'C', 'D'].get(d).copied().unwrap_or('?');
    match edit {
        Edit::Insert { .. } => format!("Added {n} items"),
        Edit::Remove(_) => format!("Removed {n} rows"),
        Edit::Move { .. } => format!("Moved {n} rows"),
        Edit::ToggleLock(_) => format!("Changed the lock of {n} rows"),
        Edit::RenamePlaylist { from, to } => format!("Renamed {from} to {to}"),
        Edit::DuplicatePlaylist(name) => format!("Made a copy of {name}"),
        Edit::NewPlaylist(_) => "Made a new playlist".to_owned(),
        Edit::DeletePlaylist(name) => format!("Deleted {name}"),
        Edit::RemovePlaylistItems { playlist, .. } => format!("Removed {n} items from {playlist}"),
        Edit::RemoveLibraryItems(_) => format!("Removed {n} items from the Library"),
        Edit::MovePlaylistItem { .. } => format!("Moved {n} items"),
        Edit::SetMeta(_) => format!("Changed the tags of {n} items; tags are not written to files"),
        Edit::Deck { deck, command } => {
            let name = deck_name(*deck);
            match command {
                DeckCommand::PlayPause => format!("Deck {name} plays or pauses"),
                DeckCommand::Play(_) => format!("Deck {name} is on air"),
                DeckCommand::Fade(_) => format!("Deck {name} fades out, then the next item starts"),
                DeckCommand::Seek(to) => {
                    format!("Deck {name} is at {}", crate::app::position(*to))
                }
            }
        }
        Edit::PlayNext { .. } => "The next deck is on air".to_owned(),
        Edit::AssignCart {
            cart,
            item: Some(_),
        } => format!("Assigned cart {}", cart + 1),
        Edit::AssignCart { cart, item: None } => format!("Cleared cart {}", cart + 1),
        Edit::SetTiming { .. } => "Changed the timing".to_owned(),
        Edit::SetPlaylistColour { playlist, colour } => {
            let name = crate::theme::PALETTE.get(*colour).map_or("?", |c| c.0);
            format!("{playlist} is {name} now")
        }
        Edit::SetPlaylistEnabled { playlist, enabled } => {
            let state = if *enabled { "on" } else { "off" };
            format!("{playlist} is {state}")
        }
        Edit::SetWeight { weight, .. } => format!("Set the weight to ×{weight}"),
        Edit::SetStrip { strip, .. } => format!("Changed strip {strip}"),
        Edit::SetIcecast(ice) => format!(
            "Applied the mount {}:{}{}; it connects now while on air",
            ice.host, ice.port, ice.mount
        ),
        Edit::Pair { address, .. } => {
            format!(
                "Asking {} to pair; the Server page says how it went",
                address.trim()
            )
        }
        Edit::Unpair => "Unpaired from the server; its key left the keychain".to_owned(),
        Edit::SetStreaming(true) => "Streaming to listeners starts".to_owned(),
        Edit::SetStreaming(false) => "Streaming to listeners stopped".to_owned(),
        Edit::SetDynamics(_) => "Changed the processing on Program".to_owned(),
        Edit::SetLoudness(_) => "Changed the loudness normalisation".to_owned(),
        Edit::SetDevice { device, .. } => {
            format!("Picked {device}; the app uses it from its next start")
        }
        Edit::AddCartPage { name } => format!("Added the page {name}"),
        Edit::RenameCartPage { name, .. } => format!("Renamed the page to {name}"),
        Edit::RemoveCartPage { .. } => "Removed the page".to_owned(),
        Edit::SetCart { cart, .. } => format!("Changed cart {}", cart + 1),
        Edit::FireCart { cart } => format!("Fired cart {}", cart + 1),
        Edit::RemoveDuplicates { playlist } => format!("Removed {n} duplicates from {playlist}"),
        Edit::ClearPlaylist { playlist, .. } => format!("Removed {n} items from {playlist}"),
        Edit::StopCarts => "Stopped all carts".to_owned(),
        Edit::AddFolder { path } => format!("Reading the audio files of {path}"),
        Edit::RemoveFolder { path } => format!("Removed {path} from the Library"),
        Edit::Rescan { folder } => format!("Reading the audio files of {folder} again"),
        Edit::SetPartSetting { part, key, .. } => format!("Changed {part} {key}"),
        Edit::Undo => "Undid the last edit".to_owned(),
        Edit::Redo => "Did the edit again".to_owned(),
        Edit::SetMode(mode) => format!("Mode is {}", mode.label()),
        Edit::StopAfterCurrent(true) => "The station stops after the item on air".to_owned(),
        Edit::StopAfterCurrent(false) => "The station plays on after the item on air".to_owned(),
        Edit::AddFiles { playlist, .. } | Edit::AddFolderItems { playlist, .. } => {
            format!("Adding to {playlist}…")
        }
        Edit::AddStream { playlist, .. } => format!("Added {n} stream to {playlist}"),
        Edit::AddLive { playlist, .. } => format!("Added {n} live input to {playlist}"),
        Edit::AckAlarm(_) | Edit::AckAlarms(_) => {
            format!("Acknowledged {n} alarms; they no longer count as new")
        }
        Edit::ExportAlarms { path, ids } => format!("Writing {} alarms to {path}…", ids.len()),
        Edit::ExportHistory { path, report, .. } => {
            format!("Writing the {} to {path}…", report.label())
        }
        Edit::ScheduleOneOff(o) if o.id == 0 => format!("Added a one-off at {}", o.start),
        Edit::ScheduleOneOff(o) => format!("Changed the one-off at {}", o.start),
        Edit::RemoveOneOff(_) => "Removed the one-off; Undo brings it back".to_owned(),
    }
}

/// The message after an edit of the preview, which saves nothing.
fn describe_preview(edit: &Edit, n: usize) -> String {
    let what = match edit {
        Edit::Insert { .. } => format!("added {n} items"),
        Edit::Remove(_) => format!("Removed {n} rows"),
        Edit::Move { .. } => format!("moved {n} rows"),
        Edit::ToggleLock(_) => format!("changed the lock of {n} rows"),
        Edit::RenamePlaylist { from, to } => format!("renamed {from} to {to}"),
        Edit::DuplicatePlaylist(name) => format!("made a copy of {name}"),
        Edit::NewPlaylist(_) => "made a new playlist (not saved)".to_owned(),
        Edit::DeletePlaylist(name) => format!("deleted {name}"),
        Edit::RemovePlaylistItems { playlist, .. } => format!("removed {n} items from {playlist}"),
        Edit::RemoveLibraryItems(_) => format!("removed {n} items from the Library"),
        Edit::MovePlaylistItem { .. } => format!("moved {n} items"),
        Edit::SetMeta(_) => format!("changed the tags of {n} items; tags are not written to files"),
        Edit::Deck { deck, command } => {
            let name = ['A', 'B', 'C', 'D'].get(*deck).copied().unwrap_or('?');
            match command {
                DeckCommand::PlayPause => {
                    format!("deck {name} paused or plays again here only; the timeline keeps time")
                }
                DeckCommand::Play(_) => format!("deck {name} is on air"),
                DeckCommand::Fade(s) => {
                    format!("deck {name} fades out over {s:.1} s, then the next item starts")
                }
                DeckCommand::Seek(to) => {
                    let at = crate::app::position(*to);
                    format!("deck {name} is at {at} here only; the timeline keeps time")
                }
            }
        }
        Edit::PlayNext { .. } => "the next item is on air".to_owned(),
        Edit::AssignCart {
            cart,
            item: Some(_),
        } => format!("assigned cart {}", cart + 1),
        Edit::AssignCart { cart, item: None } => format!("cleared cart {}", cart + 1),
        Edit::SetTiming { .. } => "changed the timing; the scheduler is not connected".to_owned(),
        Edit::SetPlaylistColour { playlist, colour } => {
            let name = crate::theme::PALETTE.get(*colour).map_or("?", |c| c.0);
            format!("{playlist} is {name} now (not saved)")
        }
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
        Edit::Pair { .. } => "asked nothing · the preview reaches no server".to_owned(),
        Edit::Unpair => "nothing to unpair · the preview is not paired".to_owned(),
        Edit::SetStreaming(on) => format!(
            "turned streaming {} · the preview sends no audio",
            if *on { "on" } else { "off" }
        ),
        Edit::SetDynamics(_) => "changed the processing · nothing plays here".to_owned(),
        Edit::SetLoudness(_) => "changed the loudness · nothing plays here".to_owned(),
        Edit::SetDevice { device, .. } => format!("picked {device}"),
        Edit::AddCartPage { name } => format!("added the page {name}"),
        Edit::RenameCartPage { name, .. } => format!("renamed the page to {name}"),
        Edit::RemoveCartPage { .. } => "removed the page".to_owned(),
        Edit::SetCart { cart, .. } => format!("changed cart {}", cart + 1),
        Edit::FireCart { cart } => format!("cart {} flashes; no audio", cart + 1),
        Edit::RemoveDuplicates { playlist } => format!("removed {n} duplicates from {playlist}"),
        Edit::ClearPlaylist { playlist, .. } => format!("removed {n} items from {playlist}"),
        Edit::StopCarts => "stopped all carts; no audio".to_owned(),
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
        Edit::AckAlarm(_) | Edit::AckAlarms(_) => format!("acknowledged {n} alarms"),
        Edit::ExportAlarms { path, .. } => format!("wrote {n} alarms to {path}"),
        Edit::ExportHistory { path, report, .. } => {
            format!("wrote the {} to {path}", report.label())
        }
        Edit::ScheduleOneOff(o) => format!("put a one-off at {}; nothing plays here", o.start),
        Edit::RemoveOneOff(_) => "removed the one-off".to_owned(),
    };
    format!("preview — {what} (not saved)")
}

impl RadioApp {
    /// The message after a change that worked: the preview says that it saves nothing.
    pub(super) fn done(&self, what: String) -> String {
        if self.view.preview {
            format!("preview — {what} (not saved)")
        } else {
            let mut c = what.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }

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
        // A display preference of this computer goes to its preferences, not to the station.
        if let Edit::SetPartSetting { part, key, value } = &edit
            && crate::app_prefs::is_display(part, key)
        {
            self.prefs.set(part, key, value.clone());
            self.refresh(true);
            return;
        }
        match self.source.edit(edit.clone()) {
            Ok(n) if loud => {
                self.toast(ctx, describe(&edit, n, self.view.preview));
                // A remove offers Undo in the toast, next to what it removed.
                self.toast_undo = matches!(
                    edit,
                    Edit::Remove(_)
                        | Edit::RemovePlaylistItems { .. }
                        | Edit::RemoveLibraryItems(_)
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
                    Ok(n) => self.done(format!("added {n} items to {place}")),
                    Err(e) => e,
                };
                self.toast(ctx, text);
            }
            Action::Import(paths) => {
                let text = match self.source.import_files(paths, "Unsorted") {
                    // A source that reads in the background reports the files later.
                    Ok(i) if i.ids.is_empty() && i.skipped == 0 => {
                        "Reading the files for the library…".to_owned()
                    }
                    Ok(i) => format!(
                        "{}{}",
                        self.done(format!("added {} files to Unsorted", i.ids.len())),
                        drops::skipped_note(i.skipped)
                    ),
                    Err(e) => e,
                };
                self.toast(ctx, text);
            }
            Action::Edit(edit) => self.run_edit(ctx, edit, true),
            Action::EditQuiet(edit) => self.run_edit(ctx, edit, false),
            Action::Toast(text) => self.toast(ctx, text),
            Action::ToastFor(text, secs) => self.toast_for(ctx, text, secs),
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
                    Ok(n) => self.done(format!("added {n} copies")),
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
                    let mut editor = Editor::new(&tab);
                    editor.preview = self.view.preview;
                    self.editor = Some(editor);
                }
            }
            Action::OpenShortcuts => {
                self.shortcuts_dialog = Some(craft_keymap::ui::DialogState::new());
            }
            Action::HistoryRange(range) => self.source.history_range(range),
            Action::OpenPreferences(page) => {
                self.preferences.open_at(page);
                self.open.set(ToolWindow::Preferences, true);
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
            Action::OneOff(d) => self.one_off = Some(*d),
            Action::ClearPlaylist(name) => {
                let tab = self
                    .view
                    .tabs
                    .iter()
                    .chain(std::iter::once(&self.view.main))
                    .find(|t| t.name == name);
                if let Some(tab) = tab {
                    let folder = tab.source.as_ref().is_some_and(|s| s.watch);
                    self.clear = Some((name, tab.items.len(), true, folder));
                }
            }
            Action::SelectRow(id) => {
                if self.view.rows.iter().any(|r| r.id == id) {
                    self.link.selected = vec![id];
                    self.link.anchor = Some(id);
                    self.link.focus = Pane::Main;
                    self.link.follow = false;
                    self.link.scroll_main = Some(id);
                    self.link.scroll_scheduled = Some(id);
                } else {
                    self.toast(ctx, "That item is not in the timeline any more".to_owned());
                }
            }
            Action::SelectAllRows => {
                self.link.selected = self.view.rows.iter().map(|r| r.id).collect();
                self.link.focus = Pane::Main;
            }
            Action::RemoveDuplicates(name) => {
                let n = self.source.duplicates(&name);
                if n == 0 {
                    self.toast(ctx, format!("{name} has no duplicate items"));
                } else {
                    self.confirm = Some(Confirm {
                        text: format!(
                            "Remove {n} duplicate items from {name}? The first item of each file stays. You can undo it."
                        ),
                        yes: "Remove duplicates".to_owned(),
                        then: Action::Edit(Edit::RemoveDuplicates { playlist: name }),
                    });
                }
            }
            Action::UseAsMain(name) => {
                let text = match self.source.use_as_main(&name) {
                    Ok(()) => self.done(format!("{name} is the main playlist now")),
                    Err(e) => e,
                };
                self.toast(ctx, text);
            }
            Action::FolderPlaylist { path, main } => {
                let folder = FolderSource {
                    path,
                    recursive: true,
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
                        let text = if n == 0 {
                            format!("Reading the folder for the playlist {name}…")
                        } else {
                            self.done(format!("made playlist {name} from {n} files"))
                        };
                        self.toast(ctx, text);
                    }
                    Err(e) => self.toast(ctx, e),
                }
            }
        }
    }
}
