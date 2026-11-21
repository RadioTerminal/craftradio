//! The window: decks on top, the main and scheduled panes in the middle, the cartwall below.

use std::time::Duration;

use egui::RichText;
use jiff::{SignedDuration, Timestamp};

use crate::theme::{self, Tokens, pad, size, space};
use crate::view::{Row, StationView, ViewSource};
use crate::{cartwall, decks, main_pane, scheduled_pane};

/// The desktop app. It reads the station only through a [`ViewSource`].
pub struct RadioApp {
    source: Box<dyn ViewSource>,
    clock: Box<dyn Fn() -> Timestamp>,
    view: StationView,
    styled: bool,
    link: Linked,
    tab: scheduled_pane::Tab,
    carts: cartwall::State,
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
            carts,
        }
    }

    /// Take a new snapshot when the clock reaches a new second.
    fn refresh(&mut self) {
        let now = (self.clock)();
        if now.as_second() != self.view.now.timestamp().as_second() {
            self.view = self.source.snapshot(now);
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
        self.refresh();
        ctx.request_repaint_after(Duration::from_secs(1));
        let t = Tokens::get(&ctx);

        egui::Panel::top("decks")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::GUTTER))
            .show(ui, |ui| decks::show(ui, &self.view, &t));

        egui::Panel::bottom("status")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::BAR))
            .show(ui, |ui| status_line(ui, &self.view, &t));

        egui::Panel::bottom("cartwall")
            .resizable(false)
            .frame(theme::bar_frame(&t, pad::GUTTER))
            .show(ui, |ui| cartwall::show(ui, &self.view, &mut self.carts, &t));

        let width = ui.available_width();
        egui::Panel::right("scheduled")
            .resizable(true)
            .default_size(width * size::SPLIT_FRACTION)
            .min_size(size::SPLIT_MIN)
            .frame(theme::panel_frame(&t))
            .show(ui, |ui| {
                scheduled_pane::show(ui, &self.view, &mut self.tab, &mut self.link, &t);
            });

        egui::CentralPanel::default()
            .frame(theme::panel_frame(&t))
            .show(ui, |ui| main_pane::show(ui, &self.view, &mut self.link, &t));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Tokens::DARK.canvas.to_normalized_gamma_f32()
    }
}

fn status_line(ui: &mut egui::Ui, view: &StationView, t: &Tokens) {
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
