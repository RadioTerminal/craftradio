//! The main window: decks on top, the main and scheduled panes in the middle, a status line below.
//! Tool windows (cartwall, mixer, clock, history, library) open from the deck strip.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use egui::RichText;
use jiff::{SignedDuration, Timestamp};

use crate::rules_editor::{self, Editor};
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DropTarget, Edit, FolderSource, Place, Row, StationView, ViewSource};
use crate::windows::{self, ToolWindow};
use crate::{
    cartwall, clock_window, decks, folders, history, library, main_pane, mixer, scheduled_pane,
};

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
    Properties {
        title: String,
        fields: Vec<(String, String)>,
    },
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

/// The internal clipboard: library item ids. `rows` are the rows that a cut removes on paste.
#[derive(Default)]
pub(crate) struct Clipboard {
    pub items: Vec<u64>,
    pub cut: bool,
    pub rows: Vec<u64>,
}

/// The pane that has the keyboard, for the edit shortcuts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pane {
    #[default]
    Main,
    Scheduled,
}

/// A timing change that a person made in the preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Timing {
    pub hard: bool,
    pub tolerance_s: f32,
}

/// State that the panes share: the selection, scroll requests and local preview changes.
#[derive(Default)]
pub(crate) struct Linked {
    pub selected: Vec<u64>,
    pub anchor: Option<u64>,
    pub focus: Pane,
    /// The main pane scrolls to this row once, then clears it.
    pub scroll_main: Option<u64>,
    /// The scheduled pane scrolls to this row once, then clears it.
    pub scroll_scheduled: Option<u64>,
    pub has_clip: bool,
    pub timing: HashMap<u64, Timing>,
    pub disabled: HashSet<String>,
    pub weights: HashMap<(String, usize), u32>,
    /// The drop target under the pointer in this frame, while files hover.
    pub file_target: Option<FileTarget>,
    /// The last drop target that the pointer was over.
    pub last_file_target: Option<FileTarget>,
}

impl Linked {
    pub fn is_selected(&self, id: u64) -> bool {
        self.selected.contains(&id)
    }

    /// Select after a click. Command adds or removes one row; shift selects a range of `order`.
    pub fn click(&mut self, pane: Pane, id: u64, m: egui::Modifiers, order: &[u64]) {
        self.focus = pane;
        if m.command {
            if let Some(i) = self.selected.iter().position(|s| *s == id) {
                self.selected.remove(i);
            } else {
                self.selected.push(id);
            }
            self.anchor = Some(id);
            return;
        }
        if m.shift {
            let a = self.anchor.and_then(|a| order.iter().position(|o| *o == a));
            let b = order.iter().position(|o| *o == id);
            if let (Some(a), Some(b)) = (a, b) {
                self.selected = order[a.min(b)..=a.max(b)].to_vec();
                return;
            }
        }
        self.selected = vec![id];
        self.anchor = Some(id);
        match pane {
            Pane::Main => self.scroll_scheduled = Some(id),
            Pane::Scheduled => self.scroll_main = Some(id),
        }
    }

    /// The rows that a menu on `id` acts on: the selection when it holds `id`, else `id` alone.
    pub fn targets(&self, id: u64) -> Vec<u64> {
        if self.is_selected(id) {
            self.selected.clone()
        } else {
            vec![id]
        }
    }

    pub fn hover_files(&mut self, target: FileTarget) {
        self.last_file_target = Some(target.clone());
        self.file_target = Some(target);
    }
}

/// The desktop app. It reads the station only through a [`ViewSource`].
pub struct RadioApp {
    source: Box<dyn ViewSource>,
    clock: Box<dyn Fn() -> Timestamp>,
    view: StationView,
    styled: bool,
    link: Linked,
    clip: Clipboard,
    tab: scheduled_pane::Tab,
    open: windows::Open,
    carts: cartwall::State,
    mixer: mixer::State,
    clock_state: clock_window::State,
    history: history::State,
    library: library::State,
    editor: Option<Editor>,
    toast: Option<(String, f64)>,
    confirm: Option<(String, Action)>,
    rename: Option<(String, String)>,
    properties: Option<(String, Vec<(String, String)>)>,
}

