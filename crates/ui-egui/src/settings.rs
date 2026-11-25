//! The pieces of Preferences, the one window with every setting, as in the Craft apps.
//!
//! Each window and pane has a settings button in its header row. It opens Preferences on that
//! part's page. In the preview, settings are local UI state, and the page says so.

use egui::{Align, Layout, RichText, Stroke};

use crate::app::Action;
use crate::icons::Icon;
use crate::preferences::Page;
use crate::theme::{self, Tokens, size, space};
use crate::view::Health;
use crate::widgets;
use crate::windows::Band;

pub(crate) const NOT_SAVED: &str = "preview — not saved";

/// A header row whose settings button opens Preferences on `page`: the button far right, then
/// `extra`, the part's own controls and notes.
pub(crate) fn header_to(
    ui: &mut egui::Ui,
    part: &str,
    page: Page,
    actions: &mut Vec<Action>,
    extra: impl FnOnce(&mut egui::Ui),
) {
    let mut open = false;
    header(ui, part, &mut open, extra);
    if open {
        actions.push(Action::OpenPreferences(page));
    }
}

/// A header row with only the settings button, which opens Preferences on `page`.
pub(crate) fn opens_preferences(
    ui: &mut egui::Ui,
    part: &str,
    page: Page,
    actions: &mut Vec<Action>,
) {
    header_to(ui, part, page, actions, |_| {});
}

/// The settings button of `part`. Its accessible name is the tooltip text.
pub(crate) fn settings_button(ui: &mut egui::Ui, part: &str, open: &mut bool) {
    settings_button_sized(ui, part, open, size::ICON_BUTTON);
}

/// The settings toggle at the square `side`, for a row whose icon buttons share one size.
pub(crate) fn settings_button_sized(ui: &mut egui::Ui, part: &str, open: &mut bool, side: f32) {
    let name = format!("{part} preferences");
    let resp = widgets::icon_button_sized(
        ui,
        Icon::Settings,
        *open,
        &name,
        &format!("Open {part} in Preferences"),
        side,
    );
    if resp.clicked() {
        *open = !*open;
    }
}

/// A header row: the title on the left, then `extra` and the settings button on the right.
pub(crate) fn header(
    ui: &mut egui::Ui,
    title: &str,
    open: &mut bool,
    extra: impl FnOnce(&mut egui::Ui),
) {
    // The window title bar names the part, so the header row does not repeat it:
    // The settings button far right, then the part's own controls and notes.
    let body = |ui: &mut egui::Ui| {
        settings_button(ui, title, open);
        extra(ui);
    };
    match crate::windows::title_band(ui) {
        // The window's title bar has room for the row, so it takes no row of the content.
        Band::At(rect) => crate::windows::in_title_band(ui, rect, body),
        Band::Measuring => {}
        Band::None => {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), body);
            });
        }
    }
}

/// A label in the left column of a settings grid.
pub(crate) fn label(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.label(
        RichText::new(text)
            .font(theme::text(theme::LABEL))
            .color(t.text_muted),
    );
}

/// The status dot of a settings tab: how it looks and, in the tooltip, what it says.
pub(crate) type Dot = crate::view::PageStatus;

/// One page of the rail: its name, icon and group, what the heading says about it, the words a
/// search finds it by, and whether Restore page defaults can reset it.
#[derive(Clone, Debug)]
pub(crate) struct Tab {
    pub label: &'static str,
    pub icon: Icon,
    /// The heading of the group this page belongs to in the rail.
    pub group: &'static str,
    /// One line under the page heading.
    pub description: &'static str,
    /// More words a search finds the page by, beyond its name, group and description.
    pub keywords: &'static str,
    pub dot: Option<Dot>,
    /// Ok when Restore page defaults resets the page, or why it cannot.
    pub restore: Result<(), &'static str>,
}

impl Tab {
    /// True when every word of `query` is in the page's name, group, description or keywords.
    pub(crate) fn matches(&self, query: &str) -> bool {
        let hay = format!(
            "{} {} {} {}",
            self.label, self.group, self.description, self.keywords
        )
        .to_lowercase();
        query
            .to_lowercase()
            .split_whitespace()
            .all(|word| hay.contains(word))
    }
}

/// What a person did on a settings page this frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PageResponse {
    /// Done or Escape: close the page.
    pub done: bool,
    /// Restore page defaults for the selected page.
    pub restore: bool,
}

