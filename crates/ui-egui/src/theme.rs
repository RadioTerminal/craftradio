//! The one scale every panel draws from: colours, text sizes, gaps, padding and layout sizes.
//! Panels never write a colour or a size as a number.
//!
//! The scale follows the craft design system, with a dark broadcast palette.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};

/// Heading and text sizes (points).
pub const TITLE: f32 = 26.0;
pub const SECTION: f32 = 18.0;
pub const CARD: f32 = 16.0;
pub const LEAD: f32 = 17.0;
pub const BODY: f32 = 15.0;
pub const SMALL: f32 = 14.0;
pub const LABEL: f32 = 13.0;
pub const CAPTION: f32 = 12.0;
pub const TINY: f32 = 11.0;
/// Code, times and lengths (monospace).
pub const CODE: f32 = 12.5;

/// Icon sizes (points). They sit on the same scale as the text sizes.
pub const ICON_SM: f32 = 14.0;
pub const ICON_MD: f32 = 16.0;
pub const ICON_LG: f32 = 20.0;

/// Gaps between things.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Padding inside frames, built from the gaps.
pub mod pad {
    use egui::Margin;

    pub const TIGHT: Margin = Margin::same(4);
    pub const CHIP: Margin = Margin::symmetric(8, 2);
    pub const ROW: Margin = Margin::symmetric(8, 4);
    pub const CARD: Margin = Margin::same(12);
    /// A deck card: one step more room than a plain card.
    pub const DECK: Margin = Margin::same(16);
    pub const PANEL: Margin = Margin::same(12);
    /// A group of controls inside a card.
    pub const GROUP: Margin = Margin::same(8);
    pub const BAR: Margin = Margin::symmetric(12, 8);
    pub const GUTTER: Margin = Margin::symmetric(16, 12);
}

/// Layout sizes of the console.
pub mod size {
    /// Control heights (small, medium and large).
    pub const CONTROL_SM: f32 = 24.0;
    pub const CONTROL_MD: f32 = 32.0;
    pub const CONTROL_LG: f32 = 40.0;
    /// The global transport: the largest control of the console.
    pub const CONTROL_HERO: f32 = 48.0;
    /// The square of an icon button. It is the same in every state.
    pub const ICON_BUTTON: f32 = 28.0;
    /// The square of every icon button on a deck card (play, pause, settings, armed). One
    /// size, so the row of a card looks the same whichever button shows.
    pub const DECK_BUTTON: f32 = CONTROL_MD;
    /// An armed on-air button waits this long for the second press, in seconds.
    pub const ARM_S: f64 = 3.0;
    /// Line segments in the countdown ring of an armed button.
    pub const RING_STEPS: usize = 32;
    /// The edge of the card of the deck on air.
    pub const ON_AIR_EDGE: f32 = 2.0;
    /// The gap between a cart edge and its key label.
    pub const KEY_INSET: f32 = 3.0;

    /// A row of the main timeline.
    pub const ROW: f32 = 26.0;
    /// A tool window resizes to its content only when the content size moves more than this.
    pub const FIT_SLACK: f32 = 2.0;
    /// A meter bar rises at once and falls this fast, in dB per second: 30 dB in 150 ms.
    pub const METER_RELEASE_DB_S: f32 = 200.0;
    /// A peak mark stays this long, in seconds, then falls.
    pub const METER_HOLD_S: f32 = 1.5;
    /// A peak mark falls this fast after its hold, in dB per second.
    pub const METER_HOLD_FALL_DB_S: f32 = 40.0;
    /// Meters that stayed flat this long, in seconds, stop asking for frames.
    pub const METER_IDLE_S: f32 = 1.0;
    /// A thin divider row for an item from another playlist.
    pub const DIVIDER_ROW: f32 = 20.0;
    /// The fit line under a fixed row in Upcoming.
    pub const FIT_LINE: f32 = 16.0;
    /// A row of the Upcoming list. All rows have this height, so the list draws only what shows.
    pub const UPCOMING_ROW: f32 = 34.0;

    /// Table columns.
    pub const COL_TIME: f32 = 128.0;
    /// The slot for the ⏱ of a fixed row, before its time.
    pub const GLYPH_SLOT: f32 = 14.0;
    pub const COL_LEN: f32 = 72.0;
    pub const COL_PLAYLIST: f32 = 78.0;
    /// Above this many cart pages, a picker replaces the row of page tabs.
    pub const PAGE_TABS_MAX: usize = 6;
    /// The widest cart page tab.
    pub const PAGE_TAB_MAX_W: f32 = 140.0;
    /// A playlist tab shows at most this many characters of its name.
    pub const TAB_MAX_CHARS: usize = 22;

