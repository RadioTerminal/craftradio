//! The main window: decks on top, the main and scheduled panes in the middle, a status line below.
//! Tool windows (cartwall, mixer, clock, history, library) open from the deck strip.

use std::time::Duration;

use egui::RichText;
use jiff::{SignedDuration, Timestamp};

use crate::rules_editor::{self, Editor};
use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{DropTarget, PlaylistTab, Row, StationView, ViewSource};
use crate::windows::{self, ToolWindow};
use crate::{cartwall, clock_window, decks, history, library, main_pane, mixer, scheduled_pane};

/// A change that a panel asks for. The app passes it to the view source.
pub(crate) enum Action {
    /// A library item dropped on a target.
    Drop { item: u64, target: DropTarget },
    /// Files dropped from the file manager onto the library.
    Import(Vec<String>),
}

/// The desktop app. It reads the station only through a [`ViewSource`].
pub struct RadioApp {
    source: Box<dyn ViewSource>,
    clock: Box<dyn Fn() -> Timestamp>,
    view: StationView,
    styled: bool,
    link: Linked,
    tab: scheduled_pane::Tab,
    open: windows::Open,
    carts: cartwall::State,
    mixer: mixer::State,
    clock_state: clock_window::State,
    history: history::State,
    library: library::State,
    editor: Option<Editor>,
    toast: Option<(String, f64)>,
}

/// The selection that the two panes share, and the scroll requests between them.
#[derive(Default)]
pub(crate) struct Linked {
    pub selected: Option<u64>,
    /// The main pane scrolls to this row once, then clears it.
    pub scroll_main: Option<u64>,
    /// The scheduled pane scrolls to this row once, then clears it.
    pub scroll_scheduled: Option<u64>,
}

impl Linked {
    pub fn pick_in_main(&mut self, id: u64) {
        self.selected = Some(id);
        self.scroll_scheduled = Some(id);
    }

    pub fn pick_in_scheduled(&mut self, id: u64) {
        self.selected = Some(id);
        self.scroll_main = Some(id);
    }
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
            tab: scheduled_pane::Tab::Upcoming,
            open: windows::Open::default(),
            carts,
            mixer: mixer::State::default(),
            clock_state: clock_window::State::default(),
            history: history::State::default(),
            library: library::State::default(),
            editor: None,
            toast: None,
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

    /// Pass the actions of this frame to the view source.
    fn run(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        for action in actions {
            match action {
                Action::Drop { item, target } => {
                    let title = self
                        .view
                        .library
                        .iter()
                        .find(|l| l.id == item)
                        .map_or_else(|| "item".to_owned(), |l| l.title.clone());
                    let place = match &target {
                        DropTarget::Main { .. } => self.view.main.name.clone(),
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
                    self.source.import_files(paths);
                    self.toast(
                        ctx,
                        format!("preview — added {n} files to Unsorted (not saved)"),
                    );
                }
            }
        }
        self.refresh(true);
    }

    fn tool_windows(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>, t: &Tokens) {
        let view = &self.view;
        let open = &mut self.open;
        windows::show(ctx, ToolWindow::Cartwall, open, t, |ui| {
            cartwall::show(ui, view, &mut self.carts, actions, t);
        });
        windows::show(ctx, ToolWindow::Mixer, open, t, |ui| {
            mixer::show(ui, view, &mut self.mixer, t);
        });
        windows::show(ctx, ToolWindow::Clock, open, t, |ui| {
            clock_window::show(ui, view, &mut self.clock_state, t);
        });
        windows::show(ctx, ToolWindow::History, open, t, |ui| {
            history::show(ui, view, &mut self.history, t);
        });
        if self.library.docked {
            // The docked library takes the place of its window.
            open.set(ToolWindow::Library, false);
        } else {
            windows::show(ctx, ToolWindow::Library, open, t, |ui| {
                library::show(ui, view, &mut self.library, actions, t);
            });
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
        let mut edit: Option<PlaylistTab> = None;

        egui::Panel::top("decks")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::GUTTER))
            .show(ui, |ui| {
                windows::toggles(ui, &mut self.open, &t);
                ui.add_space(space::S);
                decks::show(ui, &self.view, &t);
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
                    library::show(ui, &self.view, &mut self.library, &mut actions, &t);
                });
        }

        let width = ui.available_width();
        egui::Panel::right("scheduled")
            .resizable(true)
            .default_size(width * size::SPLIT_FRACTION)
            .min_size(size::SPLIT_MIN)
            .frame(theme::panel_frame(&t))
            .show(ui, |ui| {
                let view = &self.view;
                let cog =
                    scheduled_pane::show(ui, view, &mut self.tab, &mut self.link, &mut actions, &t);
                if let Some(i) = cog {
                    edit = view.tabs.get(i).cloned();
                }
            });

        egui::CentralPanel::default()
            .frame(theme::panel_frame(&t))
            .show(ui, |ui| {
                if main_pane::show(ui, &self.view, &mut self.link, &mut actions, &t) {
                    edit = Some(self.view.main.clone());
                }
            });

        self.tool_windows(&ctx, &mut actions, &t);

        if let Some(tab) = edit {
            self.editor = Some(Editor::new(&tab));
        }
        if rules_editor::show(&ctx, &mut self.editor, self.source.as_mut(), &t) {
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

/// A length as m:ss.
pub(crate) fn mmss(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// A local time as HH:MM:SS.
pub(crate) fn hms(z: &jiff::Zoned) -> String {
    z.strftime("%H:%M:%S").to_string()
}
