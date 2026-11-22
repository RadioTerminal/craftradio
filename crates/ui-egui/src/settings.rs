//! The pieces of Preferences, the one window with every setting, as in the Craft apps.
//!
//! Each window and pane has a settings button in its header row. It opens Preferences on that
//! part's page. In the preview, settings are local UI state, and the page says so.

use egui::{Align, Layout, RichText, Stroke};

use crate::app::Action;
use crate::icons::Icon;
use crate::preferences::Page;
use crate::theme::{self, Tokens, size, space};
use crate::widgets;

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
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            settings_button(ui, title, open);
            extra(ui);
        });
    });
}

/// A label in the left column of a settings grid.
pub(crate) fn label(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.label(
        RichText::new(text)
            .font(theme::text(theme::LABEL))
            .color(t.text_muted),
    );
}

/// The status dot of a settings tab. The shape differs too, and the tooltip says it in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Dot {
    /// Working: a filled dot.
    Good(String),
    /// Not working or not connected: a ring.
    Bad(String),
}

/// One tab of a settings page.
#[derive(Clone, Debug)]
pub(crate) struct Tab {
    pub label: &'static str,
    pub dot: Option<Dot>,
}

impl Tab {
    pub(crate) const fn plain(label: &'static str) -> Self {
        Self { label, dot: None }
    }
}

/// What a person did on a settings page this frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PageResponse {
    /// Done or Escape: go back to the window content.
    pub done: bool,
}