/// What the page knew at the end of the last frame, for the Escape of this frame. Egui drops
/// the keyboard focus on Escape before anything draws, so the page remembers it.
#[derive(Clone, Copy, Debug, Default)]
struct Last {
    /// A text field of the page had the focus: Escape only leaves the field.
    text_focus: bool,
    /// The search field had the focus: Escape also clears the search.
    search_focus: bool,
}

/// A settings page that claims Escape in a viewport: the pointer is over it, or a control of
/// it has the focus. Escape then closes the page and does not stop the carts.
#[derive(Clone, Copy, Debug)]
struct Claim {
    viewport: egui::ViewportId,
    rect: egui::Rect,
    focus_inside: bool,
}

const CLAIM: &str = "settings_page_escape";

/// True when an open settings page takes the Escape of this viewport. It reads the claim of
/// the last frame once; the page claims again each frame that it draws.
pub(crate) fn escape_claimed(ctx: &egui::Context) -> bool {
    let Some(Some(c)) = ctx.data_mut(|d| d.remove_temp::<Option<Claim>>(egui::Id::unique(CLAIM)))
    else {
        return false;
    };
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    c.viewport == ctx.viewport_id()
        && (c.focus_inside || pointer.is_some_and(|p| c.rect.contains(p)))
}

/// Where a settings page is: the selected tab and the search text.
#[derive(Clone, Debug, Default)]
pub(crate) struct Nav {
    pub selected: usize,
    pub search: String,
    /// The Search Preferences shortcut was pressed this frame: the search field takes the focus.
    pub find: bool,
}

/// The indices of the tabs that a search for `query` keeps, in rail order.
pub(crate) fn visible_tabs(tabs: &[Tab], query: &str) -> Vec<usize> {
    let query = query.trim();
    (0..tabs.len())
        .filter(|&i| query.is_empty() || tabs[i].matches(query))
        .collect()
}

