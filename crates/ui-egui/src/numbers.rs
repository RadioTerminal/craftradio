//! Number fields that write and read numbers the way the user's region does: its digits, its
//! decimal mark and its grouping ("−3,5 dB" in Slovenia, "−3.5 dB" in the US).
//!
//! The desktop app calls [`install_system`] once. Without it, as in tests, fields use English
//! numbers, so a test reads the same on every machine.

use std::sync::{Arc, OnceLock};

use craft_i18n::{Digits, Format, Region};
use craft_ui::localized::{LocaleNumbers, LocalizedDragValue};

/// The key of the installed numbers in the context's data.
fn key() -> egui::Id {
    egui::Id::unique("radio::numbers")
}

/// English numbers, the default when none are installed.
fn english() -> LocaleNumbers {
    static EN: OnceLock<LocaleNumbers> = OnceLock::new();
    EN.get_or_init(|| LocaleNumbers::from_tag("en")).clone()
}

/// Uses `numbers` for every number field of `ctx`.
pub fn install(ctx: &egui::Context, numbers: LocaleNumbers) {
    ctx.data_mut(|d| d.insert_temp(key(), numbers));
}

/// Uses the numbers of the system's region (its regional format settings) for every number
/// field of `ctx`.
pub fn install_system(ctx: &egui::Context) {
    install(
        ctx,
        LocaleNumbers::new(Arc::new(Format::new(&Region::system(), Digits::Locale))),
    );
}

/// The numbers the fields of `ui` use.
fn numbers(ui: &egui::Ui) -> LocaleNumbers {
    ui.data(|d| d.get_temp::<LocaleNumbers>(key()))
        .unwrap_or_else(english)
}

/// `drag` as a field that writes and reads numbers the region's way. Set its range, speed,
/// decimals, prefix and suffix on `drag` as before.
pub(crate) fn field<'a>(ui: &egui::Ui, drag: egui::DragValue<'a>) -> LocalizedDragValue<'a> {
    LocalizedDragValue::new(drag, &numbers(ui))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{By, NodeT, Queryable};

    /// The number field named `name`.
    fn spin(name: &str) -> By<'_> {
        By::new()
            .label(name)
            .role(egui::accesskit::Role::SpinButton)
    }

    /// The text the number field named `name` shows.
    fn shown<S>(harness: &Harness<'_, S>, name: &str) -> String {
        harness
            .get(spin(name))
            .accesskit_node()
            .value()
            .unwrap_or_default()
    }

    /// A dB field in a harness with `tag`'s numbers, named "Gain".
    fn gain_field(tag: &'static str, value: f64) -> Harness<'static, f64> {
        Harness::builder()
            .with_size(egui::vec2(240.0, 80.0))
            .build_ui_state(
                move |ui, value: &mut f64| {
                    install(ui.ctx(), LocaleNumbers::from_tag(tag));
                    let label = ui.label("Gain");
                    let drag = egui::DragValue::new(value)
                        .range(-20.0..=20.0)
                        .max_decimals(1)
                        .suffix(" dB");
                    ui.add(field(ui, drag)).labelled_by(label.id);
                },
                value,
            )
    }

    #[test]
    fn a_decimal_comma_region_writes_and_reads_its_numbers() {
        let mut harness = gain_field("sl", -3.5);
        harness.run();
        assert!(
            shown(&harness, "Gain").ends_with("3,5 dB"),
            "the field writes a decimal comma: {}",
            shown(&harness, "Gain")
        );
        // A click edits the field with its text selected.
        harness.get(spin("Gain")).click();
        harness.run();
        harness.get(spin("Gain")).type_text("2,5");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(*harness.state(), 2.5, "typed 2,5 reads as two and a half");
    }

    #[test]
    fn without_an_installed_region_fields_use_english_numbers() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(240.0, 80.0))
            .build_ui_state(
                |ui, port: &mut u16| {
                    let label = ui.label("Port");
                    let drag = egui::DragValue::new(port).range(1..=u16::MAX);
                    ui.add(field(ui, drag).ungrouped()).labelled_by(label.id);
                    let label = ui.label("Gain");
                    let mut gain = 1234.5_f64;
                    ui.add(field(ui, egui::DragValue::new(&mut gain).max_decimals(1)))
                        .labelled_by(label.id);
                },
                8000,
            );
        harness.run();
        assert_eq!(shown(&harness, "Gain"), "1,234.5", "English marks");
        assert_eq!(
            shown(&harness, "Port"),
            "8000",
            "an ungrouped field writes no separator"
        );
    }
}
