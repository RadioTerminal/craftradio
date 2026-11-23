//! The main window: decks on top, the main and scheduled panes in the middle, a status line below.
//! Tool windows (cartwall, mixer, clock, history, library, alarms) open from the deck strip.

use std::path::PathBuf;
use std::time::Duration;

use egui::RichText;
use jiff::{SignedDuration, Timestamp};

mod action;
mod clipboard;
mod dialogs;
mod drops;
mod keys;

pub(crate) use action::{Action, FileTarget, PasteTo};
pub(crate) use clipboard::{Clipboard, CutSource};

use crate::add_menu::{self, AddDialog};
use crate::icons::{self, Icon};
use crate::meta_editor::{self, MetaEditor};
use crate::prepared::Prepared;
use crate::rules_editor::{self, Editor};
use crate::shortcuts;

use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{
    Ballistics, DeckCommand, DropTarget, Edit, FolderSource, MetaTarget, Place, Row, StationView,
    Status, ViewSource,
};
use crate::windows::{self, ToolWindow};
use crate::{
    alarms, cartwall, clock_window, decks, history, library, main_pane, mixer, scheduled_pane,
};

/// The pane that has the keyboard, for the edit shortcuts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pane {
    #[default]
    Main,
    Scheduled,
    Library,
    History,
    /// The item list of a playlist tab.
    Playlist,
}

/// The id `by` rows away from `current` in `order`, for the arrow keys.
/// With nothing selected, Down picks the first row and Up the last.
pub(crate) fn step(order: &[u64], current: Option<u64>, by: i32) -> Option<u64> {
    let last = order.len().checked_sub(1)?;
    let at = current.and_then(|c| order.iter().position(|o| *o == c));
    let next = match at {
        Some(i) if by < 0 => i.saturating_sub(by.unsigned_abs() as usize),
        Some(i) => (i + by as usize).min(last),
        None if by < 0 => last,
        None => 0,
    };
    order.get(next).copied()
}

/// A question that waits for a yes before its action runs.
pub(crate) struct Confirm {
    pub text: String,
    pub yes: String,
    pub then: Action,
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
    /// The drop target under the pointer in this frame, while files hover.
    pub file_target: Option<FileTarget>,
    /// The last drop target that the pointer was over.
    pub last_file_target: Option<FileTarget>,
    /// The shortcuts of this frame, read once before the panes draw.
    pub keys: shortcuts::Pressed,
    /// The selected items of the playlist tab on show, and the name of that playlist.
    pub playlist_sel: crate::select::Selection,
    pub playlist_sel_of: String,
    /// The main pane keeps the row on air near the top. A scroll by hand turns it off.
    pub follow: bool,
    /// The row on air that the main pane last scrolled to.
    pub followed: Option<u64>,
}

impl Linked {
    pub fn is_selected(&self, id: u64) -> bool {
        self.selected.contains(&id)
    }

    /// Select after a click. Command adds or removes one row; shift selects a range of `order`.
    pub fn click(&mut self, pane: Pane, id: u64, m: egui::Modifiers, order: &[u64]) {
        self.focus = pane;
        if !crate::select::click(&mut self.selected, &mut self.anchor, id, m, order) {
            return;
        }
        match pane {
            Pane::Main => self.scroll_scheduled = Some(id),
            Pane::Scheduled => self.scroll_main = Some(id),
            Pane::Library | Pane::History | Pane::Playlist => {}
        }
    }