/// A full-window settings page, laid out as the Craft apps' Preferences: a top bar with the
/// search field, a rail of pages in groups on the left, the selected page's heading and form on
/// the right, and a footer with Restore page defaults and Done. The form scrolls when it is
/// taller than the room. The page keeps one height on every tab, so the window does not jump.
pub(crate) fn page(
    ui: &mut egui::Ui,
    title: &str,
    preview: bool,
    tabs: &[Tab],
    nav: &mut Nav,
    t: &Tokens,
    add: impl FnOnce(&mut egui::Ui, usize, bool),
) -> PageResponse {
    let Nav {
        selected,
        search,
        find,
    } = nav;
    let id = ui.scope_id().with(("settings_page", title));
    let search_id = id.with("search");
    let ctx = ui.ctx().clone();
    let last: Last = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut out = PageResponse::default();
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if escape && last.search_focus && !search.is_empty() {
        search.clear();
    } else if escape && !last.text_focus && !egui::Popup::is_any_open(&ctx) {
        out.done = true;
    }
    if *find {
        ctx.memory_mut(|m| m.request_focus(search_id));
    }
    let top = ui.cursor().min;
    let bar = ui.spacing().scroll.allocated_width();
    let field_w = form_field_w(ui.available_width() - size::SETTINGS_RAIL - space::XL * 2.0 - bar);
    let form_w = size::SETTINGS_LABEL + space::M + field_w;
    let page_w = size::SETTINGS_RAIL + space::XL * 2.0 + form_w + bar;
    // The size that the page needs: its tool window fits it while the page shows.
    let natural_w = size::SETTINGS_RAIL
        + space::XL * 2.0
        + size::SETTINGS_LABEL
        + space::M
        + size::SETTINGS_FIELD
        + bar;
    crate::windows::page_fits(&ctx, egui::vec2(natural_w, size::SETTINGS_PAGE_H));
    // The bars run to the right edge of a window with a definite size. A window that fits its
    // content keeps them at the page width.
    // A window with a definite size (native, or sized by hand) is filled: the form widens and
    // the rail and form grow to its height. A window that fits its content keeps the page at
    // its natural size, or the window could never shrink.
    let definite = crate::windows::definite_width(ui);
    let full_w = if definite {
        ui.available_width().max(page_w)
    } else {
        page_w
    };
    let form_w = if definite {
        full_w - size::SETTINGS_RAIL - space::XL - bar
    } else {
        form_w
    };
    let room_h = ui.available_height();
    let page_h = if definite && room_h.is_finite() {
        room_h.max(size::SETTINGS_PAGE_MIN_H)
    } else {
        size::SETTINGS_PAGE_H.min(room_h)
    };

    // The top bar: the preview note on the left, the search field on the right.
    let search_resp = ui
        .allocate_ui_with_layout(
            egui::vec2(full_w, size::CONTROL_MD),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.set_width(full_w);
                let r = search_field(ui, search_id, search, t);
                if preview {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        chip(ui, Icon::Eye, NOT_SAVED, t);
                    });
                }
                r
            },
        )
        .inner;
    ui.add_space(space::S);
    rule(ui, full_w, t);

    let visible = visible_tabs(tabs, search);
    if !visible.is_empty() && !visible.contains(selected) {
        *selected = visible[0];
    }
    let footer_h = size::CONTROL_MD + space::M * 2.0 + size::HAIRLINE;
    let body_h = (page_h - (ui.cursor().min.y - top.y) - footer_h).max(size::CONTROL_LG);
    ui.horizontal_top(|ui| {
        // The divider carries the gap between the rail and the form.
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_ui_with_layout(
            egui::vec2(size::SETTINGS_RAIL, body_h),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(size::SETTINGS_RAIL);
                ui.set_min_height(body_h);
                egui::ScrollArea::vertical()
                    .id_salt(id.with("rail"))
                    .max_height(body_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| rail(ui, &ctx, id, tabs, &visible, selected, t));
            },
        );
        ui.add_sized([space::XL, body_h], egui::Separator::default().vertical());
        egui::ScrollArea::vertical()
            .id_salt(id.with(("form", *selected)))
            .max_height(body_h)
            .auto_shrink([true, false])
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S;
                    ui.set_width(form_w);
                    ui.spacing_mut().item_spacing.y = space::S;
                    ui.add_space(space::M);
                    match tabs.get(*selected).filter(|_| !visible.is_empty()) {
                        Some(tab) => {
                            heading(ui, tab, t);
                            add(ui, *selected, !search.is_empty());
                        }
                        None => {
                            ui.label(
                                RichText::new(format!("No settings match “{}”.", search.trim()))
                                    .color(t.text_muted),
                            );
                        }
                    }
                    ui.add_space(space::L);
                });
            });
    });

    // The footer: Restore page defaults on the left, Done on the right.
    rule(ui, full_w, t);
    ui.add_space(space::M);
    let restore = match tabs.get(*selected).filter(|_| !visible.is_empty()) {
        Some(tab) => tab.restore,
        None => Err("No page is shown."),
    };
    ui.allocate_ui_with_layout(
        egui::vec2(full_w, size::CONTROL_MD),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(full_w);
            let words = RichText::new(crate::icons::lead(Icon::RotateCcw, "Restore page defaults"))
                .font(theme::text(theme::LABEL));
            let button = egui::Button::new(words).min_size(egui::vec2(0.0, size::CONTROL_MD));
            let r = ui.add_enabled(restore.is_ok(), button);
            // The icon is drawn in the label; the name is the words alone.
            // A disabled button says why in its description too, for a screen reader.
            ui.ctx().accesskit_node_builder(r.id, |node| {
                node.set_label("Restore page defaults");
                if let Err(why) = restore {
                    node.set_description(why);
                }
            });
            let r = match restore {
                Ok(()) => r.on_hover_text("Set this page's settings back to their defaults"),
                Err(why) => r.on_disabled_hover_text(why),
            };
            if r.clicked() {
                out.restore = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let done = RichText::new("Done")
                    .font(theme::text(theme::LABEL))
                    .strong()
                    .color(t.on_accent);
                let button = egui::Button::new(done)
                    .fill(t.accent)
                    .min_size(egui::vec2(size::SETTINGS_DONE_W, size::CONTROL_MD));
                if ui
                    .add(button)
                    .on_hover_text("Close Preferences · Escape")
                    .clicked()
                {
                    out.done = true;
                }
            });
        },
    );
    ui.add_space(space::M);

    let rect = egui::Rect::from_min_max(top, ui.min_rect().max);
    let focused = ctx.memory(|m| m.focused());
    let focus_inside = focused
        .and_then(|f| ctx.read_response(f))
        .is_some_and(|r| rect.contains(r.rect.center()));
    let text_focus = focus_inside && ctx.egui_wants_keyboard_input();
    let viewport = ctx.viewport_id();
    let search_focus = search_resp.has_focus();
    ctx.data_mut(|d| {
        d.insert_temp(
            id,
            Last {
                text_focus,
                search_focus,
            },
        );
        d.insert_temp(
            egui::Id::unique(CLAIM),
            Some(Claim {
                viewport,
                rect,
                focus_inside,
            }),
        );
    });
    out
}

