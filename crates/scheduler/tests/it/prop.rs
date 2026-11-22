//! Invariants over random stations: order and overlap, locked entries, tolerance, no panics.

use jiff::Timestamp;
use jiff::civil::date;
use proptest::prelude::*;
use radio_model::{FixMode, Item, ItemId, Placement, Repeat, Rule, Station, Trigger};
use radio_scheduler::{Entry, Origin, PlanInput, PlanOptions, PlayHistory, Warning, plan};
use radio_testkit::{jingle, playlist, station, track};
use radio_timing::advance;
use std::collections::BTreeMap;
use std::time::Duration;

fn now() -> Timestamp {
    "2026-11-21T13:00:00Z".parse().unwrap()
}

#[derive(Debug, Clone)]
struct Event {
    minute: u32,
    hard: bool,
    priority: i32,
    secs: u32,
}

#[derive(Debug, Clone)]
struct Case {
    tracks: Vec<u32>,
    fill: Vec<u32>,
    events: Vec<Event>,
    /// Pin a main track as a locked entry this many seconds after now.
    locked_at: Option<u32>,
}

fn case() -> impl Strategy<Value = Case> {
    let event = (5u32..240, any::<bool>(), 0i32..4, 10u32..300).prop_map(
        |(minute, hard, priority, secs)| Event {
            minute,
            hard,
            priority,
            secs,
        },
    );
    (
        prop::collection::vec(60u32..=400, 1..=30),
        prop::collection::vec(2u32..=15, 3..=10),
        prop::collection::vec(event, 0..=3),
        prop::option::of(0u32..3 * 3600),
    )
        .prop_map(|(tracks, fill, events, locked_at)| Case {
            tracks,
            fill,
            events,
            locked_at,
        })
}

fn build(c: &Case) -> Station {
    let items = c
        .tracks
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let id = i as u64 + 1;
            track(id, &format!("track-{id}"), &format!("artist-{id}"), s)
        })
        .collect();
    let mut main = playlist(1, "main", items);
    main.is_main = true;
    let mut pls = vec![main];
    for (k, ev) in c.events.iter().enumerate() {
        let k = k as u64;
        let mut pl = playlist(
            10 + k,
            &format!("event-{k}"),
            vec![jingle(2000 + k, ev.secs)],
        );
        pl.priority = ev.priority;
        let at = date(2026, 11, 21).at(13, 0, 0, 0) + jiff::Span::new().minutes(ev.minute);
        pl.rules = vec![Rule {
            trigger: Trigger::At {
                at,
                repeat: Repeat::Once,
            },
            placement: Placement::Fixed {
                mode: if ev.hard {
                    FixMode::Hard
                } else {
                    FixMode::Soft
                },
                tolerance: Duration::from_secs(1),
            },
            take: 1,
            constraints: Vec::new(),
        }];
        pls.push(pl);
    }
    let fill = c
        .fill
        .iter()
        .enumerate()
        .map(|(i, &s)| jingle(1000 + i as u64, s))
        .collect();
    let mut pool = playlist(9, "fill", fill);
    pool.is_fill_pool = true;
    pls.push(pool);
    station("UTC", pls)
}

fn items(st: &Station) -> BTreeMap<ItemId, &Item> {
    st.playlists
        .iter()
        .flat_map(|p| &p.items)
        .map(|i| (i.id, i))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn plan_invariants(c in case()) {
        let st = build(&c);
        let pinned: Vec<Entry> = c
            .locked_at
            .map(|s| {
                let item = &st.playlists[0].items[0];
                Entry {
                    playlist: st.playlists[0].id,
                    item: item.id,
                    start: now() + Duration::from_secs(s.into()),
                    len: radio_timing::item_len(item),
                    origin: Origin::Manual,
                    locked: true,
                    fixed: None,
                    trim: Duration::ZERO,
                }
            })
            .into_iter()
            .collect();
        let tl = plan(
            &PlanInput {
                station: &st,
                history: &PlayHistory::new(),
                now: now(),
                horizon: Duration::from_secs(4 * 3600),
                pinned: &pinned,
            },
            &PlanOptions::default(),
            None,
        );
        let by_id = items(&st);

        // 1. Sorted by start; each entry starts no earlier than the one before hands over.
        for w in tl.entries.windows(2) {
            prop_assert!(w[0].start <= w[1].start, "out of order: {:?}", w);
            let prev = by_id[&w[0].item];
            let hard_next = w[1].fixed.as_ref().is_some_and(|f| f.mode == FixMode::Hard);
            let next = (!hard_next).then(|| by_id[&w[1].item]);
            let reach = w[0].start + advance(prev, next) - w[0].trim;
            prop_assert!(reach <= w[1].start, "overlap: {:?}", w);
        }

        // 2. A locked entry keeps its start unless the planner says it moved it.
        for p in &pinned {
            let moved = tl
                .warnings
                .iter()
                .any(|w| matches!(w, Warning::LockedMoved { item, .. } if *item == p.item));
            let kept = tl.entries.iter().any(|e| e.locked && e.item == p.item && e.start == p.start);
            prop_assert!(kept || moved, "locked {:?} changed silently", p);
        }

        // 3. Within tolerance means the error is within the tolerance.
        for f in &tl.fits {
            if f.within_tolerance {
                prop_assert!(f.error.unsigned_abs() <= Duration::from_secs(1), "{:?}", f);
            }
        }
    }
}