    /// Move the selection of the main pane or Upcoming by the arrow keys of this frame.
    /// Both lists share the selection, so both scroll to the new row.
    pub fn step_rows(&mut self, pane: Pane, order: &[u64]) {
        if self.focus != pane || self.keys.step == 0 {
            return;
        }
        if let Some(id) = step(order, self.selected.last().copied(), self.keys.step) {
            self.selected = vec![id];
            self.anchor = Some(id);
            self.scroll_main = Some(id);
            self.scroll_scheduled = Some(id);
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
    /// Screenshot requests of the control channel, when it is attached.
    captures: Option<crate::capture::Captures>,
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
    preferences: crate::preferences::State,
    /// This computer's preferences: in memory until the app gives a store.
    prefs: crate::app_prefs::AppPrefs,
    /// The window layout last kept, once the app has a store.
    layout: Option<(Vec<&'static str>, bool)>,
    /// Menu commands for the next frame, as the keys of the same commands.
    menu_keys: shortcuts::Pressed,
    /// The native menu bar (macOS), once the desktop app installs it.
    native_menu: Option<craft_menu::NativeMenu>,
    /// The keymap: the catalog's defaults and the shortcuts a person changed.
    keymap: craft_keymap::Keymap,
    /// True when the context has this keymap.
    keymap_installed: bool,
    /// The Keyboard Shortcuts dialog while it is open.
    shortcuts_dialog: Option<craft_keymap::ui::DialogState>,
    /// Update checks and self-update, when the desktop app asks for them.
    updates: Option<crate::updates::Updates>,
    /// Help ▸ Check for Updates was chosen. The check starts in the next frame.
    check_updates: bool,
    clock_state: clock_window::State,
    history: history::State,
    alarms: alarms::State,
    library: library::State,
    editor: Option<Editor>,
    toast: Option<(String, f64)>,
    /// The toast offers Undo: it reports a remove.
    toast_undo: bool,
    confirm: Option<Confirm>,
    /// The Clear playlist dialog: the playlist, its item count, and "also remove locked".
    clear: Option<(String, usize, bool, bool)>,
    rename: Option<(String, String)>,
    meta: Option<MetaEditor>,
    add_dialog: Option<AddDialog>,
    one_off: Option<crate::one_off::OneOffDialog>,
    /// Display text and lookups for the current snapshot.
    prepared: Prepared,
    /// The frame time readout, when the app runs with `--perf`.
    perf: Option<Perf>,
    decks: decks::State,
    /// The levels that the meters show, after the ballistics, and when they last moved.
    meter_shown: Vec<Ballistics>,
    meter_at: Option<std::time::Instant>,
    /// Until when the meters ask for frames: a second after the last change.
    meter_active_until: Option<std::time::Instant>,
    /// The audio block that the meters last showed, when it first came, and how it went.
    meter_block: (u64, Option<std::time::Instant>),
    meter_stats: MeterStats,
    /// True while sound moves any meter, so the Mixer draws at about 30 fps.
    meters_moving: bool,
    /// True while sound moves the Out meter, the one meter of the main window.
    out_moving: bool,
}

/// The share of the top row that the in-window menus may take; the window toggles take the rest.
const MENU_SHARE: f32 = 0.4;

/// The settings key of the open tool windows, and of the docked Library.
const OPEN_KEY: &str = "open";
const DOCKED_KEY: &str = "library_docked";

impl RadioApp {
    pub fn new(mut source: Box<dyn ViewSource>, clock: Box<dyn Fn() -> Timestamp>) -> Self {
        let view = source.snapshot(clock());
        let prepared = Prepared::new(&view, 0);
        let on_air = view
            .rows
            .iter()
            .find(|r| is_on_air(r, view.now.timestamp()));
        let link = Linked {
            follow: true,
            followed: None,
            scroll_main: on_air.map(|r| r.id),
            ..Linked::default()
        };
        let carts = cartwall::State::default();
        Self {
            captures: None,
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
            preferences: crate::preferences::State::default(),
            prefs: crate::app_prefs::AppPrefs::memory(),
            layout: None,
            menu_keys: shortcuts::Pressed::default(),
            native_menu: None,
            keymap: shortcuts::default_keymap(),
            keymap_installed: false,
            shortcuts_dialog: None,
            updates: None,
            check_updates: false,
            clock_state: clock_window::State::default(),
            history: history::State::default(),
            alarms: alarms::State::default(),
            library: library::State::default(),
            editor: None,
            toast: None,
            toast_undo: false,
            confirm: None,
            clear: None,
            rename: None,
            meta: None,
            add_dialog: None,
            one_off: None,
            prepared,
            perf: None,
            decks: decks::State::default(),
            meter_shown: Vec::new(),
            meter_at: None,
            meter_active_until: None,
            meter_block: (0, None),
            meter_stats: MeterStats::default(),
            meters_moving: false,
            out_moving: false,
        }
    }

    /// Reads the meters of this instant from the source, and gives them ballistics: a bar
    /// rises at once and falls at a fixed rate in dB per second, by the real frame time.
    fn read_meters(&mut self) {
        let Some(frame) = self.source.meters() else {
            self.meters_moving = false;
            self.out_moving = false;
            return;
        };
        let now = std::time::Instant::now();
        let dt = self
            .meter_at
            .map_or(0.0, |at| now.duration_since(at).as_secs_f32());
        self.meter_at = Some(now);
        if frame.at_sample != self.meter_block.0 || self.meter_block.1.is_none() {
            self.meter_block = (frame.at_sample, Some(now));
            self.meter_stats.blocks += 1;
        }
        // How long ago the block on show arrived here: at most one audio block plus this frame.
        if let Some(came) = self.meter_block.1 {
            let age = now.duration_since(came).as_secs_f32() * 1000.0;
            self.meter_stats.age_sum_ms += age;
            self.meter_stats.age_max_ms = self.meter_stats.age_max_ms.max(age);
            self.meter_stats.reads += 1;
        }
        self.meter_shown
            .resize(frame.level_db.len(), Ballistics::default());
        let mut changed = false;
        for (i, target) in frame.level_db.iter().enumerate() {
            let held = frame.peak_db.get(i).copied().unwrap_or(*target);
            let shown = &mut self.meter_shown[i];
            changed |= shown.step(*target, held, dt);
            if let Some(m) = self.view.meters.get_mut(i) {
                m.level_db = if m.mute {
                    [size::METER_MIN_DB; 2]
                } else {
                    shown.level
                };
                m.peak_db = shown.hold;
            }
        }
        let loud = |l: &[f32; 2]| l[0].max(l[1]) > crate::view::METER_SILENT_DB;
        self.meters_moving = frame.moving() || self.meter_shown.iter().any(|b| loud(&b.level));
        let out = self.view.meters.iter().position(|m| m.label == "Out");
        self.out_moving = out.is_some_and(|i| {
            frame.level_db.get(i).is_some_and(loud)
                || self.meter_shown.get(i).is_some_and(|b| loud(&b.level))
        });
        if changed || self.meters_moving {
            self.meter_active_until = Some(now + Duration::from_secs_f32(size::METER_IDLE_S));
        }
    }

    /// Answers screenshot requests (the control channel's `ui.screenshot`) with PNGs of the
    /// main window.
    pub fn with_screenshots(
        mut self,
        requests: std::sync::mpsc::Receiver<crate::capture::ScreenshotRequest>,
    ) -> Self {
        self.captures = Some(crate::capture::Captures::new(requests));
        self
    }

    /// Show the time of each frame in the status line.
    pub fn with_perf(mut self, on: bool) -> Self {
        self.perf = on.then(Perf::default);
        self
    }

    /// Keeps this computer's preferences in `prefs`. The display settings that the station
    /// kept are copied there the first time.
    pub fn with_preferences(mut self, prefs: crate::app_prefs::AppPrefs) -> Self {
        self.prefs = prefs;
        self.prefs.import(&self.view);
        self.keymap = shortcuts::default_keymap().with_overrides(self.prefs.shortcuts());
        self.keymap_installed = false;
        self.refresh(true);
        // The windows open as they were left.
        let open = crate::parts::strings(&self.view, crate::parts::WINDOWS, OPEN_KEY, &[]);
        for w in ToolWindow::ALL {
            if open.iter().any(|t| t == w.title()) {
                self.open.set(w, true);
            }
        }
        self.library.docked =
            crate::parts::bool(&self.view, crate::parts::WINDOWS, DOCKED_KEY, false);
        self.layout = Some(self.current_layout());
        self
    }

    /// Checks for updates at start and on Help ▸ Check for Updates, and installs them.
    /// Without this call the app never asks the network for releases (tests, previews).
    #[must_use]
    pub fn with_updates(mut self) -> Self {
        self.updates = Some(crate::updates::Updates::new());
        self
    }

    /// Installs the native menu bar (macOS), once, from the desktop app's creator. False when
    /// AppKit refused it: the menus stay in the window.
    #[cfg(target_os = "macos")]
    pub fn install_native_menu(&mut self, ctx: &egui::Context) -> bool {
        let bar = craft_menu::mac_layout(
            &crate::menus::bar(&self.open, &shortcuts::keymap(ctx), self.updates.is_some()),
            &crate::menus::options(),
        )
        .bar;
        self.native_menu = craft_menu::mac::install(ctx, &bar);
        self.native_menu.is_some()
    }

    /// What may run from the menus now: nothing of the app's while a dialog is open.
    fn menu_gate(&self, ctx: &egui::Context) -> craft_menu::Gate {
        craft_menu::Gate {
            modal: self.shortcuts_dialog.is_some()
                || self
                    .updates
                    .as_ref()
                    .is_some_and(crate::updates::Updates::modal)
                || self.confirm.is_some()
                || self.clear.is_some()
                || self.rename.is_some()
                || self.meta.is_some()
                || self.add_dialog.is_some()
                || self.one_off.is_some(),
            recording_shortcut: self
                .shortcuts_dialog
                .as_ref()
                .is_some_and(craft_keymap::ui::DialogState::is_recording),
            text_editing: ctx.egui_wants_keyboard_input(),
        }
    }

    /// Runs a menu command as its shortcut would: in this frame's keys, or on a window.
    fn run_menu(&mut self, command: &craft_keymap::CommandRef) {
        match crate::menus::run(command) {
            crate::menus::Run::Keys(p) => self.menu_keys = self.menu_keys.or(p),
            crate::menus::Run::Window(w) => {
                if !(w == ToolWindow::Library && self.library.docked) {
                    let on = self.open.get(w);
                    self.open.set(w, !on);
                }
            }
            crate::menus::Run::Shortcuts => {
                self.shortcuts_dialog = Some(craft_keymap::ui::DialogState::new());
            }
            crate::menus::Run::CheckUpdates => self.check_updates = true,
            crate::menus::Run::Nothing => {}
        }
    }

    /// The Keyboard Shortcuts dialog: each change applies at once and is saved.
    fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = self.shortcuts_dialog.as_mut() else {
            return;
        };
        let out =
            craft_keymap::ui::ShortcutsDialog::new(&mut self.keymap, &craft_keymap::ui::English)
                .show(ctx, state);
        if matches!(
            out,
            Some(craft_keymap::ui::Outcome::Changed | craft_keymap::ui::Outcome::Ok)
        ) {
            self.prefs.set_shortcuts(self.keymap.overrides().clone());
            self.keymap_installed = false;
            ctx.request_repaint();
        }
        if !state.is_open() {
            self.shortcuts_dialog = None;
        }
    }

    /// The native menu's clicks since the last frame, run before this frame reads its keys.
    fn native_menu_clicks(&mut self, ctx: &egui::Context) {
        let gate = self.menu_gate(ctx);
        let Some(menu) = self.native_menu.as_mut() else {
            return;
        };
        menu.set_gate(gate);
        for command in menu.take_clicks(ctx) {
            self.run_menu(&command);
        }
    }

    /// Brings the native menu in step with the app after this frame.
    fn sync_native_menu(&mut self, ctx: &egui::Context) {
        if self.native_menu.is_none() {
            return;
        }
        let layout = craft_menu::mac_layout(
            &crate::menus::bar(&self.open, &shortcuts::keymap(ctx), self.updates.is_some()),
            &crate::menus::options(),
        );
        if let Some(menu) = self.native_menu.as_mut() {
            let _ = menu.sync(layout);
        }
    }

    /// The menu bar drawn in the window, where there is no native one.
    fn menu_bar_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let options = crate::menus::options();
        let bar = craft_menu::layout(
            &crate::menus::bar(&self.open, &shortcuts::keymap(&ctx), self.updates.is_some()),
            craft_menu::Target::current(),
            &options,
        )
        .bar;
        let gate = self.menu_gate(&ctx);
        let label: &dyn Fn(&'static str) -> String = &|english| (options.label)(english);
        // The bar takes its titles' width, and folds the titles that don't fit into its "…".
        let chosen = craft_menu::MenuBarUi::new(&bar)
            .gate(gate)
            .label(label)
            .id(egui::Id::unique("radio-menu-bar"))
            .shrink_to_fit(ui.available_width() * MENU_SHARE)
            .show(ui);
        for command in chosen {
            self.run_menu(&command);
            // The command runs with the next frame's keys.
            ctx.request_repaint();
        }
    }

    /// The open tool windows, by title, and whether the Library is docked.
    fn current_layout(&self) -> (Vec<&'static str>, bool) {
        let open = ToolWindow::ALL
            .into_iter()
            .filter(|w| self.open.get(*w))
            .map(ToolWindow::title)
            .collect();
        (open, self.library.docked)
    }

    /// Keeps the open windows and the docked Library when they change.
    fn keep_layout(&mut self) {
        let now = self.current_layout();
        if self.layout.as_ref() == Some(&now) {
            return;
        }
        if self.layout.is_some() {
            self.prefs
                .set(crate::parts::WINDOWS, OPEN_KEY, serde_json::json!(now.0));
            self.prefs
                .set(crate::parts::WINDOWS, DOCKED_KEY, serde_json::json!(now.1));
        }
        self.layout = Some(now);
    }

    /// Open or close a tool window.
    pub fn set_window(&mut self, w: ToolWindow, open: bool) {
        self.open.set(w, open);
    }

    /// Take a new snapshot when the clock reaches a new second.
    pub(super) fn refresh(&mut self, force: bool) {
        let now = (self.clock)();
        if force || now.as_second() != self.view.now.timestamp().as_second() {
            self.view = self.source.snapshot(now);
            self.prefs.overlay(&mut self.view);
            self.prepared = Prepared::new(&self.view, self.prepared.generation + 1);
        }
    }

    pub(super) fn toast(&mut self, ctx: &egui::Context, text: String) {
        let until = ctx.input(|i| i.time) + size::TOAST_S;
        self.toast = Some((text, until));
        self.toast_undo = false;
    }

    /// Open the metadata editor for items of the timeline or the library.
    pub(super) fn open_meta(&mut self, targets: Vec<MetaTarget>) {
        let rows = self.view.history.iter().chain(&self.view.rows);
        let mut list = Vec::new();
        for t in &targets {
            let meta = match t {
                MetaTarget::Row(id) => rows.clone().find(|r| r.id == *id).map(|r| r.meta.clone()),
                MetaTarget::Library(id) => self
                    .view
                    .library
                    .iter()
                    .find(|l| l.id == *id)
                    .map(|l| l.meta.clone()),
            };
            if let Some(m) = meta {
                list.push((*t, m));
            }
        }
        if list.is_empty() {
            return;
        }
        // The preview line asks the source for the stream title of a row.
        // A library item is not in the timeline, so it borrows a row with an id that no row has.
        let base = match targets.first() {
            Some(MetaTarget::Row(id)) => rows.clone().find(|r| r.id == *id).cloned(),
            _ => self.view.rows.first().cloned().map(|mut r| {
                r.id = u64::MAX;
                r
            }),
        };
        self.meta = base.map(|b| {
            let mut ed = MetaEditor::new(list, b);
            ed.base_preview = self.view.preview;
            ed
        });
    }

    pub(super) fn tool_windows(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
        t: &Tokens,
    ) {
        let on_air = self
            .view
            .rows
            .iter()
            .find(|r| is_on_air(r, self.view.now.timestamp()));
        // Only the Icecast page shows the stream title, so only open Preferences asks for it.
        let streaming = if self.open.get(ToolWindow::Preferences) {
            on_air.map_or_else(String::new, |r| self.source.stream_title(r))
        } else {
            String::new()
        };
        let view = &self.view;
        let open = &mut self.open;
        let link = &mut self.link;
        let mut sized = Vec::new();
        let saved = |w: ToolWindow| windows::Saved {
            size: saved_pair(view, w, windows::SIZE_KEY),
            pos: saved_pair(view, w, windows::POS_KEY),
        };
        let mut note = |w: ToolWindow, c: Option<windows::SizeChange>| {
            if let Some(c) = c {
                sized.push((w, c));
            }
        };
        let w = ToolWindow::Cartwall;
        note(
            w,
            windows::show(ctx, w, open, saved(w), t, |ui| {
                cartwall::show(ui, view, &mut self.carts, link.has_clip, actions, t);
            }),
        );
        let w = ToolWindow::Mixer;
        note(
            w,
            windows::show(ctx, w, open, saved(w), t, |ui| {
                mixer::show(ui, view, &mut self.mixer, &streaming, actions, t);
            }),
        );
        let w = ToolWindow::Clock;
        note(
            w,
            windows::show(ctx, w, open, saved(w), t, |ui| {
                clock_window::show(ui, view, &mut self.clock_state, actions, t);
            }),
        );
        let w = ToolWindow::History;
        note(
            w,
            windows::show(ctx, w, open, saved(w), t, |ui| {
                let root = own_keys(ui, link);
                history::show(ui, view, &mut self.history, link, actions, t);
                link.keys = root;
            }),
        );
        let w = ToolWindow::Alarms;
        note(
            w,
            windows::show(ctx, w, open, saved(w), t, |ui| {
                alarms::show(ui, view, &mut self.alarms, actions, t);
            }),
        );
        let w = ToolWindow::Preferences;
        let mut done = false;
        let was_open = open.get(w);
        note(
            w,
            windows::show(ctx, w, open, saved(w), t, |ui| {
                let root = own_keys(ui, link);
                self.preferences.find = link.keys.find;
                let parts = crate::preferences::Parts {
                    mixer: &mut self.mixer,
                    carts: &mut self.carts,
                    library: &mut self.library,
                };
                done = crate::preferences::show(
                    ui,
                    view,
                    &mut self.preferences,
                    parts,
                    &streaming,
                    actions,
                    t,
                );
                link.keys = root;
            }),
        );
        if done {
            open.set(w, false);
        }
        if was_open && !open.get(w) {
            // The next visit starts without the last search, so a ⚙ opens on its own page.
            self.preferences.search.clear();
        }
        if (was_open && !open.get(w)) || self.preferences.page != crate::preferences::Page::Cartwall
        {
            self.carts.leave_preferences();
        }
        if was_open && !open.get(w) {
            self.mixer.drop_drafts();
        }
        if self.library.docked {
            // The docked library takes the place of its window.
            open.set(ToolWindow::Library, false);
        } else {
            let w = ToolWindow::Library;
            note(
                w,
                windows::show(ctx, w, open, saved(w), t, |ui| {
                    let root = own_keys(ui, link);
                    library::show(ui, view, &mut self.library, link, actions, t);
                    link.keys = root;
                }),
            );
        }
        for (w, c) in sized {
            let (key, value) = match c {
                windows::SizeChange::Manual([x, y]) => {
                    (windows::SIZE_KEY, serde_json::json!([x, y]))
                }
                windows::SizeChange::Moved([x, y]) => (windows::POS_KEY, serde_json::json!([x, y])),
            };
            actions.push(Action::EditQuiet(Edit::SetPartSetting {
                part: w.part().to_owned(),
                key: key.to_owned(),
                value,
            }));
        }
    }
}

impl eframe::App for RadioApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The native menu's key equivalents reach egui as the keys they are.
        if let Some(menu) = &mut self.native_menu {
            menu.raw_input(raw_input);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.prefs.flush();
        if let Some(updates) = &mut self.updates {
            updates.exit();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();
        let ctx = ui.ctx().clone();
        if !self.styled {
            theme::apply(&ctx);
            egui_extras::install_image_loaders(&ctx);
            self.styled = true;
        }
        self.refresh(false);
        if let Some(captures) = &mut self.captures {
            captures.tick(&ctx);
        }
        if let Some(failed) = self.prefs.tick(&ctx) {
            self.toast(&ctx, failed);
        }
        self.read_meters();
        // Work that finished after its edit reports here, as a toast.
        let notices = self.source.take_notices();
        if !notices.is_empty() {
            self.refresh(true);
            if let Some(last) = notices.last() {
                self.toast(&ctx, last.clone());
            }
        }
        // While a meter on show moves, draw at the display rate, and for a second after it
        // stopped, so the fall is seen. Else once a second is enough. The main window shows only the Out meter, the Mixer window all of them.
        // Only the main window asks for frames: each of its frames draws the Mixer window too.
        // A request of the Mixer window itself adds a second clock, and doubles the frames.
        let active = self
            .meter_active_until
            .is_some_and(|until| std::time::Instant::now() < until);
        let meters =
            active && (self.out_moving || self.open.get(ToolWindow::Mixer) || self.meters_moving);
        if meters {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        let t = Tokens::get(&ctx);
        let mut actions = Vec::new();
        self.link.has_clip = !self.clip.items.is_empty();
        self.link.file_target = None;
        if !self.keymap_installed {
            shortcuts::install(&ctx, std::sync::Arc::new(self.keymap.clone()));
            self.keymap_installed = true;
        }
        self.native_menu_clicks(&ctx);
        if let Some(updates) = &mut self.updates {
            if std::mem::take(&mut self.check_updates) {
                updates.check_now(&ctx);
            }
            updates.show(&ctx);
        }
        // While the Keyboard Shortcuts dialog is open its keys are its own: a chord being
        // recorded runs nothing.
        let pressed = if self.shortcuts_dialog.is_some() {
            shortcuts::Pressed::default()
        } else {
            shortcuts::read(&ctx)
        };
        self.link.keys = pressed.or(std::mem::take(&mut self.menu_keys));

        egui::Panel::top("decks")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::GUTTER))
            .show(ui, |ui| {
                // The menus open the row where there is no native menu bar, as in the Craft
                // apps; the window toggles follow them.
                ui.horizontal(|ui| {
                    if self.native_menu.is_none() {
                        self.menu_bar_ui(ui);
                        ui.separator();
                    }
                    windows::toggles(ui, &mut self.open, &t);
                });
                ui.add_space(space::S);
                decks::show(ui, &self.view, &mut self.decks, &mut actions, &t);
            });

        egui::Panel::bottom("status")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::BAR))
            .show(ui, |ui| {
                let toast = self
                    .toast
                    .as_ref()
                    .map(|(text, _)| (text.as_str(), self.toast_undo));
                status_line(ui, &self.view, toast, self.perf.as_ref(), &mut actions, &t)
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
                    &self.prepared,
                    &mut self.tab,
                    &mut self.link,
                    &mut actions,
                    &t,
                );
            });

        egui::CentralPanel::default()
            .frame(theme::panel_frame(&t))
            .show(ui, |ui| {
                main_pane::show(
                    ui,
                    &self.view,
                    &self.prepared,
                    &mut self.link,
                    &mut actions,
                    &t,
                )
            });

        self.tool_windows(&ctx, &mut actions, &t);
        self.keep_layout();
        self.global_keys(&ctx, &mut actions);
        self.dialogs(&ctx, &mut actions, &t);
        self.shortcuts_dialog(&ctx);
        self.sync_native_menu(&ctx);

        if let Some(tab) = rules_editor::show(&ctx, &mut self.editor, &mut self.link, &t) {
            let text = match self.source.apply_rules(tab) {
                Ok(()) => self
                    .view
                    .say(
                        "preview — rules stored; they are not applied to the scheduler yet",
                        "Rules saved; the next plan uses them",
                    )
                    .to_owned(),
                Err(e) => e,
            };
            self.toast(&ctx, text);
            self.refresh(true);
        }

        if let Some(list) = meta_editor::show(&ctx, &mut self.meta, self.source.as_ref(), &t) {
            self.run_edit(&ctx, Edit::SetMeta(list), true);
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
        if let Some(perf) = self.perf.as_mut() {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            perf.last_ms = ms;
            perf.mean_ms += (ms - perf.mean_ms) * PERF_WEIGHT;
            perf.frames += 1;
            // The perf readout also goes to stderr every 5 s, for a measurement by a script.
            let due = perf
                .printed
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5));
            if due {
                let secs = perf
                    .printed
                    .map_or(1.0, |at| at.elapsed().as_secs_f64())
                    .max(0.001);
                eprintln!(
                    "perf: frame {:.2} ms · mean {:.2} ms · {} frames ({:.1} fps) · audio blocks {:.0}/s · meter age mean {:.1} ms max {:.1} ms · meters {}",
                    perf.last_ms,
                    perf.mean_ms,
                    perf.frames,
                    (perf.frames - perf.window_frames) as f64 / secs,
                    (self.meter_stats.blocks - perf.window_blocks) as f64 / secs,
                    self.meter_stats.age_sum_ms / self.meter_stats.reads.max(1) as f32,
                    self.meter_stats.age_max_ms,
                    if self.meters_moving {
                        "moving"
                    } else {
                        "still"
                    }
                );
                perf.window_frames = perf.frames;
                perf.window_blocks = self.meter_stats.blocks;
                self.meter_stats.age_sum_ms = 0.0;
                self.meter_stats.age_max_ms = 0.0;
                self.meter_stats.reads = 0;
                perf.printed = Some(std::time::Instant::now());
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Tokens::DARK.canvas.to_normalized_gamma_f32()
    }
}

