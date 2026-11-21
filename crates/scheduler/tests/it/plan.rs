use jiff::civil::date;
use jiff::{SignedDuration, Timestamp};
use radio_model::{FixMode, Item, ItemId, Placement, Repeat, Rule, Station, Trigger};
use radio_scheduler::{Entry, Origin, PlanInput, PlanOptions, PlayHistory, Timeline, plan};
use radio_testkit::{jingle, playlist, station, track};
use std::time::Duration;

fn now() -> Timestamp {
    "2026-11-21T13:00:00Z".parse().unwrap()
}

fn tracks(n: u64) -> Vec<Item> {
    (1..=n)
        .map(|i| track(i, &format!("track-{i}"), &format!("artist-{i}"), 180))
        .collect()
}

/// Main playlist plus one hard event at 13:`minute`:00 UTC and a pool of 20 s fillers.
fn make_station(main: Vec<Item>, minute: i8) -> Station {
    let mut m = playlist(1, "main", main);
    m.is_main = true;
    let mut ev = playlist(2, "event", vec![jingle(100, 60)]);
    ev.rules = vec![Rule {
        trigger: Trigger::At {
            at: date(2026, 11, 21).at(13, minute, 0, 0),
            repeat: Repeat::Once,
        },
        placement: Placement::Fixed {
            mode: FixMode::Hard,
            tolerance: Duration::from_secs(1),
        },
        take: 1,
        constraints: Vec::new(),
    }];
    let mut pool = playlist(9, "fill", (0..12).map(|i| jingle(200 + i, 20)).collect());
    pool.is_fill_pool = true;
    station("UTC", vec![m, ev, pool])
}

fn run(st: &Station, pinned: &[Entry], prev: Option<&Timeline>) -> Timeline {
    run_for(st, pinned, prev, Duration::from_secs(3600))
}

fn run_for(st: &Station, pinned: &[Entry], prev: Option<&Timeline>, horizon: Duration) -> Timeline {
    let history = PlayHistory::new();
    plan(
        &PlanInput {
            station: st,
            history: &history,
            now: now(),
            horizon,
            pinned,
        },
        &PlanOptions::default(),
        prev,
    )
}

/// Item ids of the entries before the fixed event.
fn window(tl: &Timeline) -> Vec<ItemId> {
    tl.entries
        .iter()
        .take_while(|e| e.fixed.is_none())
        .map(|e| e.item)
        .collect()
}

#[test]
fn pinned_entries_keep_position() {
    let st = make_station(tracks(30), 30);
    let item = track(77, "manual", "artist-77", 120);
    let start = now() + Duration::from_secs(600);
    let manual = Entry {
        playlist: st.playlists[0].id,
        item: item.id,
        start,
        len: Duration::from_secs(120),
        origin: Origin::Manual,
        locked: true,
        fixed: None,
        trim: Duration::ZERO,
    };
    let tl = run(&st, std::slice::from_ref(&manual), None);
    let got = tl.entries.iter().find(|e| e.item == item.id).unwrap();
    assert_eq!(got.start, start);
}

#[test]
fn small_improvement_does_not_reshuffle() {
    let st = make_station(tracks(30), 30);
    let fresh = run(&st, &[], None);
    // A previous plan with the first two tracks swapped and an error only 0.3 s worse.
    let mut prev = fresh.clone();
    let (a, b) = (prev.entries[0].item, prev.entries[1].item);
    prev.entries[0].item = b;
    prev.entries[1].item = a;
    let err = prev.fits[0].error;
    let worse = SignedDuration::from_millis(300);
    prev.fits[0].error = if err.is_negative() {
        err - worse
    } else {
        err + worse
    };
    let again = run(&st, &[], Some(&prev));
    assert_eq!(window(&again), window(&prev));
    assert_ne!(window(&again), window(&fresh));
}

#[test]
fn locked_window_only_fills_and_trims() {
    let st = make_station(tracks(30), 9);
    let prev = run(&st, &[], None);
    let before = window(&prev);
    // Drop the second track from the library.
    let gone = before[1];
    let mut st2 = st.clone();
    st2.playlists[0].items.retain(|i| i.id != gone);
    let tl = run(&st2, &[], Some(&prev));
    let main: Vec<ItemId> = tl
        .entries
        .iter()
        .take_while(|e| e.fixed.is_none())
        .filter(|e| e.origin == Origin::Main)
        .map(|e| e.item)
        .collect();
    let kept: Vec<ItemId> = before.iter().copied().filter(|i| *i != gone).collect();
    assert!(
        main.iter().all(|i| kept.contains(i)),
        "{main:?} not within {kept:?}"
    );
    assert!(
        main.windows(2).all(|w| {
            kept.iter().position(|i| *i == w[0]) < kept.iter().position(|i| *i == w[1])
        })
    );
    assert!(
        tl.entries
            .iter()
            .take_while(|e| e.fixed.is_none())
            .any(|e| e.origin == Origin::Fill)
    );
}