impl RadioApp {
    pub fn new(mut source: Box<dyn ViewSource>, clock: Box<dyn Fn() -> Timestamp>) -> Self {
        let view = source.snapshot(clock());
        let on_air = view
            .rows
            .iter()
            .find(|r| is_on_air(r, view.now.timestamp()));
        let link = Linked {
            scroll_main: on_air.map(|r| r.id),
            ..Linked::default()
        };
        let carts = cartwall::State::new(view.carts.len());
        Self {
            source,
            clock,
            view,
            styled: false,
            link,
            clip: Clipboard::default(),
            tab: scheduled_pane::Tab::Upcoming,
            open: windows::Open::default(),
            carts,
            mixer: mixer::State::default(),
            clock_state: clock_window::State::default(),
            history: history::State::default(),
            library: library::State::default(),
            editor: None,
            toast: None,
            confirm: None,
            rename: None,
            properties: None,
        }
    }

    /// Open or close a tool window.
    pub fn set_window(&mut self, w: ToolWindow, open: bool) {
        self.open.set(w, open);
    }

    /// Take a new snapshot when the clock reaches a new second.
    fn refresh(&mut self, force: bool) {
        let now = (self.clock)();
        if force || now.as_second() != self.view.now.timestamp().as_second() {
            self.view = self.source.snapshot(now);
        }
    }

    fn toast(&mut self, ctx: &egui::Context, text: String) {
        let until = ctx.input(|i| i.time) + size::TOAST_S;
        self.toast = Some((text, until));
    }

    fn report(&mut self, ctx: &egui::Context, result: Result<String, String>) {
        let text = match result {
            Ok(t) | Err(t) => t,
        };
        self.toast(ctx, text);
    }

    /// Put library items at a place in the main timeline, in their order.
    fn insert_items(&mut self, items: &[u64], at: Place) -> Result<String, String> {
        let mut last = Ok(String::new());
        // Inserting in reverse at one place keeps the order of the items.
        let ordered: Vec<u64> = match at {
            Place::End => items.to_vec(),
            _ => items.iter().rev().copied().collect(),
        };
        for item in ordered {
            last = self.source.edit(Edit::Insert { item, at });
            if last.is_err() {
                break;
            }
        }
        last
    }

    fn paste(&mut self, ctx: &egui::Context, to: PasteTo) {
        if self.clip.items.is_empty() {
            self.toast(ctx, "preview — the clipboard is empty".to_owned());
            return;
        }
        let items = self.clip.items.clone();
        let result = match to {
            PasteTo::Main(at) => self.insert_items(&items, at),
            PasteTo::Playlist(name) => {
                for item in &items {
                    self.source
                        .drop_item(*item, DropTarget::Playlist(name.clone()));
                }
                Ok(format!(
                    "preview — pasted {} items into {name} (not saved)",
                    items.len()
                ))
            }
            PasteTo::Cart(cart) => self.source.edit(Edit::AssignCart {
                cart,
                item: items.first().copied(),
            }),
        };
        if result.is_ok() && self.clip.cut && !self.clip.rows.is_empty() {
            let rows = std::mem::take(&mut self.clip.rows);
            let _ = self.source.edit(Edit::Remove(rows));
            self.clip.cut = false;
        }
        self.report(ctx, result);
    }