fn status_line(
    ui: &mut egui::Ui,
    view: &StationView,
    toast: Option<(&str, bool)>,
    perf: Option<&Perf>,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::L;
        let muted = |s: &str| {
            RichText::new(s)
                .font(theme::text(theme::LABEL))
                .color(t.text_muted)
        };
        // Only a connected output shows a filled green dot. Every other state is hollow and muted.
        let webrtc_word = view
            .output
            .webrtc_listeners_text()
            .map(|n| format!("listening, {n}"));
        for (name, status, connected) in [
            ("Local", &view.output.local, None),
            ("Icecast", &view.output.icecast, None),
            ("WebRTC", &view.output.webrtc, webrtc_word.as_deref()),
        ] {
            let off = name == "WebRTC" && view.output.streaming == Some(false);
            let (dot, word, colour) = match status {
                Status::NotConnected if off => (Icon::Circle, "off".to_owned(), t.text_muted),
                Status::Connected => (
                    Icon::CircleDot,
                    connected.unwrap_or("connected").to_owned(),
                    t.ok,
                ),
                Status::Preview => (Icon::Circle, "preview (no audio)".to_owned(), t.text_muted),
                Status::NotConnected => (Icon::Circle, "not connected".to_owned(), t.text_muted),
                Status::Reconnecting { attempt } => (
                    Icon::RefreshCw,
                    format!("reconnecting (try {attempt})"),
                    t.warn,
                ),
                Status::Failed(e) => (Icon::Circle, format!("failed: {e}"), t.danger),
            };
            let text = format!("{name} {} {word}", icons::glyph(dot));
            let text = RichText::new(text)
                .font(theme::text(theme::LABEL))
                .color(colour);
            // The Icecast state opens its page of Preferences.
            if name == "Icecast" {
                let link = ui
                    .add(egui::Label::new(text).sense(egui::Sense::click()))
                    .on_hover_text("Click for the Icecast settings");
                if link.clicked() {
                    actions.push(Action::OpenPreferences(crate::preferences::Page::Icecast));
                }
            } else {
                ui.label(text);
            }
        }
        // The chip counts the alarms that nobody acknowledged, with the icon and colour of
        // the most serious one. It opens the Alarms window.
        let open = view.open_alarms().count();
        let chip = match view.worst_open() {
            Some(worst) => RichText::new(icons::lead(
                alarms::icon(worst),
                &format!("{open} {}", if open == 1 { "warning" } else { "warnings" }),
            ))
            .color(alarms::colour(worst, t)),
            None => RichText::new("0 warnings").color(t.text_muted),
        };
        let chip = chip.font(theme::text(theme::LABEL));
        if ui
            .button(chip)
            .on_hover_text("Show the alarms and warnings")
            .clicked()
        {
            actions.push(Action::OpenWindow(ToolWindow::Alarms));
        }
        // The right side comes first. The toast truncates in the room that is left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let zone = view.now.time_zone().iana_name().unwrap_or("local time");
            let source = view.say("UI preview · mock data", "Station");
            ui.label(muted(&format!("{source} · {zone}")));
            if let Some(p) = perf {
                let text = format!("frame {:.1} ms · mean {:.1} ms", p.last_ms, p.mean_ms);
                ui.label(
                    RichText::new(text)
                        .font(theme::mono(theme::CAPTION))
                        .color(t.text_muted),
                );
            }
            if let Some((text, undo)) = toast {
                let label = format!("Undo ({})", shortcuts::label(ui.ctx(), shortcuts::UNDO));
                if undo && ui.small_button(label).clicked() {
                    actions.push(Action::Edit(Edit::Undo));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    crate::widgets::truncated_label(ui, muted(text));
                });
            }
        });
    });
}

