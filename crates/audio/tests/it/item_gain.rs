//! The item gain of a deck: the loudness gain that playout sets for each item.

use std::time::Duration;

use radio_audio::{Command, CommandKind, DeckId, LoadOptions, Mixer, MixerConfig, render_offline};

use crate::fixtures::{frames, rms_db, tone_wav, wait_ready};

const SR: usize = 48_000;
/// The level of the constant test signal, under the soft-clip knee.
const DC: f32 = 0.25;

/// A deck that a ring feeds with a constant level, so each sample shows the gain.
fn dc_deck(gain_db: f32, tag: u64) -> (Mixer, radio_audio::MixerHandle) {
    let (mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let (mut tx, rx) = rtrb::RingBuffer::new(4 * SR * 2);
    for _ in 0..(4 * SR * 2 - 2) {
        let _ = tx.push(DC);
    }
    drop(tx);
    let options = LoadOptions {
        item_tag: Some(tag),
        gain_db,
        ..LoadOptions::default()
    };
    handle.load_ring(DeckId::A, rx, options).expect("load");
    handle
        .send(Command::new(0, CommandKind::Play(DeckId::A)).for_item(tag))
        .expect("play");
    (mixer, handle)
}

/// The gain of the load applies from the first sample: a cut item is quieter by that much.
#[test]
fn a_load_with_a_gain_plays_at_that_gain_from_its_first_sample() {
    let path = tone_wav("item-gain", 48_000, 1.0, 1_000.0, -12.0);
    let level = |gain_db: f32| {
        let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
        let options = LoadOptions {
            gain_db,
            ..LoadOptions::default()
        };
        handle.load(DeckId::B, &path, options).expect("load");
        wait_ready(&handle, DeckId::B);
        handle
            .send(Command::new(0, CommandKind::Play(DeckId::B)))
            .expect("play");
        let out = render_offline(&mut mixer, SR / 2);
        (rms_db(frames(&out, 0, SR / 100)), rms_db(&out))
    };
    let (unity_start, unity) = level(0.0);
    let (cut_start, cut) = level(-9.0);
    assert!((unity - cut - 9.0).abs() < 0.2, "{unity} vs {cut}");
    assert!(
        (unity_start - cut_start - 9.0).abs() < 0.2,
        "no fade at the start"
    );
}

/// A gain change on air ramps over its time: no sample moves more than one ramp step,
/// and the level reaches the new gain at the end of the ramp.
#[test]
fn a_gain_change_on_air_ramps_without_a_jump() {
    let (mut mixer, mut handle) = dc_deck(0.0, 3);
    let before = render_offline(&mut mixer, SR / 10);
    let over = Duration::from_secs(1);
    let at = mixer.now();
    handle
        .send(
            Command::new(
                at,
                CommandKind::SetItemGain {
                    deck: DeckId::A,
                    db: -12.0,
                    over,
                },
            )
            .for_item(3),
        )
        .expect("set gain");
    let out = render_offline(&mut mixer, 3 * SR / 2);
    let target = DC * 10f32.powf(-12.0 / 20.0);
    let step = (DC - target) / SR as f32;
    let mut last = before[before.len() - 2];
    let mut biggest = 0.0f32;
    for f in out.as_chunks::<2>().0 {
        biggest = biggest.max((f[0] - last).abs());
        last = f[0];
    }
    assert!(biggest <= step * 1.01, "jump {biggest} > step {step}");
    let half = out[SR];
    assert!(
        half < DC && half > target,
        "still on the ramp at 0.5 s: {half}"
    );
    let end = out[out.len() - 2];
    assert!((end - target).abs() < 1e-4, "ends at {end}, not {target}");
}

/// A gain for another item on the deck does nothing.
#[test]
fn a_gain_for_another_item_does_nothing() {
    let (mut mixer, mut handle) = dc_deck(0.0, 3);
    let at = mixer.now();
    handle
        .send(
            Command::new(
                at,
                CommandKind::SetItemGain {
                    deck: DeckId::A,
                    db: -12.0,
                    over: Duration::ZERO,
                },
            )
            .for_item(99),
        )
        .expect("set gain");
    let out = render_offline(&mut mixer, SR / 10);
    assert!((out[out.len() - 2] - DC).abs() < 1e-6);
}

/// A gain change on air does not allocate in `render`.
#[test]
fn a_gain_ramp_does_not_allocate() {
    let (mut mixer, mut handle) = dc_deck(-3.0, 5);
    let mut program = vec![0.0f32; 480 * 2];
    mixer.render(&mut program, &mut []);
    handle
        .send(
            Command::new(
                mixer.now(),
                CommandKind::SetItemGain {
                    deck: DeckId::A,
                    db: 4.0,
                    over: Duration::from_millis(500),
                },
            )
            .for_item(5),
        )
        .expect("set gain");
    assert_no_alloc::reset_violation_count();
    for _ in 0..100 {
        assert_no_alloc::assert_no_alloc(|| mixer.render(&mut program, &mut []));
    }
    assert_eq!(assert_no_alloc::violation_count(), 0, "render allocated");
}

/// The handle refuses a gain that is not a number.
#[test]
fn a_nan_item_gain_is_refused() {
    let (_mixer, mut handle) = dc_deck(0.0, 1);
    let bad = Command::new(
        0,
        CommandKind::SetItemGain {
            deck: DeckId::A,
            db: f32::NAN,
            over: Duration::ZERO,
        },
    );
    assert!(handle.send(bad).is_err());
}