#[test]
fn same_input_same_output() {
    let st = make_station(tracks(30), 30);
    let a = run(&st, &[], None);
    assert_eq!(a, run(&st, &[], None));
    let b = run(&st, &[], Some(&a));
    assert_eq!(b, run(&st, &[], Some(&a)));
}

#[test]
fn kept_window_does_not_change_how_often_items_air() {
    let st = make_station(tracks(30), 30);
    let day = Duration::from_secs(24 * 3600);
    let fresh = run_for(&st, &[], None, day);
    // An older plan had track 12 first; the new merge airs it later in the window's cycle.
    let mut prev = run(&st, &[], None);
    prev.entries[0].item = ItemId(12);
    let err = prev.fits[0].error;
    let worse = SignedDuration::from_millis(300);
    prev.fits[0].error = if err.is_negative() {
        err - worse
    } else {
        err + worse
    };
    let kept = run_for(&st, &[], Some(&prev), day);
    assert_eq!(kept.entries[0].item, ItemId(12));
    let airings = |tl: &Timeline| {
        let mut n = std::collections::BTreeMap::new();
        for e in tl.entries.iter().filter(|e| e.origin == Origin::Main) {
            *n.entry(e.item).or_insert(0_i64) += 1;
        }
        n
    };
    let (a, b) = (airings(&fresh), airings(&kept));
    assert!(
        a.values().all(|n| *n >= 2),
        "the playlist should cycle twice"
    );
    for (item, n) in &a {
        let m = b.get(item).copied().unwrap_or(0);
        assert!((n - m).abs() <= 1, "{item:?} aired {n} times, now {m}");
    }
    let (ta, tb): (i64, i64) = (a.values().sum(), b.values().sum());
    assert!((ta - tb).abs() <= 2, "{ta} airings became {tb}");
}

#[test]
fn frozen_window_never_pulls_entries_across() {
    let mut st = make_station(tracks(30), 5);
    // A second, hard event follows the soft one.
    st.playlists[1].items.push(jingle(101, 30));
    st.playlists[1].rules[0].placement = Placement::Fixed {
        mode: FixMode::Soft,
        tolerance: Duration::from_secs(1),
    };
    let mut second = st.playlists[1].rules[0].clone();
    second.trigger = Trigger::At {
        at: date(2026, 11, 21).at(13, 15, 0, 0),
        repeat: Repeat::Once,
    };
    second.placement = Placement::Fixed {
        mode: FixMode::Hard,
        tolerance: Duration::from_secs(1),
    };
    let mut other = playlist(3, "event-2", vec![jingle(102, 30)]);
    other.rules = vec![second];
    st.playlists.push(other);
    let prev = run(&st, &[], None);
    let before = window(&prev);
    let mut st2 = st.clone();
    st2.playlists[0].items.retain(|i| i.id != before[0]);
    let tl = run(&st2, &[], Some(&prev));
    let first_event = tl.entries.iter().position(|e| e.fixed.is_some()).unwrap();
    for e in &tl.entries[..first_event] {
        assert!(
            e.origin == Origin::Fill || before.contains(&e.item),
            "{e:?} came from beyond the event"
        );
    }
    let len = |e: &Entry| {
        let item = st2
            .playlists
            .iter()
            .flat_map(|p| &p.items)
            .find(|i| i.id == e.item)
            .unwrap();
        radio_timing::item_len(item)
    };
    for w in tl.entries.windows(2) {
        assert!(
            w[0].start + len(&w[0]) - w[0].trim <= w[1].start + Duration::from_secs(2),
            "{:?} overlaps {:?}",
            w[0],
            w[1]
        );
    }
}