/// Is `row` on air at `now`?
/// The end instant of a row.
/// A native tool window gets its own key presses: the main window does not see them.
/// While the window draws, its keys stand in for the keys of the main window. It returns
/// the keys of the main window, to put back after the window.
fn own_keys(ui: &egui::Ui, link: &mut Linked) -> shortcuts::Pressed {
    let root = link.keys;
    if ui.ctx().viewport_id() != egui::ViewportId::ROOT {
        link.keys = shortcuts::read(ui.ctx());
    }
    root
}

pub(crate) fn row_end(row: &Row) -> Timestamp {
    let start = row.start.timestamp();
    SignedDuration::try_from(row.len).map_or(start, |d| start + d)
}

/// The time that the app spent on recent frames, for the `--perf` readout.
#[derive(Default)]
pub(crate) struct Perf {
    /// The time of the last frame, in milliseconds.
    pub last_ms: f64,
    /// A running mean, in milliseconds.
    pub mean_ms: f64,
    /// Frames since the start.
    pub frames: u64,
    /// When the readout last went to stderr.
    pub printed: Option<std::time::Instant>,
    /// The frame and block counts at that time, for the rates.
    pub window_frames: u64,
    pub window_blocks: u64,
}

/// How the meter values arrive, for the `--perf` readout.
#[derive(Default)]
pub(crate) struct MeterStats {
    /// The audio blocks that the UI saw.
    pub blocks: u64,
    pub reads: u64,
    pub age_sum_ms: f32,
    pub age_max_ms: f32,
}

