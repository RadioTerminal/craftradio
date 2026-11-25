//! The native macOS menu's keys, sent to the window that has the focus.
//!
//! AppKit runs the Edit menu's key equivalents (⌘C, ⌘X, ⌘V, ⌘A, ⌘Z) before any window sees the
//! key, and craft-menu hands them to egui through the main window's input only: eframe runs
//! `raw_input_hook` for the root viewport alone, and a ⌘V's paste comes back to the viewport that
//! asked for it, the root. A text field in a tool window (Preferences, the Library) would then
//! never copy, cut or paste. So while another window has the focus, the main window's input gives
//! up what the menu put in it, and that window takes it in its own next pass.

use egui::{Event, RawInput, ViewportId};

/// Where the events wait for their window.
fn slot(viewport: ViewportId) -> egui::Id {
    egui::Id::unique("menu_keys_for").with(viewport)
}

/// The window that has the focus, when it is not the main window.
fn focused_tool_window(raw: &RawInput) -> Option<ViewportId> {
    raw.viewports
        .iter()
        .find(|(_, info)| info.focused == Some(true))
        .map(|(id, _)| *id)
        .filter(|id| *id != ViewportId::ROOT)
}

/// For each event of `after`, whether it is not in `before`: what the native menu added. The
/// menu also takes events away (a click that closes an open menu), so an event of `after` is
/// matched with the next equal one of `before`, skipping what was taken.
fn added(before: &[Event], after: &[Event]) -> Vec<bool> {
    let mut from = 0;
    after
        .iter()
        .map(|event| {
            let rest = before.get(from..).unwrap_or_default();
            match rest.iter().position(|b| b == event) {
                Some(at) => {
                    from += at + 1;
                    false
                }
                None => true,
            }
        })
        .collect()
}

/// After the native menu's `raw_input` on the main window's input `raw`, whose events were
/// `before`: while a tool window has the focus, takes out of `raw` what the menu added and every
/// copy, cut and paste (with a native menu only the menu makes those), and keeps them for that
/// window. Returns the window they wait for.
#[doc(hidden)]
pub fn route(ctx: &egui::Context, before: &[Event], raw: &mut RawInput) -> Option<ViewportId> {
    let target = focused_tool_window(raw)?;
    let added = added(before, &raw.events);
    let mut moved = Vec::new();
    let mut kept = Vec::with_capacity(raw.events.len());
    for (event, added) in std::mem::take(&mut raw.events).into_iter().zip(added) {
        let clipboard = matches!(event, Event::Copy | Event::Cut | Event::Paste(_));
        if added || clipboard {
            moved.push(event);
        } else {
            kept.push(event);
        }
    }
    raw.events = kept;
    if moved.is_empty() {
        return None;
    }
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Event>>(slot(target))
            .extend(moved);
    });
    Some(target)
}

/// At the start of a tool window's pass: the menu keys kept for it go into its input, before its
/// text fields read it.
#[doc(hidden)]
pub fn deliver(ctx: &egui::Context) {
    let viewport = ctx.viewport_id();
    let Some(events) = ctx.data_mut(|d| d.remove_temp::<Vec<Event>>(slot(viewport))) else {
        return;
    };
    ctx.input_mut(|i| i.events.extend(events));
}
