//! Tool windows. Each opens as its own native window (an egui viewport).
//! Where a platform cannot open a second window, egui shows it as a floating window instead.
//!
//! A tool window takes the size that its content needs, and follows the content as it grows
//! or shrinks. After a person resizes a window by hand, it keeps that size until "Fit to
//! content" in its settings button. The size never passes the work area; the content scrolls inside.

use egui::{Id, RichText, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId};

use crate::theme::{self, Tokens, pad, size, space};

/// List windows show about this many rows before they scroll.
const LIST_ROWS: f32 = 20.0;
/// The share of the monitor that a tool window may take.
const WORK_AREA: f32 = 0.9;
/// The settings key of a size that a person set by hand.
pub(crate) const SIZE_KEY: &str = "window_size";
/// The part setting that keeps where a person put a tool window: its top-left corner.
pub(crate) const POS_KEY: &str = "window_pos";
/// The temporary data that tells a settings section about its window.
/// The temporary data where a settings page puts its natural size each frame it draws.
const PAGE_FIT: &str = "tool_window_page_fit";
/// The temporary data that tells content whether its window has a definite size.
const DEFINITE: &str = "tool_window_definite";

/// The temporary data that offers the title bar of a native window to its header row.
const BAND: &str = "tool_window_band";
/// The temporary data where the header row says how wide it was in the title bar.
const BAND_USED: &str = "tool_window_band_used";

/// True where a native tool window draws its header row in the title bar, beside the window
/// buttons, as the Craft apps do on macOS. Elsewhere the system's title bar stays as it is.
const MERGED_TITLE_BAR: bool = cfg!(target_os = "macos");

/// Where the header row of a window goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Band {
    /// The window has no title bar of its own: the row is a row of the content.
    None,
    /// The row goes in the title bar, in this rectangle.
    At(egui::Rect),
    /// The row goes in the title bar, and this pass only measures the content.
    Measuring,
}

/// Where the header row of the window that `ui` draws goes.
pub(crate) fn title_band(ui: &egui::Ui) -> Band {
    let rect = ui
        .ctx()
        .data(|d| d.get_temp::<egui::Rect>(Id::unique(BAND)));
    match rect {
        None => Band::None,
        Some(_) if ui.is_sizing_pass() => Band::Measuring,
        Some(rect) => Band::At(rect),
    }
}

/// Draws the header row `body` in the title bar, from its right end.
pub(crate) fn in_title_band(ui: &mut egui::Ui, rect: egui::Rect, body: impl FnOnce(&mut egui::Ui)) {
    let layout = egui::Layout::right_to_left(egui::Align::Center);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    // The content is clipped to its own panel, and the title bar is above it.
    child.set_clip_rect(rect);
    body(&mut child);
    let used = child.min_rect().width();
    ui.ctx()
        .data_mut(|d| d.insert_temp(Id::unique(BAND_USED), used));
}

/// The title bar of a native window: the title after the window buttons, and room for the
/// header row, `row_w` wide, at the right. A drag on it moves the window. It returns the width
/// that the title and the window buttons need.
fn title_bar(ui: &mut egui::Ui, w: ToolWindow, row_w: f32, t: &Tokens) -> f32 {
    egui::Panel::top(("tool_window_title_bar", w.title()))
        .resizable(false)
        .show_separator_line(false)
        .exact_size(size::TITLE_BAR)
        .frame(egui::Frame::new().fill(t.canvas))
        .show(ui, |ui| {
            let band = ui.max_rect();
            // Before the header row, so the controls of the row take the pointer first.
            moves_window(ui, band);
            let inset = buttons_inset(ui);
            let left = band.left() + inset;
            let right = band.right() - pad_x();
            let row_left = (right - row_w).max(left);
            let text = RichText::new(w.title())
                .font(theme::text(theme::LABEL))
                .strong();
            let galley = egui::WidgetText::from(text).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                (row_left - space::M - left).max(0.0),
                egui::FontSelection::Default,
            );
            // The whole title, also when this frame has no room for all of it.
            let title_w = ui
                .painter()
                .layout_no_wrap(w.title().to_owned(), theme::text(theme::LABEL), t.text)
                .size()
                .x;
            let at = egui::pos2(left, band.center().y - galley.size().y / 2.0);
            ui.painter().galley(at, galley, t.text);
            let row = egui::Rect::from_x_y_ranges(left..=right, band.y_range());
            ui.ctx().data_mut(|d| d.insert_temp(Id::unique(BAND), row));
            inset + title_w + space::M + pad_x()
        })
        .inner
}