/// How much of the running mean a new frame takes.
const PERF_WEIGHT: f64 = 0.1;

pub(crate) fn is_on_air(row: &Row, now: Timestamp) -> bool {
    let start = row.start.timestamp();
    let end = SignedDuration::try_from(row.len).map_or(start, |d| start + d);
    start <= now && now < end
}

/// A pair that a person gave tool window `w` by hand (its size or its place), from the
/// settings of its part.
fn saved_pair(view: &StationView, w: ToolWindow, key: &str) -> Option<[f32; 2]> {
    let v = view.settings.get(w.part())?.get(key)?.as_array()?;
    match v.as_slice() {
        [x, y] => Some([x.as_f64()? as f32, y.as_f64()? as f32]),
        _ => None,
    }
}

/// A length as m:ss. Zero means unknown and shows as "—".
pub(crate) fn mmss(d: Duration) -> String {
    if d.is_zero() {
        return "—".to_owned();
    }
    let s = d.as_secs();
    if s >= 3600 {
        // An hour or more reads h:mm:ss, never 720:00.
        return format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
    }
    format!("{}:{:02}", s / 60, s % 60)
}

/// A local time as HH:MM:SS.
pub(crate) fn hms(z: &jiff::Zoned) -> String {
    z.strftime("%H:%M:%S").to_string()
}