    /// The part of the central area that the scheduled pane takes.
    pub const SPLIT_FRACTION: f32 = 0.45;
    pub const SPLIT_MIN: f32 = 320.0;

    /// Deck cards.
    pub const DECK_MIN_W: f32 = 180.0;
    /// The narrowest deck card. Below the normal width its text truncates.
    pub const DECK_TINY_W: f32 = 184.0;
    /// The widest text of a dialog. Help text wraps at this width.
    pub const DIALOG_W: f32 = 420.0;
    /// The deck badge on a row of a list.
    pub const DECK_MARK: f32 = 16.0;
    /// The bar in the deck colour at the left edge of a row on a deck.
    pub const DECK_BAR: f32 = 3.0;
    /// The least room in which a deck card shows its state word.
    pub const DECK_WORD_MIN_W: f32 = 40.0;
    pub const PROGRESS_H: f32 = 6.0;
    pub const BADGE: f32 = 28.0;
    pub const PLAYLIST_TABLE_H: f32 = 360.0;
    /// The status cluster beside the decks, by width. Full: every label and the date. Mid: only
    /// Play next keeps its word. Narrow: one column, icons only, no date.
    pub const CLOCK_W: f32 = 672.0;
    pub const CLOCK_MID_W: f32 = 456.0;
    pub const CLOCK_NARROW_W: f32 = 200.0;
    /// A deck strip at least this wide gives the cluster its mid width, and at the full
    /// width it gives the full one.
    pub const STRIP_WIDE: f32 = 1200.0;
    pub const STRIP_FULL: f32 = 1700.0;
    /// The right column of the cluster: the clock, the date and the Out meter.
    pub const CLOCK_COL_W: f32 = 140.0;
    /// The Out meter in the header: the height of the meter in a narrow cluster.
    pub const VU_NARROW_H: f32 = 20.0;
    /// The two channel bars of the Out meter in the cluster, together.
    pub const VU_BARS_H: f32 = 36.0;
    /// The gap between the two channel bars of a meter.
    pub const VU_GAP: f32 = 3.0;
    /// The range of a deck fade time, in seconds.
    pub const FADE_MIN_S: f32 = 0.5;
    pub const FADE_MAX_S: f32 = 10.0;
    /// The range of a deck fade-in, in seconds. Zero starts the deck at full level.
    pub const FADE_IN_MIN_S: f32 = 0.0;

    /// The on-air marker at the left edge of a row.
    pub const ON_AIR_BAR: f32 = 3.0;
    /// The radius of a playlist dot.
    pub const DOT_R: f32 = 4.0;
    pub const HAIRLINE: f32 = 1.0;

    /// Cartwall.
    pub const CART_COLS: usize = 4;
    pub const CART_W: f32 = 132.0;
    pub const CART_H: f32 = CONTROL_MD;
    pub const STOP_W: f32 = 168.0;
    /// The time that a pressed cart flashes, in seconds.
    pub const CART_FLASH_S: f64 = 1.0;

    /// Tool windows: cartwall, mixer, clock and history.
    pub const CART_PAGE: usize = 16;
    pub const CARTWALL_W: f32 = 640.0;
    pub const CARTWALL_H: f32 = 300.0;
    pub const MIXER_W: f32 = 760.0;
    pub const MIXER_H: f32 = 440.0;
    pub const CLOCK_WIN_W: f32 = 520.0;
    pub const CLOCK_WIN_H: f32 = 520.0;
    pub const HISTORY_W: f32 = 820.0;
    pub const HISTORY_H: f32 = 540.0;
    /// The smallest size of each tool window.
    pub const CARTWALL_MIN_W: f32 = 560.0;
    pub const CARTWALL_MIN_H: f32 = 260.0;
    pub const MIXER_MIN_W: f32 = 420.0;
    pub const MIXER_MIN_H: f32 = 380.0;
    pub const CLOCK_MIN_W: f32 = 240.0;
    pub const CLOCK_MIN_H: f32 = 240.0;
    pub const HISTORY_MIN_W: f32 = 520.0;
    pub const HISTORY_MIN_H: f32 = 300.0;
    pub const LIBRARY_MIN_W: f32 = 640.0;
    pub const LIBRARY_MIN_H: f32 = 320.0;