/// The room the window buttons take at the left of a title bar. A full-screen window shows no
/// buttons, so the bar starts at its margin.
fn buttons_inset(ui: &egui::Ui) -> f32 {
    if ui.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
        space::M
    } else {
        size::TITLE_BAR_BUTTONS
    }
}

/// The empty part of a title bar at `band`: a drag there moves the window, and a double click
/// zooms it. Controls drawn after this call take the pointer first.
fn moves_window(ui: &egui::Ui, band: egui::Rect) {
    let bar = ui.interact(
        band,
        ui.scope_id().with("move_window"),
        egui::Sense::click_and_drag(),
    );
    if bar.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if bar.double_clicked() {
        let zoomed = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!zoomed));
    }
}

/// The first row of the main window, drawn in its title bar: `row` after the window buttons,
/// then the window `title` at the right end, where there is room for it.
pub(crate) fn main_title_bar(
    ui: &mut egui::Ui,
    title: &str,
    t: &Tokens,
    row: impl FnOnce(&mut egui::Ui),
) {
    let size = egui::vec2(ui.available_width(), size::TITLE_BAR);
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(size, layout, |ui| {
        // The bar reaches the edges of the window, past the margin of its panel.
        let band = ui.max_rect().expand2(egui::vec2(f32::INFINITY, 0.0));
        moves_window(ui, band.intersect(ui.clip_rect()));
        let buttons = ui.clip_rect().left() + buttons_inset(ui);
        ui.add_space((buttons - ui.cursor().left()).max(0.0));
        row(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = RichText::new(title)
                .font(theme::text(theme::LABEL))
                .color(t.text_muted);
            ui.add(egui::Label::new(text).truncate().selectable(false));
        });
    });
}

/// The room between the header row and the right edge of the title bar.
fn pad_x() -> f32 {
    space::S
}

/// A tool window that the deck strip can open and close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolWindow {
    Cartwall,
    Mixer,
    Clock,
    History,
    Library,
    Alarms,
    /// Every setting of the app in one window, as in the Craft apps.
    Preferences,
}

impl ToolWindow {
    pub const ALL: [Self; 7] = [
        Self::Cartwall,
        Self::Mixer,
        Self::Clock,
        Self::History,
        Self::Library,
        Self::Alarms,
        Self::Preferences,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Cartwall => "Cartwall",
            Self::Mixer => "Mixer",
            Self::Clock => "Clock",
            Self::History => "History",
            Self::Library => "Library",
            Self::Alarms => "Alarms",
            Self::Preferences => "Preferences",
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
            Self::Alarms => crate::parts::ALARMS,
            Self::Preferences => crate::parts::PREFERENCES,
        }
    }

    fn size(self) -> [f32; 2] {
        match self {
            Self::Cartwall => [size::CARTWALL_W, size::CARTWALL_H],
            Self::Mixer => [size::MIXER_W, size::MIXER_H],
            Self::Clock => [size::CLOCK_WIN_W, size::CLOCK_WIN_H],
            Self::History => [size::HISTORY_W, size::HISTORY_H],
            Self::Library => [size::LIBRARY_W, size::LIBRARY_H],
            Self::Alarms => [size::ALARMS_W, size::ALARMS_H],
            Self::Preferences => [size::PREFERENCES_W, size::PREFERENCES_H],
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
            Self::Alarms => [size::ALARMS_MIN_W, size::ALARMS_MIN_H],
            Self::Preferences => [size::PREFERENCES_MIN_W, size::PREFERENCES_MIN_H],
        }
    }

    /// The icon in the window's header.
    pub(crate) fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Self::Cartwall => Icon::LayoutGrid,
            Self::Mixer => Icon::SlidersVertical,
            Self::Clock => Icon::Clock,
            Self::History => Icon::History,
            Self::Library => Icon::Library,
            Self::Alarms => Icon::Bell,
            Self::Preferences => Icon::Settings,
        }
    }

    /// True for the windows that are mostly a long list.
    fn is_list(self) -> bool {
        matches!(self, Self::History | Self::Library | Self::Alarms)
    }

    /// The egui id of the floating form of the window.
    pub fn area_id(self) -> Id {
        Id::unique(("tool_window", self.title()))
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
    /// The size of the content in the last frame.
    content: Option<egui::Vec2>,
    /// While a settings page shows: the size of the window before it.
    page: Option<PageFit>,
    /// The width of the header row in the title bar in the last frame.
    band_w: f32,
    /// The saved place of the window, where it opens.
    pos: Option<egui::Pos2>,
    /// The place of the window in the last frame.
    pos_seen: Option<egui::Pos2>,
    /// A move that has not been saved yet: it is saved once the window stops.
    pos_moving: bool,
}