/// What the page knew at the end of the last frame, for the Escape of this frame. Egui drops
/// the keyboard focus on Escape before anything draws, so the page remembers it.
#[derive(Clone, Copy, Debug, Default)]
struct Last {
    /// A text field of the page had the focus: Escape only leaves the field.
    text_focus: bool,
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

/// A full-window settings page: a header with the title and Done, a rail of tabs on the left
/// and the form of the selected tab on the right. The form scrolls when it is taller than
/// the room. The page keeps one height on every tab, so the window does not jump.
pub(crate) fn page(
    ui: &mut egui::Ui,
    title: &str,
    preview: bool,
    tabs: &[Tab],
    selected: &mut usize,
    t: &Tokens,
    add: impl FnOnce(&mut egui::Ui, usize),
) -> PageResponse {
    let id = ui.scope_id().with(("settings_page", title));
    let ctx = ui.ctx().clone();
    let last: Last = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut out = PageResponse::default();
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if escape && !last.text_focus && !egui::Popup::is_any_open(&ctx) {
        out.done = true;
    }
    let top = ui.cursor().min;
    let page_h = size::SETTINGS_PAGE_H.min(ui.available_height());
    // The page has its own width: the rail, the divider and the form. It never takes the
    // whole room, so a window that fits its content can shrink to it.
    let bar = ui.spacing().scroll.allocated_width();
    let field_w = form_field_w(ui.available_width() - size::SETTINGS_RAIL - space::L - bar);
    let form_w = size::SETTINGS_LABEL + space::M + field_w;
    let page_w = size::SETTINGS_RAIL + space::L + form_w + bar;
    // The size that the page needs: its tool window fits it while the page shows.
    let natural_w = size::SETTINGS_RAIL
        + space::L
        + size::SETTINGS_LABEL
        + space::M
        + size::SETTINGS_FIELD
        + bar;
    crate::windows::page_fits(&ctx, egui::vec2(natural_w, size::SETTINGS_PAGE_H));
    // The header row runs to the right edge of a window with a definite size, so Done sits
    // at its end. A window that fits its content keeps the header at the page width.
    let header_w = if crate::windows::definite_width(ui) {
        ui.available_width().max(page_w)
    } else {
        page_w
    };
    ui.allocate_ui_with_layout(
        egui::vec2(header_w, size::CONTROL_SM),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(header_w);
            ui.label(RichText::new(title).font(theme::text(theme::CARD)).strong());
            if preview {
                let note = RichText::new(NOT_SAVED).font(theme::text(theme::CAPTION));
                ui.label(note.color(t.text_muted));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let done = RichText::new("Done")
                    .font(theme::text(theme::LABEL))
                    .strong()
                    .color(t.on_accent);
                let button = egui::Button::new(done)
                    .fill(t.accent)
                    .min_size(egui::vec2(0.0, size::CONTROL_SM));
                if ui
                    .add(button)
                    .on_hover_text("Back to the strips · Escape")
                    .clicked()
                {
                    out.done = true;
                }
            });
        },
    );
    ui.add_space(space::S);
    let body_h = (page_h - (ui.cursor().min.y - top.y)).max(size::CONTROL_LG);
    let n = tabs.len();
    if n > 0 {
        *selected = (*selected).min(n - 1);
    }
    ui.horizontal_top(|ui| {
        // The divider carries the gap between the rail and the form.
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_ui_with_layout(
            egui::vec2(size::SETTINGS_RAIL, body_h),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(size::SETTINGS_RAIL);
                ui.spacing_mut().item_spacing.y = space::XXS;
                let mut moved = false;
                for (i, tab) in tabs.iter().enumerate() {
                    let tab_id = id.with(("tab", i));
                    let Some(to) = tab_button(ui, tab_id, tab, i == *selected, t, !moved) else {
                        continue;
                    };
                    let next = match to {
                        Step::Here => i,
                        Step::Up => i.saturating_sub(1),
                        Step::Down => (i + 1).min(n - 1),
                    };
                    *selected = next;
                    if next != i {
                        ctx.memory_mut(|m| m.request_focus(id.with(("tab", next))));
                        // One step per key press: the tab that takes the focus must not
                        // read the same press again.
                        moved = true;
                    }
                }
            },
        );
        ui.add_sized([space::L, body_h], egui::Separator::default().vertical());
        egui::ScrollArea::vertical()
            .id_salt(id.with("form"))
            .max_height(body_h)
            .auto_shrink([true, false])
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S;
                    ui.set_width(form_w);
                    ui.spacing_mut().item_spacing.y = space::S;
                    add(ui, *selected);
                });
            });
    });
    let rect = egui::Rect::from_min_max(top, ui.min_rect().max);
    let focused = ctx.memory(|m| m.focused());
    let focus_inside = focused
        .and_then(|f| ctx.read_response(f))
        .is_some_and(|r| rect.contains(r.rect.center()));
    let text_focus = focus_inside && ctx.egui_wants_keyboard_input();
    let viewport = ctx.viewport_id();
    ctx.data_mut(|d| {
        d.insert_temp(id, Last { text_focus });
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

/// One tab of the rail. It returns where the selection goes: here on a click, or up and down
/// with the arrow keys while the tab has the focus.
fn tab_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    tab: &Tab,
    selected: bool,
    t: &Tokens,
    keys: bool,
) -> Option<Step> {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::CONTROL_MD),
        egui::Sense::hover(),
    );
    let resp = ui.interact(rect, id, egui::Sense::click());
    let name = match &tab.dot {
        Some(Dot::Good(w) | Dot::Bad(w)) => format!("{} — {w}", tab.label),
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
        } else if selected {
            t.accent
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
        let left = rect.left() + space::S;
        p.text(
            egui::pos2(left, rect.center().y),
            egui::Align2::LEFT_CENTER,
            tab.label,
            theme::text(theme::LABEL),
            ink,
        );
        if let Some(dot) = &tab.dot {
            let c = egui::pos2(rect.right() - space::S - size::DOT_R, rect.center().y);
            match dot {
                Dot::Good(_) => p.circle_filled(c, size::DOT_R, t.ok),
                Dot::Bad(_) => {
                    p.circle_stroke(c, size::DOT_R, Stroke::new(size::HAIRLINE, t.text_muted))
                }
            };
        }
    }
    let resp = match &tab.dot {
        Some(Dot::Good(w) | Dot::Bad(w)) => resp.on_hover_text(w.as_str()),
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

/// A row of a settings form: the label in the left column and one control in the right. The
/// control gets the one field width of the form and must fill it; its response takes the
/// label as its accessible name.
pub(crate) fn form_row(
    ui: &mut egui::Ui,
    label: &str,
    t: &Tokens,
    add: impl FnOnce(&mut egui::Ui) -> egui::Response,
) {
    let field_w = ui.available_width() - size::SETTINGS_LABEL - space::M;
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
