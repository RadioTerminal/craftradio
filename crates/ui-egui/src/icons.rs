//! The icons of the console: Lucide, from one font, by name. Screens never write a glyph.
//!
//! The font is the `lucide-icons` crate's (ISC licence, bundled). It is a family of its own
//! (`icons`) and also a fallback of the text families, so an icon can sit inside a string.

use egui::{FontFamily, FontId};

pub(crate) use lucide_icons::Icon;

/// The name of the font data of the icon font.
pub(crate) const FONT_NAME: &str = "lucide";

/// The font family that holds only the icons.
pub(crate) fn family() -> FontFamily {
    FontFamily::Name("icons".into())
}

/// The font of an icon at `size`, a size from the theme. It is the text family, whose fallback
/// is the icon font: egui binds a new family only from the frame after `set_fonts`, and the
/// first frame must not panic.
pub(crate) fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// The glyph of an icon, to put in a string next to text.
pub(crate) fn glyph(icon: Icon) -> char {
    char::from(icon)
}

/// An icon, a thin gap and a word: for a label or a menu item.
pub(crate) fn lead(icon: Icon, word: &str) -> String {
    format!("{}\u{2009}{word}", glyph(icon))
}