/// Follows where the window is. A move is reported once, in the first frame the window stands
/// still again, so a drag saves one place and not one per frame.
fn track_pos(fit: &mut Fit, now: Option<egui::Pos2>) -> Option<SizeChange> {
    let now = now?;
    let mut change = None;
    match fit.pos_seen {
        Some(seen) if (seen - now).length() > size::FIT_SLACK => fit.pos_moving = true,
        Some(_) if fit.pos_moving => {
            fit.pos_moving = false;
            fit.pos = Some(now);
            change = Some(SizeChange::Moved([now.x, now.y]));
        }
        _ => {}
    }
    fit.pos_seen = Some(now);
    change
}

/// The window size while a settings page shows in place of the window content.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageFit {
    /// The size of the window before the page. Done brings it back.
    before: egui::Vec2,
    /// True after a person resized the window by hand while the page showed. The window
    /// then keeps that size until the page closes.
    dragged: bool,
}

/// What the window does in a frame, from the page state of the last frame and of this one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PageStep {
    /// No page, before or now.
    None,
    /// The page opened: keep the size of the window before it.
    Open(PageFit),
    /// The page shows: fit it, unless a person resized the window.
    Show(PageFit),
    /// The page closed: go back to this size.
    Restore(egui::Vec2),
}

fn page_step(last: Option<PageFit>, natural: Option<egui::Vec2>, now: egui::Vec2) -> PageStep {
    match (last, natural) {
        (None, None) => PageStep::None,
        (None, Some(_)) => PageStep::Open(PageFit {
            before: now,
            dragged: false,
        }),
        (Some(p), Some(_)) => PageStep::Show(p),
        (Some(p), None) => PageStep::Restore(p.before),
    }
}

/// A settings page calls this each frame it draws, with the size that it needs. Its tool
/// window then fits the page, and goes back to its old size when the page closes.
pub(crate) fn page_fits(ctx: &egui::Context, natural: egui::Vec2) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(PAGE_FIT), natural));
}

/// True when the content area of the window has a definite size: a native window, or a
/// floating window that a person sized. Content may then fill the width. In a window that
/// fits its content, content that fills the width would keep the window from shrinking.
pub(crate) fn definite_width(ui: &egui::Ui) -> bool {
    !ui.is_sizing_pass()
        && ui
            .ctx()
            .data(|d| d.get_temp::<bool>(Id::unique(DEFINITE)).unwrap_or(false))
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
    /// A person moved the window: its top-left corner is here now.
    Moved([f32; 2]),
}

/// What the settings kept of a tool window: the size and the place a person gave it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Saved {
    pub size: Option<[f32; 2]>,
    pub pos: Option<[f32; 2]>,
}

/// A row of toggle buttons, one per tool window.
pub(crate) fn toggles(ui: &mut egui::Ui, open: &mut Open, _t: &Tokens) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
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
    saved: Saved,
    t: &Tokens,
    mut add: impl FnMut(&mut egui::Ui),
) -> Option<SizeChange> {
    if !open.get(w) {
        return None;
    }
    let i = w.index();
    let mut change = None;
    {
        let fit = &mut open.fit[i];
        if !fit.loaded {
            fit.user = saved.size.map(egui::Vec2::from);
            fit.pos = saved.pos.map(egui::Pos2::from);
            fit.loaded = true;
        }
    }
    let user = open.fit[i].user;
    let min = egui::Vec2::from(w.min_size());
    let max = max_size(ctx, min);
    let stays = if ctx.embed_viewports() {
        ctx.data_mut(|d| d.insert_temp(Id::unique(DEFINITE), user.is_some()));
        let pos = open.fit[i].pos;
        let stays = floating(ctx, w, (user, pos), (min, max), t, &mut add);
        let at = ctx.memory(|m| m.area_rect(w.area_id())).map(|r| r.min);
        change = track_pos(&mut open.fit[i], at);
        stays
    } else {
        ctx.data_mut(|d| d.insert_temp(Id::unique(DEFINITE), true));
        let (stays, c) = native(ctx, w, &mut open.fit[i], (min, max), t, &mut add);
        change = c.or(change);
        stays
    };
    if !stays {
        open.set(w, false);
    }
    ctx.data_mut(|d| {
        d.insert_temp(Id::unique(DEFINITE), false);
        d.remove_temp::<egui::Vec2>(Id::unique(PAGE_FIT));
    });
    change
}