    /// Library window.
    pub const LIBRARY_W: f32 = 900.0;
    pub const LIBRARY_H: f32 = 520.0;
    pub const TREE_W: f32 = 180.0;
    pub const DOCK_H: f32 = 260.0;
    pub const TEXT_COL_MIN: f32 = 64.0;
    pub const TEXT_COL_MAX: f32 = 640.0;
    pub const COL_PATH: f32 = 280.0;
    pub const COL_FIT: f32 = 96.0;
    pub const UPCOMING_TIME: f32 = 112.0;
    pub const UPCOMING_PLAYLIST: f32 = 64.0;
    pub const UPCOMING_LEN: f32 = 52.0;
    pub const DECK_STRIP_H: f32 = 200.0;
    pub const COL_TITLE: f32 = 240.0;
    pub const COL_ARTIST: f32 = 150.0;
    pub const COL_CATEGORY: f32 = 90.0;
    pub const COL_ORIGIN: f32 = 70.0;
    /// A text field in a settings section.
    pub const FIELD_W: f32 = 220.0;
    /// A settings page: the tab rail, the label column and the one width of every control.
    /// A narrow window shrinks the controls to the least width, never below it.
    pub const SETTINGS_RAIL: f32 = 96.0;
    pub const SETTINGS_LABEL: f32 = 124.0;
    pub const SETTINGS_FIELD: f32 = FIELD_W;
    pub const SETTINGS_FIELD_MIN: f32 = 132.0;
    /// The height of a settings page with its header. A shorter window scrolls the form.
    pub const SETTINGS_PAGE_H: f32 = 480.0;
    /// The cart colour bar at the left edge of a cart.
    pub const CART_BAR: f32 = 4.0;
    /// The longest list in the rules editor before it scrolls.
    pub const EDITOR_ITEMS_H: f32 = 140.0;
    /// The time that a toast stays, in seconds.
    pub const TOAST_S: f64 = 4.0;

    /// Mixer strips.
    pub const STRIP_W: f32 = 104.0;
    /// The dB numbers beside a mixer meter.
    pub const METER_SCALE_W: f32 = 22.0;
    pub const METER_H: f32 = 220.0;
    pub const METER_BAR_W: f32 = 8.0;
    pub const FADER_W: f32 = 20.0;
    /// The meter scale in dB.
    pub const METER_MIN_DB: f32 = -60.0;
    pub const METER_MAX_DB: f32 = 6.0;
    /// Levels above these show in the amber and red zones.
    pub const METER_AMBER_DB: f32 = -18.0;
    pub const METER_RED_DB: f32 = -6.0;

    /// Clock window: the share of the window that the ring takes, and the digit size per point of ring.
    pub const CLOCK_RING_SHARE: f32 = 0.62;
    pub const CLOCK_DIGITS_PER_RING: f32 = 0.16;
    pub const CLOCK_TICK_R: f32 = 3.0;
    /// Every fifth tick of the ring is bigger, by this factor.
    pub const CLOCK_MAJOR_EVERY: i8 = 5;
    pub const CLOCK_MAJOR_TICK: f32 = 1.5;
    /// The date text size as a part of the digit size.
    pub const CLOCK_DATE_PER_DIGITS: f32 = 0.3;
    pub const CLOCK_LINE_PER_RING: f32 = 0.08;
    /// In its last seconds, a clock countdown grows by this factor and warns.
    pub const CLOCK_URGENT_S: u64 = 30;
    pub const CLOCK_URGENT_GROW: f32 = 1.25;

    /// The rules editor window.
    pub const EDITOR_W: f32 = 760.0;
    pub const EDITOR_H: f32 = 460.0;
    pub const EDITOR_ROWS_H: f32 = 300.0;
    pub const COMBO_W: f32 = 150.0;
    pub const SCRIPT_ROWS: usize = 14;

    /// The window.
    pub const WINDOW_W: f32 = 1440.0;
    pub const WINDOW_H: f32 = 860.0;
    pub const WINDOW_MIN_W: f32 = 1024.0;
    pub const WINDOW_MIN_H: f32 = 640.0;
}