/// A hairline across the page.
fn rule(ui: &mut egui::Ui, w: f32, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, size::HAIRLINE), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(size::HAIRLINE, t.card_border),
    );
}

/// A small rounded note with an icon, as the page's state beside the heading.
fn chip(ui: &mut egui::Ui, icon: Icon, text: &str, t: &Tokens) {
    egui::Frame::new()
        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
        .corner_radius(t.radius_lg)
        .inner_margin(theme::pad::CHIP)
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::icons::lead(icon, text))
                    .font(theme::text(theme::CAPTION))
                    .color(t.text_muted),
            );
        });
}

/// The search field of the top bar: a search icon and the placeholder, and the key of Search
/// Preferences at the right end while it is empty.
fn search_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    search: &mut String,
    t: &Tokens,
) -> egui::Response {
    let hint = format!("{}  Search preferences", crate::icons::glyph(Icon::Search));
    let edit = egui::TextEdit::singleline(search)
        .id(id)
        .hint_text(RichText::new(hint).color(t.text_muted))
        .desired_width(size::SETTINGS_SEARCH_W)
        .min_size(egui::vec2(size::SETTINGS_SEARCH_W, size::CONTROL_MD))
        .align(egui::Align2::LEFT_CENTER);
    let r = ui.add(edit);
    let keys = crate::shortcuts::label(ui.ctx(), crate::shortcuts::FIND);
    if search.is_empty() && !keys.is_empty() {
        let galley =
            ui.painter()
                .layout_no_wrap(keys.clone(), theme::mono(theme::CAPTION), t.text_muted);
        let pad = theme::pad::CHIP;
        let size = galley.size() + egui::vec2(pad.sum().x, pad.sum().y);
        let cap = egui::Rect::from_min_size(
            egui::pos2(
                r.rect.right() - space::S - size.x,
                r.rect.center().y - size.y / 2.0,
            ),
            size,
        );
        let p = ui.painter();
        p.rect(
            cap,
            t.radius_sm,
            t.field,
            Stroke::new(size::HAIRLINE, t.card_border),
            egui::StrokeKind::Inside,
        );
        p.galley(
            cap.min + egui::vec2(f32::from(pad.left), f32::from(pad.top)),
            galley,
            t.text_muted,
        );
    }
    // The field has no label beside it, as in the Craft apps: its placeholder is its name.
    ui.ctx()
        .accesskit_node_builder(r.id, |node| node.set_label("Search preferences"));
    if keys.is_empty() {
        r.on_hover_text("Search every page")
    } else {
        r.on_hover_text(format!("Search every page · {keys}"))
    }
}

/// The selected page's heading: its name, large, and one line about it. The name is painted,
/// not a label: the selected tab of the rail already names the page, and two controls with one
/// name would be ambiguous to a screen reader.
fn heading(ui: &mut egui::Ui, tab: &Tab, t: &Tokens) {
    let title = egui::WidgetText::from(
        RichText::new(tab.label)
            .font(theme::text(theme::TITLE))
            .strong(),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        ui.available_width(),
        egui::FontSelection::Default,
    );
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), title.size().y),
        egui::Sense::hover(),
    );
    ui.painter().galley(rect.min, title, t.text);
    ui.add(
        egui::Label::new(
            RichText::new(tab.description)
                .font(theme::text(theme::BODY))
                .color(t.text_muted),
        )
        .wrap(),
    );
    ui.add_space(space::S);
}