/// The floating form: an egui window that sizes itself to its content. It returns false
/// when a person closed it.
fn floating(
    ctx: &egui::Context,
    w: ToolWindow,
    (user, pos): (Option<egui::Vec2>, Option<egui::Pos2>),
    (min, max): (egui::Vec2, egui::Vec2),
    t: &Tokens,
    add: &mut impl FnMut(&mut egui::Ui),
) -> bool {
    let mut is_open = true;
    let window = egui::Window::new(w.title())
        .id(w.area_id())
        .title_bar(false)
        .collapsible(false)
        .frame(theme::dialog_frame(t))
        .min_size(min)
        .max_size(max);
    let window = match pos {
        Some(p) => window.default_pos(p),
        None => window,
    };
    // A list scrolls by itself, so its window starts at about LIST_ROWS rows and the person
    // can resize it.
    let list = egui::vec2(w.size()[0], list_height(min, max));
    let window = match (user, w.is_list()) {
        (Some(s), _) => window.resizable(true).default_size(s),
        (None, true) => window.resizable(true).default_size(list),
        (None, false) => window.auto_sized(),
    };
    window.show(ctx, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // The header, as SolidCraft's windows have: a drag on its empty part moves the window.
        if crate::chrome::header(ui, w.icon(), w.title(), true, t) {
            is_open = false;
        }
        egui::Frame::new().inner_margin(pad::PANEL).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ui.style().spacing.item_spacing.y;
            if w.is_list() {
                add(ui);
            } else {
                egui::ScrollArea::both()
                    .max_height(max.y)
                    .show(ui, |ui| add(ui));
            }
        });
    });
    is_open
}

