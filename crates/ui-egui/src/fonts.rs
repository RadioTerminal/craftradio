//! Fallback fonts from the operating system, for scripts that the fonts of egui do not cover.
//!
//! The app ships no extra font. It reads one font per script group from the system, so
//! Japanese, Chinese, Arabic and other scripts show as letters, not as boxes.

use std::sync::{Arc, OnceLock};

/// Font files to try, per script group, in order. The first file that exists wins.
const GROUPS: [&[(&str, u32)]; 2] = [
    // Chinese, Japanese and Korean.
    &[
        ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
        (
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            0,
        ),
        ("C:\\Windows\\Fonts\\YuGothR.ttc", 0),
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),
    ],
    // Arabic, Hebrew and the other scripts of a wide font.
    &[
        ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
        (
            "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
            0,
        ),
        ("/usr/share/fonts/noto/NotoSansArabic-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
        ("C:\\Windows\\Fonts\\segoeui.ttf", 0),
        ("C:\\Windows\\Fonts\\arial.ttf", 0),
    ],
];

/// The fallback fonts that this system has, read once.
fn system_fallbacks() -> &'static [(String, Arc<egui::FontData>)] {
    static FONTS: OnceLock<Vec<(String, Arc<egui::FontData>)>> = OnceLock::new();
    FONTS.get_or_init(|| {
        GROUPS
            .iter()
            .enumerate()
            .filter_map(|(g, group)| {
                group.iter().find_map(|(path, index)| {
                    let bytes = std::fs::read(path).ok()?;
                    let mut data = egui::FontData::from_owned(bytes);
                    data.index = *index;
                    Some((format!("system-fallback-{g}"), Arc::new(data)))
                })
            })
            .collect()
    })
}

/// Adds the system fallback fonts after the fonts of egui, in both families.
pub(crate) fn install(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    // The Lucide icons: a family of their own, and a fallback of the text families, so an
    // icon inside a text string draws in the same galley.
    let lucide = crate::icons::FONT_NAME.to_owned();
    defs.font_data.insert(
        lucide.clone(),
        Arc::new(egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES)),
    );
    defs.families
        .insert(crate::icons::family(), vec![lucide.clone()]);
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        defs.families
            .entry(family)
            .or_default()
            .push(lucide.clone());
    }
    for (name, data) in system_fallbacks() {
        defs.font_data.insert(name.clone(), Arc::clone(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families.entry(family).or_default().push(name.clone());
        }
    }
    ctx.set_fonts(defs);
}
