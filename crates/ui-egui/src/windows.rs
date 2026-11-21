//! Tool windows. Each opens as its own native window (an egui viewport).
//! Where a platform cannot open a second window, egui shows it as a floating window instead.
//!
//! A tool window takes the size that its content needs, and follows the content as it grows
//! or shrinks. After a person resizes a window by hand, it keeps that size until "Fit to
//! content" in its ⚙. The size never passes the work area; the content scrolls inside.

use egui::{Id, RichText, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId};

use crate::theme::{self, Tokens, size, space};

/// List windows show about this many rows before they scroll.
const LIST_ROWS: f32 = 20.0;
/// The share of the monitor that a tool window may take.
const WORK_AREA: f32 = 0.9;
/// The settings key of a size that a person set by hand.
pub(crate) const SIZE_KEY: &str = "window_size";
/// The temporary data that tells a settings section about its window.
const USER_SIZED: &str = "tool_window_user_sized";
const FIT_TARGET: &str = "tool_window_fit_target";

/// A tool window that the deck strip can open and close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolWindow {
    Cartwall,
    Mixer,
    Clock,
    History,
    Library,
}

impl ToolWindow {
    pub const ALL: [Self; 5] = [
        Self::Cartwall,
        Self::Mixer,
        Self::Clock,
        Self::History,
        Self::Library,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Cartwall => "Cartwall",
            Self::Mixer => "Mixer",
            Self::Clock => "Clock",
            Self::History => "History",
            Self::Library => "Library",
        }
    }

    /// The settings part of the window, where its size lives.
    pub fn part(self) -> &'static str {
        match self {
            Self::Cartwall => crate::parts::CARTWALL,
            Self::Mixer => crate::parts::MIXER,
            Self::Clock => crate::parts::CLOCK,
            Self::History => crate::parts::HISTORY,
            Self::Library => crate::parts::LIBRARY,
        }
    }

    fn size(self) -> [f32; 2] {
        match self {
            Self::Cartwall => [size::CARTWALL_W, size::CARTWALL_H],
            Self::Mixer => [size::MIXER_W, size::MIXER_H],
            Self::Clock => [size::CLOCK_WIN_W, size::CLOCK_WIN_H],
            Self::History => [size::HISTORY_W, size::HISTORY_H],
            Self::Library => [size::LIBRARY_W, size::LIBRARY_H],
        }
    }

    /// The smallest size at which the window content still fits.
    pub fn min_size(self) -> [f32; 2] {
        match self {
            Self::Cartwall => [size::CARTWALL_MIN_W, size::CARTWALL_MIN_H],
            Self::Mixer => [size::MIXER_MIN_W, size::MIXER_MIN_H],
            Self::Clock => [size::CLOCK_MIN_W, size::CLOCK_MIN_H],
            Self::History => [size::HISTORY_MIN_W, size::HISTORY_MIN_H],
            Self::Library => [size::LIBRARY_MIN_W, size::LIBRARY_MIN_H],
        }
    }

    /// True for the windows that are mostly a long list.
    fn is_list(self) -> bool {
        matches!(self, Self::History | Self::Library)
    }

    /// The egui id of the floating form of the window.
    pub fn area_id(self) -> Id {
        Id::new(("tool_window", self.title()))
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The size of one tool window.
#[derive(Clone, Copy, Debug, Default)]
struct Fit {
    /// The size that the window last asked for.
    applied: Option<egui::Vec2>,
    /// The size of the window in the last frame.
    seen: Option<egui::Vec2>,
    /// A size that a person set by hand.
    user: Option<egui::Vec2>,
    /// True after the saved size was read.
    loaded: bool,
    /// True while a size request waits for the window system.
    pending: bool,
    /// The frames since the request, so a request that never settles ends.
    waited: u8,
    /// The natural width of the content, from the last sizing pass, and when it ran.
    natural_w: Option<f32>,
    measured: Option<std::time::Instant>,
}

/// A sizing pass measures the natural width of the content this often.
const MEASURE_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// A size request settles within this many frames, even when the window system takes
/// several steps or clamps it.
const SETTLE_FRAMES: u8 = 30;

/// Which tool windows are open, and their sizes.
#[derive(Default)]
pub(crate) struct Open {
    open: [bool; ToolWindow::ALL.len()],
    fit: [Fit; ToolWindow::ALL.len()],
}

impl Open {
    pub fn get(&self, w: ToolWindow) -> bool {
        self.open[w.index()]
    }

    pub fn set(&mut self, w: ToolWindow, value: bool) {
        self.open[w.index()] = value;
    }
}

/// A change of a window size that the settings must keep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SizeChange {
    /// A person resized the window to this size.
    Manual([f32; 2]),
    /// The window follows its content again.
    Fit,
}