/// Asks the window system for `size`, and waits for its answer.
fn request(ctx: &egui::Context, fit: &mut Fit, size: egui::Vec2) {
    let same = fit.seen.is_some_and(|s| !differs(s, size));
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
    fit.applied = Some(size);
    // A request for the size that the window has already brings no change.
    fit.pending = !same;
    fit.waited = 0;
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
    // The header row takes the place of the title in the title bar. The window keeps its
    // buttons, its frame and its title in the window list.
    #[cfg(target_os = "macos")]
    let builder = builder
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    let builder = match fit.pos {
        Some(p) => builder.with_position(p),
        None => builder,
    };
    let mut change = None;
    let closed = ctx.show_viewport_immediate(
        ViewportId::from_hash_of(("tool_window", w.title())),
        builder,
        |ui, class| {
            // The Edit menu's keys taken while this window had the focus.
            crate::key_route::deliver(ui.ctx());
            let frame = theme::panel_frame(t);
            if class == ViewportClass::EmbeddedWindow {
                // egui draws the window over the main window here.
                add(ui);
                return ui.input(|i| i.viewport().close_requested());
            }
            let bar_w = MERGED_TITLE_BAR.then(|| title_bar(ui, w, fit.band_w, t));
            let out = egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                // Now and then an invisible sizing pass finds the natural width of the content.
                // Content that fills the width cannot tell it, so the window can get narrower.
                // The probe is made on every frame, measuring or not: a child takes one automatic
                // id, and the content after it must keep its ids. Otherwise a text field would
                // get a new id, and lose the focus, each time the probe measures.
                let rect = egui::Rect::from_min_size(ui.min_rect().min, max);
                let mut probe = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .sizing_pass()
                        .invisible(),
                );
                if fit.measured.is_none_or(|at| at.elapsed() >= MEASURE_EVERY) {
                    add(&mut probe);
                    fit.natural_w = Some(probe.min_rect().width());
                    fit.measured = Some(std::time::Instant::now());
                }
                if w.is_list() {
                    // A list scrolls by itself: its width counts, and its height is fixed.
                    add(ui);
                    egui::vec2(ui.min_rect().width(), list_height(min, max))
                } else {
                    // A direction scrolls only when the content is larger than the window by more
                    // than the slack. A fitted window can be a fraction smaller than its content.
                    // A scroll bar then shows, takes room, hides again, and the window draws
                    // every frame.
                    let room = ui.available_size() + egui::Vec2::splat(size::FIT_SLACK);
                    let last = fit.content.unwrap_or(egui::Vec2::ZERO);
                    let scroll = [last.x > room.x, last.y > room.y];
                    let size = egui::ScrollArea::new(scroll)
                        .show(ui, |ui| add(ui))
                        .content_size;
                    fit.content = Some(size);
                    size
                }
            });
            let mut margin = frame.total_margin().sum();
            let mut content = out.inner;
            if let Some(natural) = fit.natural_w {
                content.x = content.x.min(natural);
            }
            if let Some(bar_w) = bar_w {
                // The title bar is part of the window now, and the content must not get
                // narrower than the title and the header row beside it.
                let used = ui.ctx().data_mut(|d| {
                    d.remove::<egui::Rect>(Id::unique(BAND));
                    d.remove_temp::<f32>(Id::unique(BAND_USED))
                });
                fit.band_w = used.unwrap_or(0.0);
                margin.y += size::TITLE_BAR;
                content.x = content.x.max(bar_w + fit.band_w - margin.x);
            }
            let want = desired(w, content, margin, min, max);
            // A settings page in place of the content: the window fits the page, and goes
            // back to its old size when the page closes.
            let natural = ui
                .ctx()
                .data_mut(|d| d.remove_temp::<egui::Vec2>(Id::unique(PAGE_FIT)));
            let size_now = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
            let current = size_now.or(fit.seen).unwrap_or(start);
            match page_step(fit.page, natural, current) {
                PageStep::Open(p) => fit.page = Some(p),
                PageStep::Restore(before) => {
                    fit.page = None;
                    request(ui.ctx(), fit, before);
                }
                PageStep::None | PageStep::Show(_) => {}
            }
            if let Some(now) = size_now {
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
                    if let Some(p) = &mut fit.page {
                        // A resize by hand on the page holds until the page closes. It is
                        // not the size of the window, so the settings do not keep it.
                        p.dragged = true;
                    } else {
                        fit.user = Some(now);
                        change = Some(SizeChange::Manual([now.x, now.y]));
                    }
                }
                fit.seen = Some(now);
            }
            match (fit.page, natural) {
                (Some(p), Some(natural)) => {
                    let page = desired(w, natural, margin, min, max);
                    if !p.dragged && fit.applied.is_none_or(|a| differs(a, page)) {
                        request(ui.ctx(), fit, page);
                    }
                }
                _ => {
                    if fit.user.is_none() && fit.applied.is_none_or(|a| differs(a, want)) {
                        request(ui.ctx(), fit, want);
                    }
                }
            }
            let at = ui.input(|i| i.viewport().outer_rect.map(|r| r.min));
            if let Some(moved) = track_pos(fit, at) {
                change = change.or(Some(moved));
            }
            ui.input(|i| i.viewport().close_requested())
        },
    );
    (!closed, change)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_saves_one_place_once_the_window_stands_still() {
        let mut fit = Fit::default();
        let at = |x: f32| Some(egui::pos2(x, 100.0));
        assert_eq!(
            track_pos(&mut fit, at(10.0)),
            None,
            "the first place is not a move"
        );
        assert_eq!(
            track_pos(&mut fit, at(10.0)),
            None,
            "standing still is not a move"
        );
        for x in [40.0, 80.0, 120.0] {
            assert_eq!(
                track_pos(&mut fit, at(x)),
                None,
                "nothing is saved during the drag"
            );
        }
        assert_eq!(
            track_pos(&mut fit, at(120.0)),
            Some(SizeChange::Moved([120.0, 100.0])),
            "the place where the drag ended is saved"
        );
        assert_eq!(track_pos(&mut fit, at(120.0)), None, "and only once");
    }

    #[test]
    fn a_page_keeps_the_size_before_it_and_gives_it_back() {
        let before = egui::vec2(1900.0, 600.0);
        let page = egui::vec2(560.0, 480.0);
        let PageStep::Open(open) = page_step(None, Some(page), before) else {
            panic!("the page opens");
        };
        assert_eq!(open.before, before);
        // A resize by hand while the page shows does not change the size to go back to.
        let dragged = PageFit {
            dragged: true,
            ..open
        };
        let later = egui::vec2(800.0, 700.0);
        assert_eq!(
            page_step(Some(dragged), Some(page), later),
            PageStep::Show(dragged)
        );
        assert_eq!(
            page_step(Some(dragged), None, later),
            PageStep::Restore(before)
        );
        assert_eq!(page_step(None, None, later), PageStep::None);
    }
}
