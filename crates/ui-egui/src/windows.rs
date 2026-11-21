//! Tool windows. Each opens as its own native window (an egui viewport).
//! Where a platform cannot open a second window, egui shows it as a floating window instead.

use egui::{RichText, ViewportBuilder, ViewportClass, ViewportId};

use crate::theme::{self, Tokens, size, space};

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

    fn size(self) -> [f32; 2] {
        match self {
            Self::Cartwall => [size::CARTWALL_W, size::CARTWALL_H],
            Self::Mixer => [size::MIXER_W, size::MIXER_H],
            Self::Clock => [size::CLOCK_WIN_W, size::CLOCK_WIN_H],
            Self::History => [size::HISTORY_W, size::HISTORY_H],
            Self::Library => [size::LIBRARY_W, size::LIBRARY_H],
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Which tool windows are open.
#[derive(Default)]
pub(crate) struct Open {
    open: [bool; ToolWindow::ALL.len()],
}

impl Open {
    pub fn get(&self, w: ToolWindow) -> bool {
        self.open[w.index()]
    }

    pub fn set(&mut self, w: ToolWindow, value: bool) {
        self.open[w.index()] = value;
    }
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

/// Show tool window `w` while it is open. `add` draws its content.
pub(crate) fn show(
    ctx: &egui::Context,
    w: ToolWindow,
    open: &mut Open,
    t: &Tokens,
    mut add: impl FnMut(&mut egui::Ui),
) {
    if !open.get(w) {
        return;
    }
    let builder = ViewportBuilder::default()
        .with_title(w.title())
        .with_inner_size(w.size())
        .with_min_inner_size([size::CARTWALL_W, size::CARTWALL_H]);
    let closed = ctx.show_viewport_immediate(
        ViewportId::from_hash_of(("tool_window", w.title())),
        builder,
        |ui, class| {
            if class == ViewportClass::EmbeddedWindow {
                // Here egui draws the window as a floating window over the main window.
                add(ui);
            } else {
                egui::CentralPanel::default()
                    .frame(theme::panel_frame(t))
                    .show(ui, |ui| add(ui));
            }
            ui.input(|i| i.viewport().close_requested())
        },
    );
    if closed {
        open.set(w, false);
    }
}
