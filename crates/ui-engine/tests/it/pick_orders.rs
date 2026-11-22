//! Every pick order the rules editor offers is one the engine takes, and it comes back as
//! the same order.

use crate::fixtures::*;
use radio_ui_egui::rules_script;
use radio_ui_egui::view::{Edit, Pick, ViewSource};

#[test]
fn every_pick_order_of_the_editor_saves_and_reads_back() {
    let d = dir();
    let song = track(1, "Song", &wav(&d, "song", 30.0), 30.0);
    let (mut source, clock) = source(vec![song]);
    let _ = wait_for(&mut source, &clock, |v| !v.rows.is_empty());
    assert!(source.edit(Edit::NewPlaylist("Picks".into())).is_ok());
    for pick in Pick::ALL {
        let view = wait_for(&mut source, &clock, |v| {
            v.tabs.iter().any(|t| t.name == "Picks")
        });
        let mut tab = view
            .tabs
            .into_iter()
            .find(|t| t.name == "Picks")
            .expect("the new playlist has a tab");
        tab.order = pick;
        rules_script::refresh(&mut tab);
        assert_eq!(
            source.apply_rules(tab),
            Ok(()),
            "the engine takes the pick order {pick:?}"
        );
        let view = wait_for(&mut source, &clock, |v| {
            v.tabs.iter().any(|t| t.name == "Picks" && t.order == pick)
        });
        let back = view
            .tabs
            .iter()
            .find(|t| t.name == "Picks")
            .map(|t| t.order);
        assert_eq!(back, Some(pick), "the pick order reads back");
    }
}