/// A row of toggle buttons, one per tool window.
pub(crate) fn toggles(ui: &mut egui::Ui, open: &mut Open, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        let caption = RichText::new("Windows").font(theme::text(theme::CAPTION));
        ui.label(caption.color(t.text_muted));
        for w in ToolWindow::ALL {
            let on = open.get(w);
            let text = RichText::new(w.title()).font(theme::text(theme::LABEL));
            let button = egui::Button::new(text).selected(on);
            if ui.add(button).clicked() {
                open.set(w, !on);
            }
        }
    });
}

/// The "Fit to content" item of a settings section. It shows only in a window with a size
/// that a person set by hand.
pub(crate) fn fit_item(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let user = ctx.data(|d| d.get_temp::<bool>(Id::new(USER_SIZED)).unwrap_or(false));
    let target = ctx.data(|d| d.get_temp::<Id>(Id::new(FIT_TARGET)));
    if let (true, Some(target)) = (user, target)
        && ui
            .button("Fit to content")
            .on_hover_text("Size the window to its content again")
            .clicked()
    {
        ctx.data_mut(|d| d.insert_temp(target, true));
    }
}

/// The largest size of a tool window: a share of the monitor, never below the minimum.
fn max_size(ctx: &egui::Context, min: egui::Vec2) -> egui::Vec2 {
    let monitor = ctx.input(|i| i.viewport().monitor_size);
    let screen = monitor.unwrap_or_else(|| ctx.content_rect().size());
    (screen * WORK_AREA).max(min)
}

/// The height of a list window: about [`LIST_ROWS`] rows and its header.
fn list_height(min: egui::Vec2, max: egui::Vec2) -> f32 {
    (LIST_ROWS * size::ROW + 2.0 * size::CONTROL_MD)
        .max(min.y)
        .min(max.y)
}

/// The size that content of size `content` needs in window `w`.
fn desired(
    w: ToolWindow,
    content: egui::Vec2,
    frame: egui::Vec2,
    min: egui::Vec2,
    max: egui::Vec2,
) -> egui::Vec2 {
    let mut d = content + frame;
    if w.is_list() {
        d.y = d.y.min(list_height(min, max));
    }
    d.clamp(min, max)
}

fn differs(a: egui::Vec2, b: egui::Vec2) -> bool {
    (a - b).abs().max_elem() > size::FIT_SLACK
}

/// Show tool window `w` while it is open. `add` draws its content. `saved` is the size that a
/// person set by hand before. It returns a size change that the settings must keep.
pub(crate) fn show(
    ctx: &egui::Context,
    w: ToolWindow,
    open: &mut Open,
    saved: Option<[f32; 2]>,
    t: &Tokens,
    mut add: impl FnMut(&mut egui::Ui),
) -> Option<SizeChange> {
    if !open.get(w) {
        return None;
    }
    let i = w.index();
    let mut change = None;
    let fit_id = Id::new(("tool_window_fit", w.title()));
    {
        let fit = &mut open.fit[i];
        if !fit.loaded {
            fit.user = saved.map(egui::Vec2::from);
            fit.loaded = true;
        }
        if ctx
            .data_mut(|d| d.remove_temp::<bool>(fit_id))
            .unwrap_or(false)
        {
            fit.user = None;
            fit.applied = None;
            change = Some(SizeChange::Fit);
        }
    }
    let user = open.fit[i].user;
    ctx.data_mut(|d| {
        d.insert_temp(Id::new(USER_SIZED), user.is_some());
        d.insert_temp(Id::new(FIT_TARGET), fit_id);
    });
    let min = egui::Vec2::from(w.min_size());
    let max = max_size(ctx, min);
    let stays = if ctx.embed_viewports() {
        floating(ctx, w, user, (min, max), t, &mut add)
    } else {
        let (stays, c) = native(ctx, w, &mut open.fit[i], (min, max), t, &mut add);
        change = c.or(change);
        stays
    };
    if !stays {
        open.set(w, false);
    }
    ctx.data_mut(|d| d.insert_temp(Id::new(USER_SIZED), false));
    change
}

