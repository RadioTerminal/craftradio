//! The menu bar, as in the Craft apps: one definition, laid out by craft-menu for each
//! platform. On macOS it is the native menu bar (the desktop app installs it); elsewhere, and
//! when AppKit refuses it, it is drawn in the main window. Items run the keymap's commands, with
//! the keymap's chords, so a menu click and its shortcut do the same thing.

use craft_keymap::CommandRef;
use craft_menu::{Item, ItemRole, LayoutOptions, Menu, MenuBar, MenuRole, Node};

use crate::shortcuts::{self, Pressed};
use crate::windows::{Open, ToolWindow};

/// The app's name in the menus the layouts write (About, Hide, Quit).
pub(crate) const APP_NAME: &str = "CraftRadio";

/// Open or close a tool window: `{"window": title}`.
const WINDOW: &str = "window.toggle";

fn window(w: ToolWindow) -> CommandRef {
    CommandRef::with_params(WINDOW, serde_json::json!({ "window": w.title() }))
}

fn item(id: &str, label: &str) -> Node {
    Node::Item(Item::new(id, label))
}

/// The layouts' own labels. craft-menu writes "⋯" for the menu of titles that do not fit, a
/// glyph the app's fonts lack; the ordinary ellipsis stands in.
fn label(english: &'static str) -> String {
    if english == "\u{22ef}" {
        "…".to_owned()
    } else {
        english.to_owned()
    }
}

/// The layout options: the app's name and its labels.
pub(crate) fn options() -> LayoutOptions<'static> {
    LayoutOptions {
        label: &label,
        ..LayoutOptions::new(APP_NAME)
    }
}

/// The menus as the app is now: which tool windows are open.
pub(crate) fn bar(open: &Open) -> MenuBar {
    let edit = Menu::new(
        "Edit",
        vec![
            item(shortcuts::UNDO, "Undo"),
            item(shortcuts::REDO, "Redo"),
            Node::Separator,
            item(shortcuts::CUT, "Cut"),
            item(shortcuts::COPY, "Copy"),
            item(shortcuts::PASTE, "Paste"),
            item(shortcuts::DUPLICATE, "Duplicate"),
            item(shortcuts::DELETE, "Remove"),
            item(shortcuts::SELECT_ALL, "Select All"),
            Node::Separator,
            Node::Item(Item::new(shortcuts::PREFERENCES, "Preferences…").role(ItemRole::Settings)),
        ],
    )
    .role(MenuRole::Edit);
    let mut playout = vec![
        item(shortcuts::PLAY_NEXT, "Play Next"),
        item(shortcuts::FADE, "Fade Out"),
        item(shortcuts::PLAY_PAUSE, "Play or Pause the Focused Deck"),
        item(shortcuts::STOP_CARTS, "Stop All Carts"),
        Node::Separator,
    ];
    for (i, name) in ['A', 'B', 'C', 'D'].iter().enumerate() {
        playout.push(Node::Item(Item::new(
            shortcuts::focus_deck(i),
            format!("Focus Deck {name}"),
        )));
    }
    let windows = ToolWindow::ALL
        .into_iter()
        .filter(|w| *w != ToolWindow::Preferences)
        .map(|w| Node::Item(Item::new(window(w), w.title()).check(open.get(w))))
        .collect();
    let mut bar = MenuBar {
        menus: vec![
            edit,
            Menu::new("Playout", playout),
            Menu::new("Window", windows).role(MenuRole::Window),
        ],
        documents: Vec::new(),
    };
    craft_menu::apply_keymap(&mut bar, &shortcuts::keymap());
    bar
}

/// What a menu command does: the keys it stands for, or the window it opens or closes.
pub(crate) enum Run {
    Keys(Pressed),
    Window(ToolWindow),
    Nothing,
}

/// The effect of `command`, as the keys of the same command would have.
pub(crate) fn run(command: &CommandRef) -> Run {
    let mut p = Pressed::default();
    let param = |name: &str| {
        command
            .params
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    match command.id.as_str() {
        shortcuts::UNDO => p.undo = true,
        shortcuts::REDO => p.redo = true,
        shortcuts::CUT => p.cut = true,
        shortcuts::COPY => p.copy = true,
        shortcuts::PASTE => p.paste = true,
        shortcuts::DUPLICATE => p.duplicate = true,
        shortcuts::DELETE => p.delete = true,
        shortcuts::SELECT_ALL => p.select_all = true,
        shortcuts::PREFERENCES => p.preferences = true,
        shortcuts::PLAY_NEXT => p.play_next = true,
        shortcuts::FADE => p.fade = true,
        shortcuts::PLAY_PAUSE => p.play_pause = true,
        shortcuts::STOP_CARTS => p.stop_carts = true,
        shortcuts::FOCUS_DECK => p.focus_deck = param("deck"),
        shortcuts::FIRE_CART => p.cart = param("cart"),
        WINDOW => {
            let title = command
                .params
                .get("window")
                .and_then(serde_json::Value::as_str);
            return ToolWindow::ALL
                .into_iter()
                .find(|w| Some(w.title()) == title)
                .map_or(Run::Nothing, Run::Window);
        }
        _ => return Run::Nothing,
    }
    Run::Keys(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_item_does_what_its_shortcut_does_and_shows_it() {
        let open = Open::default();
        let bar = bar(&open);
        for item in bar.items() {
            assert!(
                !matches!(run(&item.command), Run::Nothing),
                "{} runs something",
                item.command.id
            );
        }
        let undo = bar.find(&shortcuts::UNDO.into()).expect("Undo");
        assert!(undo.shortcut.is_some(), "Undo shows its chord");
        assert!(matches!(run(&shortcuts::UNDO.into()), Run::Keys(p) if p.undo));
        assert!(matches!(
            run(&window(ToolWindow::Mixer)),
            Run::Window(ToolWindow::Mixer)
        ));
    }
}