    fn add_files_to(&mut self, ctx: &egui::Context, paths: &[PathBuf]) {
        let (audio, skipped) = folders::expand_dropped(paths);
        let skipped_note = if skipped > 0 {
            format!(" · skipped {skipped} non-audio files")
        } else {
            String::new()
        };
        let target = self
            .link
            .file_target
            .clone()
            .or(self.link.last_file_target.clone());
        let Some(target) = target else {
            self.toast(ctx, format!("drop onto a playlist{skipped_note}"));
            return;
        };
        let n = audio.len();
        if n == 0 {
            self.toast(
                ctx,
                format!("preview — no audio files to add{skipped_note}"),
            );
            return;
        }
        let place = match &target {
            FileTarget::Main(_) => self.view.main_playlist.clone(),
            FileTarget::Playlist(name) => name.clone(),
            FileTarget::Editor => self
                .editor
                .as_ref()
                .map_or_else(|| "the editor".to_owned(), |e| e.name().to_owned()),
            FileTarget::Library => "the Library".to_owned(),
        };
        let ids = self.source.import_files(audio.clone(), "Unsorted");
        let result = match target {
            FileTarget::Main(at) => self.insert_items(&ids, at).map(|_| String::new()),
            FileTarget::Playlist(name) => {
                for id in &ids {
                    self.source
                        .drop_item(*id, DropTarget::Playlist(name.clone()));
                }
                Ok(String::new())
            }
            FileTarget::Editor => {
                if let Some(e) = self.editor.as_mut() {
                    e.add_files(&audio);
                }
                Ok(String::new())
            }
            FileTarget::Library => Ok(String::new()),
        };
        let text = match result {
            Ok(_) => format!("preview — added {n} files to {place} (not saved){skipped_note}"),
            Err(e) => format!("{e}{skipped_note}"),
        };
        self.toast(ctx, text);
    }

    /// Pass the actions of this frame on.
    fn run(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        for action in actions {
            self.act(ctx, action);
        }
        self.refresh(true);
    }

    fn act(&mut self, ctx: &egui::Context, action: Action) {
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
            Action::Properties { title, fields } => self.properties = Some((title, fields)),
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

    fn tool_windows(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>, t: &Tokens) {
        let view = &self.view;
        let open = &mut self.open;
        let link = &mut self.link;
        windows::show(ctx, ToolWindow::Cartwall, open, t, |ui| {
            cartwall::show(ui, view, &mut self.carts, link.has_clip, actions, t);
        });
        windows::show(ctx, ToolWindow::Mixer, open, t, |ui| {
            mixer::show(ui, view, &mut self.mixer, t);
        });
        windows::show(ctx, ToolWindow::Clock, open, t, |ui| {
            clock_window::show(ui, view, &mut self.clock_state, t);
        });
        windows::show(ctx, ToolWindow::History, open, t, |ui| {
            history::show(ui, view, &mut self.history, actions, t);
        });
        if self.library.docked {
            // The docked library takes the place of its window.
            open.set(ToolWindow::Library, false);
        } else {
            windows::show(ctx, ToolWindow::Library, open, t, |ui| {
                library::show(ui, view, &mut self.library, link, actions, t);
            });
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>, t: &Tokens) {
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
        if let Some((title, fields)) = &self.properties {
            let mut close = false;
            egui::Window::new(format!("{title} — properties"))
                .collapsible(false)
                .frame(theme::card_frame(t).inner_margin(pad::GUTTER))
                .show(ctx, |ui| {
                    egui::Grid::new("properties")
                        .spacing(egui::vec2(space::M, space::XS))
                        .show(ui, |ui| {
                            for (k, v) in fields {
                                ui.label(RichText::new(k).color(t.text_muted));
                                ui.label(v);
                                ui.end_row();
                            }
                        });
                    let note = "Cue points are read-only until the library engine decodes files.";
                    ui.label(
                        RichText::new(note)
                            .font(theme::text(theme::CAPTION))
                            .color(t.text_muted),
                    );
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            if close {
                self.properties = None;
            }
        }
    }
}

impl eframe::App for RadioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.styled {
            theme::apply(&ctx);
            self.styled = true;
        }
        self.refresh(false);
        ctx.request_repaint_after(Duration::from_secs(1));
        let t = Tokens::get(&ctx);
        let mut actions = Vec::new();
        self.link.has_clip = !self.clip.items.is_empty();
        self.link.file_target = None;

        egui::Panel::top("decks")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::GUTTER))
            .show(ui, |ui| {
                windows::toggles(ui, &mut self.open, &t);
                ui.add_space(space::S);
                decks::show(ui, &self.view, &mut actions, &t);
            });

        egui::Panel::bottom("status")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::BAR))
            .show(ui, |ui| {
                status_line(ui, &self.view, self.toast.as_ref(), &t)
            });

        if self.library.docked {
            egui::Panel::bottom("library_dock")
                .resizable(true)
                .default_size(size::DOCK_H)
                .frame(theme::bar_frame(&t, pad::GUTTER))
                .show(ui, |ui| {
                    library::show(
                        ui,
                        &self.view,
                        &mut self.library,
                        &mut self.link,
                        &mut actions,
                        &t,
                    );
                });
        }

        let width = ui.available_width();
        egui::Panel::right("scheduled")
            .resizable(true)
            .default_size(width * size::SPLIT_FRACTION)
            .min_size(size::SPLIT_MIN)
            .frame(theme::panel_frame(&t))
            .show(ui, |ui| {
                scheduled_pane::show(
                    ui,
                    &self.view,
                    &mut self.tab,
                    &mut self.link,
                    &mut actions,
                    &t,
                );
            });

        egui::CentralPanel::default()
            .frame(theme::panel_frame(&t))
            .show(ui, |ui| {
                main_pane::show(ui, &self.view, &mut self.link, &mut actions, &t)
            });

        self.tool_windows(&ctx, &mut actions, &t);
        self.dialogs(&ctx, &mut actions, &t);

        if let Some(mut tab) = rules_editor::show(&ctx, &mut self.editor, &mut self.link, &t) {
            if let Some(src) = &tab.source {
                match folders::list_audio(std::path::Path::new(&src.path), src.recursive) {
                    Ok(files) => {
                        tab.items = files
                            .iter()
                            .map(|f| (folders::stem_of(f).to_owned(), Duration::ZERO))
                            .collect();
                    }
                    Err(e) => self.toast(&ctx, format!("preview — {e}")),
                }
            }
            self.source.apply_rules(tab);
            self.refresh(true);
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_files_to(&ctx, &dropped);
            self.refresh(true);
        }
        let expired = self
            .toast
            .as_ref()
            .is_some_and(|(_, until)| ctx.input(|i| i.time) > *until);
        if expired {
            self.toast = None;
        }
        self.run(&ctx, actions);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Tokens::DARK.canvas.to_normalized_gamma_f32()
    }
}

