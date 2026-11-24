use std::collections::BTreeSet;

use jiff::Timestamp;
use radio_model::{ItemId, PickOrder};
use radio_scheduler::{Picker, PlayHistory, Played};
use radio_testkit::{playlist, track};

fn pool() -> radio_model::Playlist {
    playlist(
        1,
        "Test pool",
        (1..=3)
            .map(|id| track(id, "Test tone", "Test artist", 30))
            .collect(),
    )
}

#[test]
fn in_order_cycles_and_rejects_candidates() {
    let mut pl = pool();
    let hist = PlayHistory::new();
    let mut picker = Picker::new(&pl);
    let ids: Vec<_> = (0..4)
        .map(|_| picker.next(&pl, &hist, |_| true).unwrap().id)
        .collect();
    assert_eq!(ids, [ItemId(1), ItemId(2), ItemId(3), ItemId(1)]);
    for order in [
        PickOrder::InOrder,
        PickOrder::Shuffle { seed: 42 },
        PickOrder::LeastRecentlyPlayed,
        PickOrder::Weighted {
            weights: vec![(ItemId(1), 1)],
        },
    ] {
        pl.pick = order;
        let mut picker = Picker::new(&pl);
        assert!(picker.next(&pl, &hist, |_| false).is_none());
        assert!(
            picker
                .next(&pl, &hist, |item| item.id == ItemId(1))
                .is_some()
        );
    }
    pl.items.clear();
    assert!(Picker::new(&pl).next(&pl, &hist, |_| true).is_none());
}

#[test]
fn shuffle_same_seed_same_order_and_plays_all_before_repeat() {
    let mut pl = pool();
    pl.pick = PickOrder::Shuffle { seed: 42 };
    let hist = PlayHistory::new();
    let mut first = Picker::new(&pl);
    let mut second = Picker::new(&pl);
    let sequence: Vec<_> = (0..10)
        .map(|_| first.next(&pl, &hist, |_| true).unwrap().id)
        .collect();
    let repeat: Vec<_> = (0..10)
        .map(|_| second.next(&pl, &hist, |_| true).unwrap().id)
        .collect();
    assert_eq!(sequence, repeat);
    for cycle in sequence.chunks_exact(pl.items.len()) {
        assert_eq!(cycle.iter().collect::<BTreeSet<_>>().len(), pl.items.len());
    }
}

#[test]
fn least_recently_played_picks_oldest_and_weighted_excludes_zero() {
    let mut pl = pool();
    pl.pick = PickOrder::LeastRecentlyPlayed;
    let mut hist = PlayHistory::new();
    let early: Timestamp = "2026-11-21T10:00:00Z".parse().unwrap();
    let late: Timestamp = "2026-11-21T11:00:00Z".parse().unwrap();
    for (item, at) in [(ItemId(1), late), (ItemId(1), early), (ItemId(2), early)] {
        hist.push(Played {
            item,
            playlist: pl.id,
            at,
            artist: None,
            title: "Test tone".into(),
        });
    }
    assert_eq!(hist.last_played(ItemId(1)), Some(late));
    assert_eq!(
        hist.iter_since(early)
            .map(|entry| entry.at)
            .collect::<Vec<_>>(),
        [early, early, late]
    );
    assert_eq!(hist.count_since(|entry| entry.item == ItemId(1), late), 1);
    let mut picker = Picker::new(&pl);
    assert_eq!(picker.next(&pl, &hist, |_| true).unwrap().id, ItemId(3));
    assert_eq!(
        picker
            .next(&pl, &hist, |item| item.id != ItemId(3))
            .unwrap()
            .id,
        ItemId(2)
    );

    pl.pick = PickOrder::Weighted {
        weights: vec![(ItemId(1), 0), (ItemId(2), 1), (ItemId(3), 3)],
    };
    let mut picker = Picker::new(&pl);
    let sequence: Vec<_> = (0..40)
        .map(|_| picker.next(&pl, &hist, |_| true).unwrap().id)
        .collect();
    assert!(!sequence.contains(&ItemId(1)));
    let light = sequence.iter().filter(|id| **id == ItemId(2)).count();
    let heavy = sequence.iter().filter(|id| **id == ItemId(3)).count();
    assert_eq!(heavy, 3 * light);
    assert!(
        sequence
            .as_chunks::<4>()
            .0
            .iter()
            .all(|chunk| chunk.contains(&ItemId(2)))
    );
}

#[test]
fn equal_daypart_boundaries_allow_the_whole_day() {
    let time = jiff::civil::time(9, 0, 0, 0);
    let constraint = radio_model::Constraint::Daypart {
        from: time,
        to: time,
    };
    let item = radio_testkit::track(1, "Test track", "Test artist", 60);
    for hour in [0, 8, 9, 15, 23] {
        let at: jiff::Timestamp = format!("2026-11-21T{hour:02}:00:00Z").parse().unwrap();
        assert!(radio_scheduler::allowed(
            &constraint,
            &item,
            radio_model::PlaylistId(1),
            at,
            None,
            &PlayHistory::new(),
            &jiff::tz::TimeZone::UTC
        ));
    }
}

/// Playout makes a new plan, and so a new picker, for every pick. Each picker continues
/// from the plays so far.
fn picks_over_new_plans(pl: &radio_model::Playlist, count: usize) -> Vec<ItemId> {
    let hist = PlayHistory::new();
    let mut played = Vec::new();
    for _ in 0..count {
        let mut picker = Picker::new(pl);
        picker.resume(pl, &played);
        played.push(picker.next(pl, &hist, |_| true).unwrap().id);
    }
    played
}

/// A shuffled playlist plays every item before any repeats, also when every pick comes
/// from a new plan.
#[test]
fn shuffle_plays_all_before_repeat_across_plans() {
    let mut pl = playlist(
        1,
        "Test pool",
        (1..=5)
            .map(|id| track(id, "Test tone", "Test artist", 30))
            .collect(),
    );
    pl.pick = PickOrder::Shuffle { seed: 42 };
    let played = picks_over_new_plans(&pl, 20);
    for cycle in played.chunks_exact(pl.items.len()) {
        assert_eq!(
            cycle.iter().collect::<BTreeSet<_>>().len(),
            pl.items.len(),
            "a cycle repeats an item: {played:?}"
        );
    }
}

/// A weighted playlist keeps its shares when every pick comes from a new plan: the light
/// item still plays, and the heavy one plays more.
#[test]
fn weighted_keeps_its_shares_across_plans() {
    let mut pl = pool();
    pl.pick = PickOrder::Weighted {
        weights: vec![(ItemId(1), 0), (ItemId(2), 1), (ItemId(3), 3)],
    };
    let played = picks_over_new_plans(&pl, 40);
    let light = played.iter().filter(|id| **id == ItemId(2)).count();
    let heavy = played.iter().filter(|id| **id == ItemId(3)).count();
    assert!(light > 0, "the light item never played");
    assert!(heavy > 2 * light, "heavy {heavy}, light {light}");
    assert!(!played.contains(&ItemId(1)));
}
