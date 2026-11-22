//! The font stack: craft-ui's `UiFonts`, the order every Craft app draws text in.
//!
//! egui's own faces come first (Ubuntu for text, Hack for numbers, code and symbols), then any
//! font installed on the system that has a character they lack, found when a title needs it, so a
//! Thai, Hindi, Hebrew, Khmer or Japanese title draws as letters. egui's emoji faces come after.
//! The app ships no extra text font. The first such title waits once for craft-ui's index of the
//! installed fonts (under half a second in a release build; the index is reused after that).
//!
//! The Lucide icons are a family of their own (`icons`) and a fallback of the text families, so
//! an icon can sit inside a string.

use craft_ui::fonts::{I18nScripts, UiFonts};
use egui::{FontData, FontFamily};

/// The text families of the UI.
const TEXT: [FontFamily; 2] = [FontFamily::Proportional, FontFamily::Monospace];

/// Installs the font stack in `ctx`.
pub(crate) fn install(ctx: &egui::Context) {
    let mut fonts = UiFonts::new(&I18nScripts);
    // A lookup that waits for the index of installed fonts the first time a character needs
    // one, so a title never draws as boxes while the index is built.
    if let Some(system) = craft_ui::fonts::system_font_provider("en") {
        fonts = fonts.system_provider(system);
    }
    let mut stack = fonts
        // Hack has the arrows and symbols Ubuntu lacks (→, ±, ▸).
        .egui_face("Hack", &[FontFamily::Proportional])
        // egui's emoji faces, ahead of the system fonts' own emoji.
        .egui_face("NotoEmoji-Regular", &TEXT)
        .egui_face("emoji-icon-font", &TEXT)
        .stack();
    let lucide = crate::icons::FONT_NAME;
    let data = FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES);
    stack.insert(&crate::icons::family(), 0, lucide, data.clone());
    for family in &TEXT {
        let end = stack.chain(family).len();
        stack.insert(family, end, lucide, data.clone());
    }
    stack.install(ctx);
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;

    /// Titles in scripts that egui's faces lack draw as letters from the fonts installed on the
    /// machine: the test harness fails on any character no face can draw. It needs a system
    /// with fonts for these scripts, as macOS and most desktop Linux installs have.
    #[test]
    fn titles_in_other_scripts_draw_from_the_system_fonts() {
        const TITLES: [&str; 5] = [
            "เพลงกลางคืน", // Thai
            "रात का गीत", // Hindi (Devanagari)
            "שיר הלילה",  // Hebrew
            "ចម្រៀងពេលយប់", // Khmer
            "የሌሊት ዘፈን",   // Amharic (Ethiopic)
        ];
        let mut harness = Harness::builder()
            .with_size(egui::vec2(400.0, 300.0))
            .build_ui(|ui| {
                // The stack binds from the frame after it is installed.
                if ui
                    .ctx()
                    .fonts(|f| !f.families().contains(&crate::icons::family()))
                {
                    super::install(ui.ctx());
                    return;
                }
                for title in TITLES {
                    ui.label(title);
                }
            });
        harness.run();
    }
}