/// One set of colours for the dark console theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// The page behind everything.
    pub canvas: Color32,
    /// A card or panel on the page.
    pub card: Color32,
    /// Anything to press. It is a shade lighter than the card.
    pub field: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    /// The soft edge at rest.
    pub card_border: Color32,
    /// The 3:1 edge under the pointer.
    pub field_border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    /// The app colour for selection, focus and the playing deck.
    pub accent: Color32,
    pub accent_ink: Color32,
    /// Text and icons on a solid accent fill.
    pub on_accent: Color32,
    pub selection: Color32,
    /// The colour of the row and the deck that are on air.
    pub on_air: Color32,
    pub on_air_bg: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub danger_fill: Color32,
    /// The fill of a warning callout and of a PFL that is on.
    pub warn_fill: Color32,
    /// A cart while it flashes.
    pub flash: Color32,
    pub flash_ink: Color32,
    /// Meter zones: normal, loud and too loud.
    pub meter_low: Color32,
    pub meter_mid: Color32,
    pub meter_high: Color32,
    pub radius_sm: CornerRadius,
    pub radius: CornerRadius,
    pub radius_lg: CornerRadius,
}

impl Tokens {
    pub const DARK: Self = Self {
        canvas: Color32::from_rgb(0x0E, 0x10, 0x14),
        card: Color32::from_rgb(0x16, 0x19, 0x1F),
        field: Color32::from_rgb(0x20, 0x24, 0x2C),
        hover: Color32::from_rgb(0x2A, 0x2F, 0x39),
        pressed: Color32::from_rgb(0x34, 0x3A, 0x46),
        card_border: Color32::from_rgb(0x2A, 0x2F, 0x38),
        field_border: Color32::from_rgb(0x6A, 0x73, 0x82),
        text: Color32::from_rgb(0xE8, 0xEB, 0xF0),
        text_muted: Color32::from_rgb(0x9C, 0xA5, 0xB4),
        accent: Color32::from_rgb(0x3D, 0xB8, 0xF5),
        accent_ink: Color32::from_rgb(0x8F, 0xD6, 0xFA),
        on_accent: Color32::from_rgb(0x05, 0x16, 0x22),
        selection: Color32::from_rgb(0x17, 0x3A, 0x52),
        on_air: Color32::from_rgb(0xFF, 0x5A, 0x5F),
        on_air_bg: Color32::from_rgb(0x3A, 0x18, 0x1C),
        ok: Color32::from_rgb(0x4C, 0xC3, 0x8A),
        warn: Color32::from_rgb(0xF5, 0xB9, 0x42),
        danger: Color32::from_rgb(0xFF, 0x7A, 0x7A),
        danger_fill: Color32::from_rgb(0x8E, 0x22, 0x2A),
        warn_fill: Color32::from_rgb(0x4A, 0x36, 0x0E),
        flash: Color32::from_rgb(0xF5, 0xB9, 0x42),
        flash_ink: Color32::from_rgb(0x1A, 0x14, 0x05),
        meter_low: Color32::from_rgb(0x4C, 0xC3, 0x8A),
        meter_mid: Color32::from_rgb(0xF5, 0xB9, 0x42),
        meter_high: Color32::from_rgb(0xFF, 0x5A, 0x5F),
        radius_sm: CornerRadius::same(4),
        radius: CornerRadius::same(6),
        radius_lg: CornerRadius::same(8),
    };

    pub fn get(_ctx: &egui::Context) -> Self {
        Self::DARK
    }

    /// A playlist colour from the view-model as a `Color32`.
    pub fn playlist(rgb: [u8; 3]) -> Color32 {
        Color32::from_rgb(rgb[0], rgb[1], rgb[2])
    }

    /// The identity colour of deck `i` (0 is A).
    pub fn deck(i: usize) -> Color32 {
        let rgb = DECK_COLOURS[i.min(3)];
        Color32::from_rgb(rgb[0], rgb[1], rgb[2])
    }

    /// The letter ink on the colour of deck `i`: the canvas or the text, whichever reads.
    pub fn deck_ink(&self, i: usize) -> Color32 {
        let bg = Self::deck(i);
        if contrast(self.canvas, bg) >= contrast(self.text, bg) {
            self.canvas
        } else {
            self.text
        }
    }

    /// The playlist colour, moved toward `text` until it reads at 4.5:1 on `bg`.
    pub fn readable(&self, rgb: [u8; 3], bg: Color32) -> Color32 {
        let base = Self::playlist(rgb);
        (0u8..=10)
            .map(|step| lerp(base, self.text, f32::from(step) / 10.0))
            .find(|c| contrast(*c, bg) >= MIN_TEXT_CONTRAST)
            .unwrap_or(self.text)
    }
}