/// The rail: a heading for each group, then its pages. The arrow keys move between the pages
/// that the search keeps.
fn rail(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    id: egui::Id,
    tabs: &[Tab],
    visible: &[usize],
    selected: &mut usize,
    t: &Tokens,
) {
    ui.spacing_mut().item_spacing.y = space::XXS;
    ui.add_space(space::S);
    let mut moved = false;
    let mut group = None;
    for (pos, &i) in visible.iter().enumerate() {
        let tab = &tabs[i];
        if group != Some(tab.group) {
            group = Some(tab.group);
            ui.add_space(if pos == 0 { 0.0 } else { space::S });
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), size::SETTINGS_GROUP_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add_space(space::M);
                    ui.label(
                        RichText::new(tab.group.to_uppercase())
                            .font(theme::text(theme::CAPTION))
                            .strong()
                            .extra_letter_spacing(size::CAPS_TRACKING)
                            .color(t.text_muted),
                    );
                },
            );
        }
        let tab_id = id.with(("tab", i));
        let Some(to) = tab_button(ui, tab_id, tab, i == *selected, t, !moved) else {
            continue;
        };
        let next = match to {
            Step::Here => i,
            Step::Up => visible[pos.saturating_sub(1)],
            Step::Down => visible[(pos + 1).min(visible.len() - 1)],
        };
        *selected = next;
        if next != i {
            ctx.memory_mut(|m| m.request_focus(id.with(("tab", next))));
            // One step per key press: the tab that takes the focus must not read the same
            // press again.
            moved = true;
        }
    }
}

/// The width of every control of a form, for the room beside the tab rail. One width for all
/// rows, from the theme, narrower only in a narrow window.
fn form_field_w(room: f32) -> f32 {
    (room - size::SETTINGS_LABEL - space::M).clamp(size::SETTINGS_FIELD_MIN, size::SETTINGS_FIELD)
}

enum Step {
    Here,
    Up,
    Down,
}

/// A page's status in words for its Status line, led by an icon and in the dot's colour.
pub(crate) fn status_line(ui: &mut egui::Ui, status: &Dot, t: &Tokens) {
    let (icon, ink) = match status.health {
        Health::Good => (Icon::CircleDot, t.ok),
        Health::Busy => (Icon::RefreshCw, t.warn),
        Health::Fault => (Icon::Circle, t.danger),
        Health::Idle | Health::Off => (Icon::Circle, t.text_muted),
    };
    form_note(
        ui,
        RichText::new(crate::icons::lead(icon, &status.words))
            .font(theme::text(theme::LABEL))
            .color(ink),
    );
}

/// A status dot: green, orange, red or muted, and each in a shape of its own, so the colour is
/// never the only sign.
fn draw_dot(p: &egui::Painter, c: egui::Pos2, health: Health, t: &Tokens) {
    match health {
        Health::Good => {
            p.circle_filled(c, size::DOT_R, t.ok);
        }
        Health::Busy => {
            p.circle_stroke(c, size::DOT_R, Stroke::new(size::HAIRLINE, t.warn));
            p.circle_filled(c, size::DOT_R / 2.0, t.warn);
        }
        Health::Fault => {
            let square = egui::Rect::from_center_size(c, egui::Vec2::splat(size::DOT_R * 2.0));
            p.rect_filled(square, 0.0, t.danger);
        }
        Health::Idle => {
            p.circle_stroke(c, size::DOT_R, Stroke::new(size::HAIRLINE, t.text_muted));
        }
        Health::Off => {
            p.circle_filled(c, size::DOT_R / 2.0, t.text_muted);
        }
    }
}

/// One page of the rail: its icon and name, the selected one filled with the accent. It
/// returns where the selection goes: here on a click, or up and down with the arrow keys while
/// the tab has the focus.
fn tab_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    tab: &Tab,
    selected: bool,
    t: &Tokens,
    keys: bool,
) -> Option<Step> {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::SETTINGS_RAIL_ROW),
        egui::Sense::hover(),
    );
    let resp = ui.interact(rect, id, egui::Sense::click());
    let name = match &tab.dot {
        Some(dot) => format!("{} — {}", tab.label, dot.words),
        None => tab.label.to_owned(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &name));
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            t.selection
        } else if resp.hovered() {
            t.hover
        } else {
            egui::Color32::TRANSPARENT
        };
        let edge = if resp.has_focus() {
            t.accent_ink
        } else {
            egui::Color32::TRANSPARENT
        };
        let p = ui.painter();
        p.rect(
            rect,
            t.radius,
            fill,
            Stroke::new(size::HAIRLINE, edge),
            egui::StrokeKind::Inside,
        );
        let ink = if selected { t.accent_ink } else { t.text };
        let left = rect.left() + space::M;
        p.text(
            egui::pos2(left + theme::ICON_MD / 2.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            crate::icons::glyph(tab.icon),
            crate::icons::font(theme::ICON_MD),
            if selected { t.accent_ink } else { t.text_muted },
        );
        let label = RichText::new(tab.label).font(theme::text(theme::BODY));
        let label = if selected { label.strong() } else { label };
        let galley = egui::WidgetText::from(label).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            rect.width() - space::M * 3.0 - theme::ICON_MD - size::DOT_R * 2.0,
            egui::FontSelection::Default,
        );
        let at = egui::pos2(
            left + theme::ICON_MD + space::M,
            rect.center().y - galley.size().y / 2.0,
        );
        p.galley(at, galley, ink);
        if let Some(dot) = &tab.dot {
            let c = egui::pos2(rect.right() - space::M - size::DOT_R, rect.center().y);
            draw_dot(p, c, dot.health, t);
        }
    }
    let resp = match &tab.dot {
        Some(dot) => resp.on_hover_text(dot.words.as_str()),
        None => resp,
    };
    if keys && resp.has_focus() {
        // The arrow keys move the selection; egui must not move the focus as well.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter {
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (up, down) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if up {
            return Some(Step::Up);
        }
        if down {
            return Some(Step::Down);
        }
    }
    resp.clicked().then_some(Step::Here)
}