#[test]
fn normal_event_before_frozen_window_leaves_it_untouched() {
    let mut st = make_station(tracks(30), 3);
    let mut second = st.playlists[1].rules[0].clone();
    second.trigger = Trigger::At {
        at: date(2026, 11, 21).at(13, 7, 0, 0),
        repeat: Repeat::Once,
    };
    let mut other = playlist(3, "event-2", vec![jingle(102, 30)]);
    other.rules = vec![second];
    st.playlists.push(other);
    let prev = run(&st, &[], None);
    let fixed: Vec<usize> = (0..prev.entries.len())
        .filter(|&i| prev.entries[i].fixed.is_some())
        .collect();
    let first_window = ids_of(&prev.entries[..fixed[0]]);
    let second_window = ids_of(&prev.entries[fixed[0] + 1..fixed[1]]);
    // Only the second window changes, so only it is frozen.
    let mut st2 = st.clone();
    st2.playlists[0].items.retain(|i| i.id != second_window[0]);
    let tl = run(&st2, &[], Some(&prev));
    let fixed: Vec<usize> = (0..tl.entries.len())
        .filter(|&i| tl.entries[i].fixed.is_some())
        .collect();
    assert_eq!(ids_of(&tl.entries[..fixed[0]]), first_window);
    for e in &tl.entries[fixed[0] + 1..fixed[1]] {
        assert!(
            e.origin == Origin::Fill || second_window.contains(&e.item),
            "{e:?} was pulled into the frozen window"
        );
    }
    assert!(
        tl.entries
            .iter()
            .all(|e| e.fixed.as_ref().is_none_or(|f| !f.frozen))
    );
}

fn ids_of(entries: &[Entry]) -> Vec<ItemId> {
    entries.iter().map(|e| e.item).collect()
}

#[test]
fn frozen_windows_keep_order_with_close_events() {
    // The first event (60 s) ends at 13:06; the second window is only 90 s long.
    let mut st = make_station(tracks(30), 5);
    let mut second = st.playlists[1].rules[0].clone();
    second.trigger = Trigger::At {
        at: date(2026, 11, 21).at(13, 7, 30, 0),
        repeat: Repeat::Once,
    };
    let mut other = playlist(3, "event-2", vec![jingle(102, 30)]);
    other.rules = vec![second];
    st.playlists.push(other);
    let prev = run(&st, &[], None);
    let fixed = |tl: &Timeline| -> Vec<usize> {
        (0..tl.entries.len())
            .filter(|&i| tl.entries[i].fixed.is_some())
            .collect()
    };
    let main = |tl: &Timeline, r: std::ops::Range<usize>| -> Vec<ItemId> {
        tl.entries[r]
            .iter()
            .filter(|e| e.origin == Origin::Main)
            .map(|e| e.item)
            .collect()
    };
    let pf = fixed(&prev);
    let first = ids_of(&prev.entries[..pf[0]]);
    let second_w = main(&prev, pf[0] + 1..pf[1]);
    assert!(!second_w.is_empty(), "the second window airs a track");
    // The first track leaves the library: both windows differ and are kept.
    let mut st2 = st.clone();
    st2.playlists[0].items.retain(|i| i.id != first[0]);
    let fresh = run(&st2, &[], None);
    let ff = fixed(&fresh);
    assert_ne!(
        main(&fresh, ff[0] + 1..ff[1]),
        second_w,
        "a fresh plan orders the second window differently"
    );
    let tl = run(&st2, &[], Some(&prev));
    let f = fixed(&tl);
    let subsequence = |got: &[ItemId], of: &[ItemId]| {
        let mut rest = of.iter();
        got.iter().all(|g| rest.any(|o| o == g))
    };
    let got = main(&tl, 0..f[0]);
    assert!(subsequence(&got, &first), "{got:?}");
    assert_eq!(main(&tl, f[0] + 1..f[1]), second_w);
}

#[test]
fn dedupe_never_touches_a_frozen_window_containing_a_locked_entry() {
    // A near (frozen) event at 13:09 with a locked entry inside its window, then a later
    // event at 13:30.
    let mut st = make_station(tracks(30), 9);
    let mut second = st.playlists[1].rules[0].clone();
    second.trigger = Trigger::At {
        at: date(2026, 11, 21).at(13, 30, 0, 0),
        repeat: Repeat::Once,
    };
    let mut other = playlist(3, "event-2", vec![jingle(102, 30)]);
    other.rules = vec![second];
    st.playlists.push(other);
    let manual = Entry {
        playlist: st.playlists[0].id,
        item: ItemId(77),
        start: now() + Duration::from_secs(180),
        len: Duration::from_secs(60),
        origin: Origin::Manual,
        locked: true,
        fixed: None,
        trim: Duration::ZERO,
    };
    let pinned = std::slice::from_ref(&manual);
    let fresh = run(&st, pinned, None);
    let lock = fresh.entries.iter().position(|e| e.locked).unwrap();
    let ev = fresh
        .entries
        .iter()
        .position(|e| e.fixed.is_some())
        .unwrap();
    assert!(lock > 0 && lock + 2 < ev, "{:?}", fresh.entries);
    let first = fresh.entries[0].item;
    // The previous plan differs after the locked entry (so the first window is kept) and
    // aired the very first track early in the second window (so it is pulled in there).
    let mut prev = fresh.clone();
    prev.entries.swap(lock + 1, lock + 2);
    (prev.entries[lock + 1].start, prev.entries[lock + 2].start) =
        (fresh.entries[lock + 1].start, fresh.entries[lock + 2].start);
    prev.entries[ev + 1].item = first;
    let tl = run(&st, pinned, Some(&prev));
    let got = tl.entries.iter().position(|e| e.fixed.is_some()).unwrap();
    assert_eq!(
        ids_of(&tl.entries[..got]),
        ids_of(&prev.entries[..ev]),
        "the frozen window changed"
    );
}

