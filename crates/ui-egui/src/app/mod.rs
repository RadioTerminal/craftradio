//! The main window: decks on top, the main and scheduled panes in the middle, a status line below.
//! Tool windows (cartwall, mixer, clock, history, library) open from the deck strip.

use std::path::PathBuf;
use std::time::Duration;

use egui::RichText;
use jiff::{SignedDuration, Timestamp};

mod action;
mod clipboard;
mod dialogs;
mod drops;

pub(crate) use action::{Action, FileTarget, PasteTo};
pub(crate) use clipboard::{Clipboard, CutSource};

use crate::add_menu::{self, AddDialog};
use crate::meta_editor::{self, MetaEditor};
use crate::prepared::Prepared;
use crate::rules_editor::{self, Editor};

use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{
    DeckCommand, DropTarget, Edit, FolderSource, MetaTarget, Place, Row, StationView, Status,
    ViewSource,
};
use crate::windows::{self, ToolWindow};
use crate::{cartwall, clock_window, decks, history, library, main_pane, mixer, scheduled_pane};

/// The pane that has the keyboard, for the edit shortcuts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pane {
    #[default]
    Main,
    Scheduled,
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
    meta: Option<MetaEditor>,
    add_dialog: Option<AddDialog>,
    /// Display text and lookups for the current snapshot.
    prepared: Prepared,
    /// The frame time readout, when the app runs with `--perf`.
    perf: Option<Perf>,
    decks: decks::State,
}

impl RadioApp {
    pub fn new(mut source: Box<dyn ViewSource>, clock: Box<dyn Fn() -> Timestamp>) -> Self {
        let view = source.snapshot(clock());
        let prepared = Prepared::new(&view, 0);
        let on_air = view
            .rows
            .iter()
            .find(|r| is_on_air(r, view.now.timestamp()));
        let link = Linked {
            scroll_main: on_air.map(|r| r.id),
            ..Linked::default()
        };
        let carts = cartwall::State::default();
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
            meta: None,
            add_dialog: None,
            prepared,
            perf: None,
            decks: decks::State::default(),
        }
    }

    /// Show the time of each frame in the status line.
    pub fn with_perf(mut self, on: bool) -> Self {
        self.perf = on.then(Perf::default);
        self
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
            self.prepared = Prepared::new(&self.view, self.prepared.generation + 1);
        }
    }

    pub(super) fn toast(&mut self, ctx: &egui::Context, text: String) {
        let until = ctx.input(|i| i.time) + size::TOAST_S;
        self.toast = Some((text, until));
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
        self.meta = base.map(|b| MetaEditor::new(list, b));
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
        // Only the mixer shows the stream title, so only an open mixer asks for it.
        let streaming = if self.open.get(ToolWindow::Mixer) {
            on_air.map_or_else(String::new, |r| self.source.stream_title(r))
        } else {
            String::new()
        };
        let view = &self.view;
        let open = &mut self.open;
        let link = &mut self.link;
        windows::show(ctx, ToolWindow::Cartwall, open, t, |ui| {
            cartwall::show(ui, view, &mut self.carts, link.has_clip, actions, t);
        });
        windows::show(ctx, ToolWindow::Mixer, open, t, |ui| {
            mixer::show(ui, view, &mut self.mixer, &streaming, actions, t);
        });
        windows::show(ctx, ToolWindow::Clock, open, t, |ui| {
            clock_window::show(ui, view, &mut self.clock_state, actions, t);
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
}

impl eframe::App for RadioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();
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
                decks::show(ui, &self.view, &mut self.decks, &mut actions, &t);
            });

        egui::Panel::bottom("status")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::BAR))
            .show(ui, |ui| {
                status_line(ui, &self.view, self.toast.as_ref(), self.perf.as_ref(), &t)
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
        self.dialogs(&ctx, &mut actions, &t);

        if let Some(tab) = rules_editor::show(&ctx, &mut self.editor, &mut self.link, &t) {
            let text = match self.source.apply_rules(tab) {
                Ok(()) => {
                    "preview — rules stored; they are not applied to the scheduler yet".to_owned()
                }
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
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Tokens::DARK.canvas.to_normalized_gamma_f32()
    }
}

fn status_line(
    ui: &mut egui::Ui,
    view: &StationView,
    toast: Option<&(String, f64)>,
    perf: Option<&Perf>,
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
        for (name, status) in [
            ("Local", &view.output.local),
            ("Icecast", &view.output.icecast),
        ] {
            let (dot, word, colour) = match status {
                Status::Connected => ("●", "connected".to_owned(), t.ok),
                Status::Preview => ("○", "preview (no audio)".to_owned(), t.text_muted),
                Status::NotConnected => ("○", "not connected".to_owned(), t.text_muted),
                Status::Failed(e) => ("○", format!("failed: {e}"), t.danger),
            };
            let text = format!("{name} {dot} {word}");
            ui.label(
                RichText::new(text)
                    .font(theme::text(theme::LABEL))
                    .color(colour),
            );
        }
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
        if let Some(p) = perf {
            let text = format!("frame {:.1} ms · mean {:.1} ms", p.last_ms, p.mean_ms);
            ui.label(
                RichText::new(text)
                    .font(theme::mono(theme::CAPTION))
                    .color(t.text_muted),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let zone = view.now.time_zone().iana_name().unwrap_or("local time");
            ui.label(muted(&format!("UI preview · mock data · {zone}")));
        });
    });
}

/// Is `row` on air at `now`?
/// The end instant of a row.
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
}

/// How much of the running mean a new frame takes.
const PERF_WEIGHT: f64 = 0.1;

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