fn status_line(ui: &mut egui::Ui, view: &StationView, toast: Option<&(String, f64)>, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::L;
        let muted = |s: &str| {
            RichText::new(s)
                .font(theme::text(theme::LABEL))
                .color(t.text_muted)
        };
        // Neither output works in the preview, so both show a hollow dot in a muted colour.
        ui.label(muted("Local ○ preview (no audio)"));
        ui.label(muted("Icecast ○ not connected"));
        let warn = format!("· {} warnings", view.warnings);
        let colour = if view.warnings > 0 {
            t.warn
        } else {
            t.text_muted
        };
        ui.label(
            RichText::new(warn)
                .font(theme::text(theme::LABEL))
                .color(colour),
        );
        if let Some((text, _)) = toast {
            ui.label(muted(text));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let zone = view.now.time_zone().iana_name().unwrap_or("local time");
            ui.label(muted(&format!("UI preview · mock data · {zone}")));
        });
    });
}

/// Is `row` on air at `now`?
pub(crate) fn is_on_air(row: &Row, now: Timestamp) -> bool {
    let start = row.start.timestamp();
    let end = SignedDuration::try_from(row.len).map_or(start, |d| start + d);
    start <= now && now < end
}

/// A length as m:ss. Zero means unknown and shows as "—".
pub(crate) fn mmss(d: Duration) -> String {
    if d.is_zero() {
        return "—".to_owned();
    }
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// A local time as HH:MM:SS.
pub(crate) fn hms(z: &jiff::Zoned) -> String {
    z.strftime("%H:%M:%S").to_string()
}

/// The fields of the properties window for a timeline row or playlist item.
pub(crate) fn row_fields(
    title: &str,
    len: Duration,
    extra: &[(&str, String)],
) -> Vec<(String, String)> {
    let mut f = vec![
        ("Title".to_owned(), title.to_owned()),
        ("Length".to_owned(), mmss(len)),
    ];
    f.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
    for cue in ["Cue in", "Intro", "Segue", "Cue out"] {
        f.push((cue.to_owned(), "— (not decoded yet)".to_owned()));
    }
    f
}