/// The floating form: an egui window that sizes itself to its content. It returns false
/// when a person closed it.
fn floating(
    ctx: &egui::Context,
    w: ToolWindow,
    user: Option<egui::Vec2>,
    (min, max): (egui::Vec2, egui::Vec2),
    t: &Tokens,
    add: &mut impl FnMut(&mut egui::Ui),
) -> bool {
    let mut is_open = true;
    let window = egui::Window::new(w.title())
        .id(w.area_id())
        .open(&mut is_open)
        .collapsible(false)
        .frame(theme::panel_frame(t))
        .min_size(min)
        .max_size(max);
    // A list scrolls by itself, so its window starts at about LIST_ROWS rows and the person
    // can resize it.
    let list = egui::vec2(w.size()[0], list_height(min, max));
    let window = match (user, w.is_list()) {
        (Some(s), _) => window.resizable(true).default_size(s),
        (None, true) => window.resizable(true).default_size(list),
        (None, false) => window.auto_sized(),
    };
    window.show(ctx, |ui| {
        if w.is_list() {
            add(ui);
        } else {
            egui::ScrollArea::both()
                .max_height(max.y)
                .show(ui, |ui| add(ui));
        }
    });
    is_open
}

/// The native form: a viewport that asks for the size of its content. It returns false when
/// a person closed it, and a size change.
fn native(
    ctx: &egui::Context,
    w: ToolWindow,
    fit: &mut Fit,
    (min, max): (egui::Vec2, egui::Vec2),
    t: &Tokens,
    add: &mut impl FnMut(&mut egui::Ui),
) -> (bool, Option<SizeChange>) {
    let start = fit
        .user
        .or(fit.applied)
        .unwrap_or_else(|| egui::Vec2::from(w.size()));
    let builder = ViewportBuilder::default()
        .with_title(w.title())
        .with_inner_size(start)
        .with_min_inner_size(min);
    let mut change = None;
    let closed = ctx.show_viewport_immediate(
        ViewportId::from_hash_of(("tool_window", w.title())),
        builder,
        |ui, class| {
            let frame = theme::panel_frame(t);
            if class == ViewportClass::EmbeddedWindow {
                // egui draws the window over the main window here.
                add(ui);
                return ui.input(|i| i.viewport().close_requested());
            }
            let out = egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                // Now and then an invisible sizing pass finds the natural width of the content.
                // Content that fills the width cannot tell it, so the window can get narrower.
                if fit.measured.is_none_or(|at| at.elapsed() >= MEASURE_EVERY) {
                    let rect = egui::Rect::from_min_size(ui.min_rect().min, max);
                    let mut probe = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .sizing_pass()
                            .invisible(),
                    );
                    add(&mut probe);
                    fit.natural_w = Some(probe.min_rect().width());
                    fit.measured = Some(std::time::Instant::now());
                }
                if w.is_list() {
                    // A list scrolls by itself: its width counts, and its height is fixed.
                    add(ui);
                    egui::vec2(ui.min_rect().width(), list_height(min, max))
                } else {
                    egui::ScrollArea::both().show(ui, |ui| add(ui)).content_size
                }
            });
            let margin = frame.total_margin().sum();
            let mut content = out.inner;
            if let Some(natural) = fit.natural_w {
                content.x = content.x.min(natural);
            }
            let want = desired(w, content, margin, min, max);
            if let Some(now) = ui.input(|i| i.viewport().inner_rect.map(|r| r.size())) {
                // A size that changes by itself to another size than the asked size is a
                // resize by hand.
                // While a request waits, the first change is its answer, even when the
                // window system clamps it.
                let moved = fit.seen.is_some_and(|s| differs(s, now));
                if fit.pending {
                    // The window system can answer in several steps. The request settles when
                    // the size matches it, or after some frames.
                    fit.waited = fit.waited.saturating_add(1);
                    let reached = fit.applied.is_some_and(|a| !differs(a, now));
                    if reached || fit.waited > SETTLE_FRAMES {
                        fit.pending = false;
                    }
                } else if moved && fit.applied.is_some_and(|a| differs(a, now)) {
                    fit.user = Some(now);
                    change = Some(SizeChange::Manual([now.x, now.y]));
                }
                fit.seen = Some(now);
            }
            if fit.user.is_none() && fit.applied.is_none_or(|a| differs(a, want)) {
                let same = fit.seen.is_some_and(|s| !differs(s, want));
                ui.ctx().send_viewport_cmd(ViewportCommand::InnerSize(want));
                fit.applied = Some(want);
                // A request for the size that the window has already brings no change.
                fit.pending = !same;
                fit.waited = 0;
            }
            ui.input(|i| i.viewport().close_requested())
        },
    );
    (!closed, change)
}
