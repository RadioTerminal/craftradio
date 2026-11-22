//! Window and dialog chrome, ported from SolidCraft's UI kit (with its maintainers'
//! permission): a header with the icon, the title and a close button over a rule, a padded
//! body, and a footer with the answers on the right and a slot on the left. A dialog sits over
//! a dimmed backdrop and blocks the window behind it.

use egui::{Align, Layout, RichText, Stroke};

use crate::icons::{self, Icon};
use crate::theme::{self, Tokens, size, space};
use crate::widgets;

/// What a dialog's main answer does: an ordinary action, or one that removes or cuts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tone {
    #[default]
    Primary,
    Danger,
}

/// A person's answer to a dialog this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The main button.
    Ok,
    /// Cancel, the close button or Escape.
    Cancel,
}

/// A modal dialog: its id, icon and title, its width and its footer buttons.
pub(crate) struct Dialog<'a> {
    id: egui::Id,
    icon: Icon,
    title: &'a str,
    width: f32,
    ok: &'a str,
    ok_enabled: bool,
    tone: Tone,
    cancel: &'a str,
}

const OPEN: &str = "chrome_dialog_open";

/// True when a dialog showed in the last frame: Escape then answers it and does not stop the
/// carts. It reads the flag once; the dialog sets it again each frame that it draws.
pub(crate) fn escape_claimed(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique(OPEN)))
        .unwrap_or(false)
}

impl<'a> Dialog<'a> {
    pub(crate) fn new(id: &str, icon: Icon, title: &'a str) -> Self {
        Self {
            id: egui::Id::unique(("dialog", id)),
            icon,
            title,
            width: size::DIALOG_W,
            ok: "OK",
            ok_enabled: true,
            tone: Tone::Primary,
            cancel: "Cancel",
        }
    }

    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// The main button's words, and whether it can be pressed.
    pub(crate) fn ok(mut self, label: &'a str, enabled: bool) -> Self {
        self.ok = label;
        self.ok_enabled = enabled;
        self
    }

    pub(crate) fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Shows the dialog for one frame. `body` draws the content and `left` the footer's left
    /// slot. It returns the answer given this frame, if any.
    pub(crate) fn show(
        self,
        ctx: &egui::Context,
        t: &Tokens,
        body: impl FnOnce(&mut egui::Ui),
        left: impl FnOnce(&mut egui::Ui),
    ) -> Option<Answer> {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(OPEN), true));
        let mut answer = None;
        let modal = egui::Modal::new(self.id)
            .backdrop_color(theme::backdrop(t))
            .frame(theme::dialog_frame(t))
            .show(ctx, |ui| {
                ui.set_width(self.width);
                ui.spacing_mut().item_spacing.y = 0.0;
                if header(ui, self.icon, self.title, true, t) {
                    answer = Some(Answer::Cancel);
                }
                egui::Frame::new()
                    .inner_margin(theme::pad::DIALOG_BODY)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = space::S;
                        ui.set_width(ui.available_width());
                        body(ui);
                    });
                if let Some(a) = self.footer(ui, t, left) {
                    answer = Some(a);
                }
            });
        // Escape answers Cancel; a click on the backdrop does nothing, so a half-filled form
        // is not lost to a stray click.
        if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(Answer::Cancel);
        }
        let _ = modal;
        answer
    }

    /// The footer: a rule, the left slot, and Cancel and the main button on the right.
    fn footer(
        &self,
        ui: &mut egui::Ui,
        t: &Tokens,
        left: impl FnOnce(&mut egui::Ui),
    ) -> Option<Answer> {
        rule(ui, t);
        let mut answer = None;
        egui::Frame::new()
            .fill(t.canvas)
            .corner_radius(egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: t.radius_lg.sw,
                se: t.radius_lg.se,
            })
            .inner_margin(theme::pad::DIALOG_FOOTER)
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), size::CONTROL_MD),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        left(ui);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (fill, ink) = match self.tone {
                                Tone::Primary => (t.accent, t.on_accent),
                                Tone::Danger => (t.danger_fill, t.text),
                            };
                            let ok = egui::Button::new(
                                RichText::new(self.ok)
                                    .font(theme::text(theme::LABEL))
                                    .strong()
                                    .color(ink),
                            )
                            .fill(fill)
                            .min_size(egui::vec2(size::DIALOG_BUTTON_W, size::CONTROL_MD));
                            if ui.add_enabled(self.ok_enabled, ok).clicked() {
                                answer = Some(Answer::Ok);
                            }
                            if !self.cancel.is_empty() {
                                let cancel = egui::Button::new(
                                    RichText::new(self.cancel).font(theme::text(theme::LABEL)),
                                )
                                .min_size(egui::vec2(size::DIALOG_BUTTON_W, size::CONTROL_MD));
                                if ui.add(cancel).clicked() {
                                    answer = Some(Answer::Cancel);
                                }
                            }
                        });
                    },
                );
            });
        answer
    }
}

/// A window header: the icon, the title, and a close button when `closeable`, over a rule. It
/// returns true when the close button was pressed. Its empty part is not interactive, so a
/// drag there moves a floating window.
pub(crate) fn header(
    ui: &mut egui::Ui,
    icon: Icon,
    title: &str,
    closeable: bool,
    t: &Tokens,
) -> bool {
    let mut closed = false;
    let (band, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::WINDOW_HEADER),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let icon_c = egui::pos2(
        band.left() + space::M + theme::ICON_MD / 2.0,
        band.center().y,
    );
    p.text(
        icon_c,
        egui::Align2::CENTER_CENTER,
        icons::glyph(icon),
        icons::font(theme::ICON_MD),
        t.accent_ink,
    );
    let close_side = size::ICON_BUTTON;
    let title_left = icon_c.x + theme::ICON_MD / 2.0 + space::S;
    let title_right = if closeable {
        band.right() - space::S - close_side - space::S
    } else {
        band.right() - space::M
    };
    let galley =
        egui::WidgetText::from(RichText::new(title).font(theme::text(theme::CARD)).strong())
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                (title_right - title_left).max(0.0),
                egui::FontSelection::Default,
            );
    let at = egui::pos2(title_left, band.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, t.text);
    if closeable {
        let rect = egui::Rect::from_center_size(
            egui::pos2(band.right() - space::S - close_side / 2.0, band.center().y),
            egui::Vec2::splat(close_side),
        );
        let r = ui
            .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                widgets::icon_button_sized(
                    ui,
                    Icon::X,
                    false,
                    "Close",
                    "Close · Escape",
                    close_side,
                )
            })
            .inner;
        closed = r.clicked();
    }
    rule(ui, t);
    closed
}

/// A hairline across the width.
fn rule(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::HAIRLINE),
        egui::Sense::hover(),
    );
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(size::HAIRLINE, t.card_border),
    );
}
