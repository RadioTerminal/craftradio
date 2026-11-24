//! The number of decks the station uses.

use crate::fixtures::*;
use radio_engine::Command;
use radio_playout::Playout;

fn songs(tag: &str, n: u64, len: f32, segue: f32) -> Vec<radio_model::Item> {
    (1..=n)
        .map(|i| {
            let side = if i % 2 == 1 { Side::Left } else { Side::Right };
            let path = wav(&format!("{tag}{i}"), len, side);
            file_item(i, &format!("Song {i}"), &path, len, segue, 0.0)
        })
        .collect()
}

fn on_decks(p: &Playout) -> Vec<Option<u64>> {
    p.snapshot()
        .decks
        .iter()
        .map(|d| d.item.map(|i| i.0))
        .collect()
}

fn titles(p: &Playout) -> Vec<String> {
    let rows = p.history(t0(), 20).unwrap();
    rows.into_iter().map(|r| r.entry.title).collect()
}

/// With two decks the plan plays on A and B, with each segue on time. C and D never take
/// an item.
#[test]
fn two_decks_carry_the_plan_and_the_others_stay_empty() {
    let mut engine = engine_with(songs("two", 5, 2.0, 1.5));
    set_decks(&mut engine, 2);
    let (p, clock) = offline(engine, |_| {});
    let mut program = Vec::new();
    for _ in 0..50 {
        program.extend(run(&p, &clock, 0.1));
        let decks = on_decks(&p);
        assert_eq!(decks[2..], [None, None], "a deck past the two took an item");
    }
    let start1 = first_audible(&program, 0, 0).expect("song 1 sounds");
    let start2 = first_audible(&program, 1, 0).expect("song 2 sounds");
    let segue = start1 + (1.5 * SR as f32) as usize;
    assert!(
        start2.abs_diff(segue) <= SR / 200,
        "song 2 at {start2}, segue at {segue}"
    );
    let played = titles(&p);
    assert!(
        played.starts_with(&["Song 1".into(), "Song 2".into(), "Song 3".into()]),
        "the plan goes on in order: {played:?}"
    );
}

/// A lower number of decks never cuts an item. The items that wait on C and D still play
/// in their turn, and then those decks stay empty.
#[test]
fn a_lower_number_lets_the_other_decks_play_out() {
    let (p, clock) = offline(engine_with(songs("low", 6, 2.0, 2.0)), |_| {});
    run(&p, &clock, 0.5);
    let before = on_decks(&p);
    assert!(before[2].is_some() && before[3].is_some(), "{before:?}");
    p.dispatch(Command::SettingsSet {
        part: radio_model::DECKS_PART.into(),
        key: radio_model::DECK_COUNT_KEY.into(),
        value: 2.into(),
    })
    .unwrap();
    run(&p, &clock, 0.5);
    assert_eq!(on_decks(&p), before, "the change took an item off a deck");
    // Songs 3 and 4 end 8 s in. After that only A and B take items.
    run(&p, &clock, 8.5);
    let played = titles(&p);
    let want: Vec<String> = (1..=5).map(|i| format!("Song {i}")).collect();
    assert!(played.starts_with(&want), "every song played: {played:?}");
    let after = on_decks(&p);
    assert_eq!(after[2..], [None, None], "C and D took a new item");
    assert!(after[..2].iter().any(Option::is_some), "the plan goes on");
}

/// A hard event still airs on time with two decks. It waits on a deck past the two, so A
/// and B stay with the plan, and Program sounds as it does with four decks.
#[test]
fn a_hard_event_airs_on_time_with_two_decks() {
    let render = |decks: u64| {
        let tag = format!("hard{decks}");
        let main: Vec<_> = (1..=4)
            .map(|i| {
                let path = wav(&format!("{tag}-song{i}"), 4.0, Side::Left);
                file_item(i, &format!("Song {i}"), &path, 4.0, 3.5, 0.5)
            })
            .collect();
        let mut engine = engine_with(main);
        let path = wav(&format!("{tag}-news"), 1.0, Side::Right);
        let news = file_item(10, "News", &path, 1.0, 1.0, 0.0);
        let script = r#"rule({at: {date: "2026-11-21", time: "13:00:05", repeat: "once"},
                place: {fixed: "hard", tolerance: 1}, take: 1, constraints: []});"#;
        add_playlist(&mut engine, "News", vec![news], script);
        set_decks(&mut engine, decks);
        let (p, clock) = offline(engine, |_| {});
        let mut program = run(&p, &clock, 4.5);
        let waits_on = on_decks(&p).iter().position(|d| *d == Some(10));
        program.extend(run(&p, &clock, 3.5));
        (program, waits_on)
    };
    let (two, waits_on) = render(2);
    let (four, _) = render(4);

    assert!(
        waits_on.is_some_and(|d| d >= 2),
        "the event waits on deck {waits_on:?}"
    );
    let news_at = first_audible(&two, 1, 0).expect("the news airs");
    assert!(
        news_at.abs_diff(5 * SR) <= SR / 100,
        "the news at {news_at}, its time is {}",
        5 * SR
    );
    // The music around the event starts and stops where it does with four decks.
    let music = |program: &[f32]| -> Vec<bool> {
        (0..14)
            .map(|k| peak(program, 0, k * SR / 2 + SR / 10, (k + 1) * SR / 2 - SR / 10) > 0.01)
            .collect()
    };
    assert_eq!(music(&two), music(&four));
}
