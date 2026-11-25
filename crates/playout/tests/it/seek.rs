//! `deck.seek`: the position of a deck moves, and the plan moves with it.

use crate::fixtures::*;
use radio_engine::Command;
use radio_model::ItemId;
use std::time::Duration;

fn seek(deck: u8, item: u64, to: f32) -> Command {
    Command::DeckSeek {
        deck,
        position: Duration::from_secs_f32(to),
        expect_item: Some(ItemId(item)),
    }
}

/// A seek forward on the deck on air skips that part of the item: the next item starts
/// at the segue, sooner by the time that the seek jumped over, and the item on air never
/// goes silent on the way.
#[test]
fn a_seek_on_air_brings_the_next_item_sooner() {
    let one = file_item(1, "One", &wav("s-one", 6.0, Side::Left), 6.0, 5.5, 0.5);
    let two = file_item(2, "Two", &wav("s-two", 2.0, Side::Right), 2.0, 1.5, 0.5);
    let (p, clock) = offline(engine_with(vec![one, two]), |_| {});
    run(&p, &clock, 1.0);
    let reply = p.send(seek(0, 1, 4.0)).unwrap();
    let after = run(&p, &clock, 4.0);
    reply.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();

    // One had 4.5 s to its segue before the seek, and 1.5 s after it.
    let next = first_audible(&after, 1, 0).expect("item two never started");
    assert!(next < 2 * SR, "item two kept its old start, frame {next}");
    assert!(next > SR, "item two started over item one, at frame {next}");
    let quiet = (0..next).find(|&f| peak(&after, 0, f, f + SR / 100) < 0.01);
    assert_eq!(quiet, None, "item one went silent before the handover");
}

/// A seek to the start of a paused deck keeps it paused, shows the new position, and the
/// item then plays in full: the next item waits for the whole of it.
#[test]
fn cue_to_start_on_a_paused_deck_plays_the_item_from_its_start() {
    let one = file_item(1, "One", &wav("s-cue-one", 4.0, Side::Left), 4.0, 3.5, 0.5);
    let two = file_item(2, "Two", &wav("s-cue-two", 2.0, Side::Right), 2.0, 1.5, 0.5);
    let (p, clock) = offline(engine_with(vec![one, two]), |_| {});
    run(&p, &clock, 2.0);
    let _ = p
        .send(Command::DeckPause {
            deck: 0,
            expect_item: Some(ItemId(1)),
        })
        .unwrap();
    run(&p, &clock, 0.5);
    let before = p.snapshot().decks[0].position;
    let reply = p.send(seek(0, 1, 0.0)).unwrap();
    let paused = run(&p, &clock, 0.5);
    reply.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();

    let deck = p.snapshot().decks[0].clone();
    assert!(deck.position < before, "the position did not move back");
    assert!(!deck.running, "a paused deck shows a running position");
    assert_eq!(first_audible(&paused, 0, 0), None, "a paused deck sounds");

    let _ = p
        .send(Command::DeckPlay {
            deck: 0,
            expect_item: Some(ItemId(1)),
            fade_in: Duration::ZERO,
        })
        .unwrap();
    let after = run(&p, &clock, 5.0);
    let next = first_audible(&after, 1, 0).expect("item two never started");
    assert!(
        next > 3 * SR,
        "item two cut item one short, at frame {next}"
    );
    assert!(next < 4 * SR, "item two started late, at frame {next}");
}

/// A seek on a deck that waits moves where its item starts: the deck shows the position
/// while it waits, and the item ends sooner by that much once it plays.
#[test]
fn a_seek_on_a_waiting_deck_starts_the_item_there() {
    let one = file_item(1, "One", &wav("s-w-one", 2.0, Side::Left), 2.0, 1.5, 0.5);
    let two = file_item(2, "Two", &wav("s-w-two", 4.0, Side::Right), 4.0, 3.5, 0.5);
    let three = file_item(
        3,
        "Three",
        &wav("s-w-three", 2.0, Side::Left),
        2.0,
        1.5,
        0.5,
    );
    let (p, clock) = offline(engine_with(vec![one, two, three]), |_| {});
    run(&p, &clock, 0.5);
    let reply = p.send(seek(1, 2, 2.0)).unwrap();
    let mut program = run(&p, &clock, 0.5);
    reply.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    let deck = p.snapshot().decks[1].clone();
    assert_eq!(deck.position, Duration::from_secs(2));
    assert!(!deck.running);

    program.extend(run(&p, &clock, 5.0));
    // Two starts at the segue of One, as planned. It has 1.5 s to its own segue, not 3.5 s.
    let two_at = first_audible(&program, 1, 0).expect("item two never started");
    // One sounds for 0.5 s more, on the same side as Three.
    let quiet = (two_at + SR / 2..program.len() / 2)
        .find(|&f| program[f * 2].abs() < 0.001 && peak(&program, 0, f, f + SR / 20) < 0.01)
        .expect("item one never ended");
    let gone = first_audible(&program, 0, quiet).expect("item three never started");
    let gap = gone - two_at;
    assert!(gap < 2 * SR, "item two played its first part, {gap} frames");
    assert!(gap > SR, "item three started over item two, {gap} frames");
}

/// A seek for another item than the deck holds changes nothing.
#[test]
fn a_seek_for_another_item_fails() {
    let one = file_item(1, "One", &wav("s-x-one", 3.0, Side::Left), 3.0, 2.5, 0.5);
    let (p, clock) = offline(engine_with(vec![one]), |_| {});
    run(&p, &clock, 0.5);
    let reply = p.send(seek(0, 9, 2.0)).unwrap();
    run(&p, &clock, 0.2);
    let answer = reply.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(answer.is_err(), "the seek was taken: {answer:?}");
}