/// Colours that a person can give a playlist or a cart, with their names. The name always
/// shows too. Red, amber, green and the accent blue mean on air, danger, warning, ok and
/// selection, so the palette keeps away from them.
/// The identity colour of each deck, A to D. They are far from each other, from the
/// playlist palette and from the colours that mean something. The deck letter always comes
/// with the colour.
pub const DECK_COLOURS: [[u8; 3]; 4] = [
    [0x22, 0x44, 0xFF],
    [0xFF, 0x00, 0xFF],
    [0x77, 0xFF, 0x33],
    [0xFF, 0xFF, 0xDD],
];

pub const PALETTE: [(&str, [u8; 3]); 8] = [
    ("Lime", [0x9C, 0xC0, 0x3A]),
    ("Copper", [0xB8, 0x7A, 0x52]),
    ("Magenta", [0xC8, 0x5F, 0xC8]),
    ("Indigo", [0x6A, 0x74, 0xE8]),
    ("Violet", [0x8E, 0x6A, 0xD8]),
    ("Teal", [0x2E, 0x9C, 0xA0]),
    ("Rose", [0xD0, 0x6A, 0xA8]),
    ("Slate", [0x8A, 0x96, 0xA8]),
];

/// The WCAG minimum contrast for text.
pub const MIN_TEXT_CONTRAST: f32 = 4.5;

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| {
        let v = f32::from(x) + (f32::from(y) - f32::from(x)) * t;
        v.round().clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// The WCAG contrast ratio between two opaque colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Proportional text at a size from the scale.
pub fn text(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Monospace text for times, lengths and the clock.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// A card on the canvas.
pub fn card_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
        .corner_radius(t.radius_lg)
        .inner_margin(pad::CARD)
}

/// A panel on the canvas with even padding.
pub fn panel_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::new().fill(t.canvas).inner_margin(pad::PANEL)
}

/// A bar across the window, for the decks, the cartwall and the status line.
pub fn bar_frame(t: &Tokens, margin: Margin) -> egui::Frame {
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(size::HAIRLINE, t.card_border))
        .inner_margin(margin)
}

/// Set the egui style from the tokens.
pub fn apply(ctx: &egui::Context) {
    crate::fonts::install(ctx);
    let t = Tokens::get(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Heading, text(SECTION)),
            (TextStyle::Body, text(BODY)),
            (TextStyle::Button, text(BODY)),
            (TextStyle::Small, text(CAPTION)),
            (TextStyle::Monospace, mono(CODE)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(space::S, space::XS);
        s.button_padding = egui::vec2(space::M, space::XS);
        s.interact_size.y = size::CONTROL_SM;
        // Scroll bars always show, so a person sees that more content waits.
        s.scroll = egui::style::ScrollStyle::solid();
        // Rows react to clicks and right-clicks. Selectable text would take those clicks.
        style.interaction.selectable_labels = false;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(t.text);
        v.panel_fill = t.canvas;
        v.window_fill = t.card;
        v.window_stroke = Stroke::new(size::HAIRLINE, t.card_border);
        v.extreme_bg_color = t.canvas;
        v.faint_bg_color = t.card;
        v.hyperlink_color = t.accent_ink;
        v.warn_fg_color = t.warn;
        v.error_fg_color = t.danger;
        v.selection.bg_fill = t.selection;
        v.selection.stroke = Stroke::new(size::HAIRLINE, t.accent_ink);
        v.window_corner_radius = t.radius_lg;
        v.menu_corner_radius = t.radius;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = t.card;
        w.noninteractive.weak_bg_fill = t.card;
        w.noninteractive.bg_stroke = Stroke::new(size::HAIRLINE, t.card_border);
        w.noninteractive.fg_stroke = Stroke::new(size::HAIRLINE, t.text);
        for (wv, fill, edge) in [
            (&mut w.inactive, t.field, t.card_border),
            (&mut w.hovered, t.hover, t.field_border),
            (&mut w.active, t.pressed, t.accent),
            (&mut w.open, t.pressed, t.accent),
        ] {
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.bg_stroke = Stroke::new(size::HAIRLINE, edge);
            wv.fg_stroke = Stroke::new(size::HAIRLINE, t.text);
            wv.corner_radius = t.radius;
            wv.expansion = 0.0;
        }
    });
}