#[test]
fn resume_rotation_continues_after_pinned_entries() {
    let mut main = playlist(1, "main", tracks(3));
    main.is_main = true;
    let st = station("UTC", vec![main]);
    let first = plan(
        &PlanInput {
            station: &st,
            history: &PlayHistory::new(),
            now: now(),
            horizon: Duration::from_secs(600),
            pinned: &[],
        },
        &PlanOptions::default(),
        None,
    );
    // Items 1 and 2 are on air and on a deck; the playout re-plans a minute later.
    let pinned: Vec<Entry> = first.entries[..2].to_vec();
    let later = now() + Duration::from_secs(60);
    let input = PlanInput {
        station: &st,
        history: &PlayHistory::new(),
        now: later,
        horizon: Duration::from_secs(600),
        pinned: &pinned,
    };
    let next_after_pinned = |tl: &Timeline| tl.entries.get(2).map(|e| e.item);
    let opts = PlanOptions {
        resume_rotation: true,
        ..PlanOptions::default()
    };
    assert_eq!(
        next_after_pinned(&plan(&input, &opts, None)),
        Some(ItemId(3))
    );
    // Off by default: the order starts again from the first item.
    assert_eq!(
        next_after_pinned(&plan(&input, &PlanOptions::default(), None)),
        Some(ItemId(1))
    );
}

/// A frozen window keeps its order after an edit, and the item on air (pinned, started
/// before `now`) stays at its head as given: it is not dropped or moved after the event.
#[test]
fn frozen_window_keeps_the_pinned_item_on_air() {
    let st = make_station(tracks(30), 9);
    let mut prev = run(&st, &[], None);
    // The first item started 10 s ago, in the previous plan too.
    prev.entries[0].start = now() - Duration::from_secs(10);
    let on_air = prev.entries[0].clone();
    // An edit: the third track of the window leaves the library.
    let gone = window(&prev)[2];
    let mut st2 = st.clone();
    st2.playlists[0].items.retain(|i| i.id != gone);
    let tl = run(&st2, std::slice::from_ref(&on_air), Some(&prev));
    let event = tl.entries.iter().position(|e| e.fixed.is_some()).unwrap();
    assert_eq!(
        tl.entries[0], on_air,
        "the item on air leads the window as given"
    );
    assert!(
        tl.entries[event..].iter().all(|e| e.item != on_air.item),
        "the item on air was moved after the event"
    );
    let kept: Vec<ItemId> = window(&prev)
        .into_iter()
        .filter(|i| *i != gone && *i != on_air.item)
        .collect();
    let main: Vec<ItemId> = tl.entries[1..event]
        .iter()
        .filter(|e| e.origin == Origin::Main)
        .map(|e| e.item)
        .collect();
    assert!(
        main.iter().all(|i| kept.contains(i)),
        "{main:?} not within {kept:?}"
    );
}

#[test]
fn kept_exact_window_reuses_the_complete_fit_result() {
    let mut main = tracks(12);
    for item in &mut main {
        let timing = item.timing.as_mut().unwrap();
        timing.segue = Duration::from_secs(60);
        timing.cue_out = timing.segue;
    }
    let st = make_station(main, 3);
    let fresh = run(&st, &[], None);
    assert!(
        fresh
            .fits
            .iter()
            .all(|f| f.error.is_zero() && f.steps.is_empty())
    );
    let mut prev = fresh.clone();
    // An unavailable entry makes the old window differ, but keeping its surviving
    // entries gives the same exact order. No fit details or warnings may change.
    let mut gone = prev.entries[0].clone();
    gone.item = ItemId(9999);
    prev.entries.insert(0, gone);
    assert_eq!(run(&st, &[], Some(&prev)), fresh);
}