/// A folding section of a page: a chevron, the title in capitals and a rule to the right edge,
/// then its rows. It starts open; a click or Space on the title folds it.
pub(crate) fn section<R>(
    ui: &mut egui::Ui,
    title: &str,
    t: &Tokens,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let id = ui.scope_id().with(("section", title));
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
    ui.add_space(space::M);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::CONTROL_SM),
        egui::Sense::hover(),
    );
    let resp = ui.interact(rect, id.with("title"), egui::Sense::click());
    let open = state.is_open();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, title));
    if resp.clicked() {
        state.toggle(ui);
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let ink = if resp.hovered() || resp.has_focus() {
            t.text
        } else {
            t.text_muted
        };
        let chevron = if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        p.text(
            egui::pos2(rect.left() + theme::ICON_SM / 2.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            crate::icons::glyph(chevron),
            crate::icons::font(theme::ICON_SM),
            ink,
        );
        let words = RichText::new(title.to_uppercase())
            .font(theme::text(theme::LABEL))
            .strong()
            .extra_letter_spacing(size::CAPS_TRACKING);
        let galley = egui::WidgetText::from(words).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            rect.width(),
            egui::FontSelection::Default,
        );
        let x = rect.left() + theme::ICON_SM + space::S;
        let text_w = galley.size().x;
        p.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
        let from = x + text_w + space::M;
        if from < rect.right() {
            p.hline(
                from..=rect.right(),
                rect.center().y,
                Stroke::new(size::HAIRLINE, t.card_border),
            );
        }
        if resp.has_focus() {
            p.rect_stroke(
                rect,
                t.radius_sm,
                Stroke::new(size::HAIRLINE, t.accent_ink),
                egui::StrokeKind::Inside,
            );
        }
    }
    state.store(ui.ctx());
    state
        .show_body_unindented(ui, |ui| {
            ui.add_space(space::XS);
            add(ui)
        })
        .map(|r| r.inner)
}

/// A row of a settings form: the label in the left column and one control in the right. The
/// control gets the one field width of the form and must fill it; its response takes the
/// label as its accessible name.
pub(crate) fn form_row(
    ui: &mut egui::Ui,
    label: &str,
    t: &Tokens,
    add: impl FnOnce(&mut egui::Ui) -> egui::Response,
) {
    let field_w =
        (ui.available_width() - size::SETTINGS_LABEL - space::M).min(size::SETTINGS_FIELD);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let l = ui
            .allocate_ui_with_layout(
                egui::vec2(size::SETTINGS_LABEL, size::CONTROL_SM),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(size::SETTINGS_LABEL);
                    ui.add(
                        egui::Label::new(
                            RichText::new(label)
                                .font(theme::text(theme::LABEL))
                                .color(t.text_muted),
                        )
                        .wrap(),
                    )
                },
            )
            .inner;
        ui.allocate_ui_with_layout(
            egui::vec2(field_w, size::CONTROL_SM),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_width(field_w);
                add(ui).labelled_by(l.id);
            },
        );
    });
}

/// A full-width line of a form: a note or a status above the rows.
pub(crate) fn form_note(ui: &mut egui::Ui, text: RichText) {
    ui.add(egui::Label::new(text).wrap());
}