/// Map Base16 Mocha Dark syntax classes to the station palette.
pub(crate) fn script_palette(job: &mut egui::text::LayoutJob, t: &Tokens) {
    for section in &mut job.sections {
        let c = section.format.color;
        let spread = c.r().max(c.g()).max(c.b()) - c.r().min(c.g()).min(c.b());
        let ink = if spread < 30 {
            t.text_muted
        } else if c.g() > c.r() && c.g() > c.b() {
            t.ok
        } else if c.r() > c.g() && c.r() > c.b() {
            t.warn
        } else {
            t.accent_ink
        };
        section.format.color = if contrast(ink, t.field) >= 4.5 {
            ink
        } else {
            t.text
        };
        section.format.font_id = mono(CODE);
        section.format.underline.color = section.format.color;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CIE L*a*b* of an sRGB colour, for the colour distance.
    fn lab(c: Color32) -> [f32; 3] {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.950_47;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.088_83;
        let f = |t: f32| {
            if t > 0.008_856 {
                t.cbrt()
            } else {
                7.787 * t + 16.0 / 116.0
            }
        };
        [
            116.0 * f(y) - 16.0,
            500.0 * (f(x) - f(y)),
            200.0 * (f(y) - f(z)),
        ]
    }

    /// The CIE76 colour difference.
    fn delta_e(a: Color32, b: Color32) -> f32 {
        let (a, b) = (lab(a), lab(b));
        a.iter()
            .zip(b)
            .map(|(p, q)| (p - q).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    #[test]
    fn palette_colours_stay_clear_of_the_colours_that_mean_something() {
        // A ΔE of 25 reads as a different colour at a glance.
        const MIN_DELTA_E: f32 = 25.0;
        let t = Tokens::DARK;
        let semantic = [t.on_air, t.danger, t.warn, t.ok, t.accent, t.selection];
        for (name, rgb) in PALETTE {
            for s in semantic {
                let d = delta_e(Tokens::playlist(rgb), s);
                assert!(d >= MIN_DELTA_E, "{name} is {d:.1} from {s:?}");
            }
        }
    }

    #[test]
    fn deck_colours_stay_clear_of_each_other_the_palette_and_the_meanings() {
        const MIN_DELTA_E: f32 = 20.0;
        let t = Tokens::DARK;
        let mut others: Vec<Color32> =
            vec![t.on_air, t.danger, t.warn, t.ok, t.accent, t.selection];
        others.extend(PALETTE.iter().map(|(_, rgb)| Tokens::playlist(*rgb)));
        for i in 0..4 {
            let d = Tokens::deck(i);
            for o in &others {
                let e = delta_e(d, *o);
                assert!(e >= MIN_DELTA_E, "deck {i} is {e:.1} from {o:?}");
            }
            for j in i + 1..4 {
                let e = delta_e(d, Tokens::deck(j));
                assert!(e >= MIN_DELTA_E, "decks {i} and {j} are {e:.1} apart");
            }
            assert!(
                contrast(t.deck_ink(i), d) >= MIN_TEXT_CONTRAST,
                "the letter on deck {i} reads"
            );
        }
    }

    #[test]
    fn text_and_playlist_names_read_at_4_5_to_1_on_every_surface() {
        let t = Tokens::DARK;
        let surfaces = [t.canvas, t.card, t.field, t.hover, t.selection, t.on_air_bg];
        for bg in surfaces {
            for ink in [t.text, t.text_muted] {
                assert!(contrast(ink, bg) >= MIN_TEXT_CONTRAST, "{ink:?} on {bg:?}");
            }
            // Playlist colours are data. Any colour must come out readable.
            for rgb in [[0xD0, 0x48, 0x48], [0x8E, 0x6A, 0xD8], [0x10, 0x10, 0x10]] {
                let ink = t.readable(rgb, bg);
                assert!(contrast(ink, bg) >= MIN_TEXT_CONTRAST, "{rgb:?} on {bg:?}");
            }
        }
        assert!(contrast(t.flash_ink, t.flash) >= MIN_TEXT_CONTRAST);
        assert!(contrast(t.text, t.danger_fill) >= MIN_TEXT_CONTRAST);
        assert!(contrast(t.text, t.warn_fill) >= MIN_TEXT_CONTRAST);
        assert!(contrast(t.warn, t.warn_fill) >= MIN_TEXT_CONTRAST);
    }
}
