use std::time::Duration;

use radio_audio::{
    AudioError, AudioEvent, Clip, Command, CommandKind, DeckId, DeckState, LoadOptions, Mixer,
    MixerConfig, Strip, render_offline,
};

use crate::fixtures::{frames, peak_db, rms_db, tone_samples, tone_wav, wait_ready};

const SR: usize = 48_000;

fn play(at_sample: u64, deck: DeckId) -> Command {
    Command::new(at_sample, CommandKind::Play(deck))
}

/// A deck with a −6 dBFS tone reaches Program, and the Out peak meter shows about −6 dBFS.
/// `Mixer::render` does not allocate.
#[test]
fn a_playing_deck_reaches_program_and_the_out_meter_without_allocating() {
    let path = tone_wav("meter", 48_000, 3.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::A, &path, LoadOptions::default())
        .expect("load");
    wait_ready(&handle, DeckId::A);
    handle.send(play(0, DeckId::A)).expect("send");

    let mut program = vec![0.0f32; 480 * 2];
    let mut pfl = vec![0.0f32; 480 * 2];
    let mut loudest = f32::NEG_INFINITY;
    assert_no_alloc::reset_violation_count();
    for _ in 0..100 {
        assert_no_alloc::assert_no_alloc(|| mixer.render(&mut program, &mut pfl));
        loudest = loudest.max(peak_db(&program));
    }
    assert_eq!(
        assert_no_alloc::violation_count(),
        0,
        "Mixer::render allocated"
    );

    assert!(loudest > -7.0, "program is too quiet: {loudest} dBFS");
    let out = handle.meter_reader().read().strip(Strip::Out).peak_db;
    for ch in out {
        assert!((ch - -6.0).abs() < 0.5, "Out peak meter reads {ch} dBFS");
    }
}

/// The mixer applies `Play` at sample N exactly: N−1 is silent and N has audio.
#[test]
fn play_starts_on_its_sample() {
    let path = tone_wav("play-at", 48_000, 1.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::B, &path, LoadOptions::default())
        .expect("load");
    wait_ready(&handle, DeckId::B);
    let n = 1_234;
    handle.send(play(n as u64, DeckId::B)).expect("send");

    let out = render_offline(&mut mixer, 4_096);
    assert!(
        frames(&out, 0, n).iter().all(|s| *s == 0.0),
        "audio before sample N"
    );
    assert!(out[(n - 1) * 2] == 0.0);
    assert!(out[n * 2].abs() > 0.1, "sample N is silent");
}

/// A FadeOut is below −60 dBFS at its end, and the deck sends `DeckEnded`.
#[test]
fn fade_out_is_silent_by_its_end() {
    let path = tone_wav("fade", 48_000, 3.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::C, &path, LoadOptions::default())
        .expect("load");
    wait_ready(&handle, DeckId::C);
    let start = SR / 2;
    let over = Duration::from_millis(500);
    handle.send(play(0, DeckId::C)).expect("send");
    handle
        .send(Command::new(
            start as u64,
            CommandKind::FadeOut {
                deck: DeckId::C,
                over,
            },
        ))
        .expect("send");

    let out = render_offline(&mut mixer, 2 * SR);
    let end = start + SR / 2;
    assert!(
        peak_db(frames(&out, start - SR / 10, start)) > -7.0,
        "loud before the fade"
    );
    assert!(
        peak_db(frames(&out, end, 2 * SR)) < -60.0,
        "still audible after the fade"
    );
    assert!(
        handle
            .events()
            .any(|e| e == AudioEvent::DeckEnded(DeckId::C))
    );
}

/// A quiet cart with Duck −12 lowers Program by 10 dB or more.
#[test]
fn ducking_under_a_cart_lowers_program() {
    let path = tone_wav("duck", 48_000, 3.0, 500.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::A, &path, LoadOptions::default())
        .expect("load");
    handle
        .set_cart(
            0,
            Some(Clip::from_interleaved(tone_samples(2.0, 800.0, -50.0))),
        )
        .expect("cart");
    wait_ready(&handle, DeckId::A);
    let fire = SR as u64;
    handle.send(play(0, DeckId::A)).expect("send");
    handle
        .send(Command::new(fire, CommandKind::FireCart { slot: 0 }))
        .unwrap();
    handle
        .send(Command::new(
            fire,
            CommandKind::Duck {
                db: -12.0,
                attack: Duration::from_millis(50),
                release: Duration::from_millis(200),
            },
        ))
        .unwrap();

    let out = render_offline(&mut mixer, 2 * SR);
    let before = rms_db(frames(&out, SR / 2, SR));
    let after = rms_db(frames(&out, SR + SR / 5, 2 * SR));
    assert!(
        before - after >= 10.0,
        "ducked by only {} dB",
        before - after
    );
}

/// The detector sends `Silence` when Program silence reaches its threshold, not before.
#[test]
fn silence_detector_fires_after_its_threshold() {
    let config = MixerConfig::default();
    let after = config.silence.after;
    let (mut mixer, mut handle, _live) = Mixer::new(config);

    let almost = (after.as_secs_f64() * SR as f64) as usize - SR / 10;
    let _ = render_offline(&mut mixer, almost);
    assert!(
        !handle
            .events()
            .any(|e| matches!(e, AudioEvent::Silence { .. })),
        "fired early",
    );

    let _ = render_offline(&mut mixer, SR / 5);
    let fired: Vec<_> = handle.events().collect();
    assert!(
        fired.contains(&AudioEvent::Silence { since_sample: 0 }),
        "events: {fired:?}"
    );
}

/// The handle refuses a NaN or +inf gain. Program stays finite, also with a very high gain.
#[test]
fn a_gain_that_is_not_a_number_is_rejected_and_program_stays_finite() {
    let path = tone_wav("nan-gain", 48_000, 1.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::A, &path, LoadOptions::default())
        .expect("load");
    wait_ready(&handle, DeckId::A);
    handle.send(play(0, DeckId::A)).expect("send");

    for db in [f32::NAN, f32::INFINITY] {
        let gain = CommandKind::SetGain {
            strip: Strip::Deck(DeckId::A),
            db,
        };
        let sent = handle.send(Command::new(0, gain));
        assert!(
            matches!(sent, Err(AudioError::InvalidCommand(_))),
            "{db} dB: {sent:?}"
        );
    }
    let loud = CommandKind::SetGain {
        strip: Strip::Deck(DeckId::A),
        db: 1.0e9,
    };
    handle.send(Command::new(0, loud)).expect("a high gain");

    let out = render_offline(&mut mixer, SR / 2);
    assert!(out.iter().all(|s| s.is_finite()), "Program is not finite");
    assert!(peak_db(&out) <= -0.99, "the limiter let a peak through");
    assert!(peak_db(&out) > -7.0, "the deck is not audible");
}

/// The mixer refuses a load onto a deck that is on air, and the deck keeps its item.
#[test]
fn a_load_onto_a_playing_deck_is_rejected() {
    let first = tone_wav("on-air", 48_000, 3.0, 1_000.0, -6.0);
    let second = tone_wav("next", 48_000, 3.0, 300.0, -20.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::D, &first, LoadOptions::default())
        .expect("load");
    wait_ready(&handle, DeckId::D);
    handle.send(play(0, DeckId::D)).expect("send");
    let _ = render_offline(&mut mixer, SR / 10);
    assert_eq!(handle.deck_state(DeckId::D), DeckState::Playing);

    handle
        .load(DeckId::D, &second, LoadOptions::default())
        .expect("send the second load");
    assert_eq!(handle.deck_state(DeckId::D), DeckState::Playing);

    let out = render_offline(&mut mixer, SR / 10);
    let events: Vec<_> = handle.events().collect();
    assert!(
        events.iter().any(|e| matches!(
            e,
            AudioEvent::LoadRejected {
                deck: DeckId::D,
                ..
            }
        )),
        "events: {events:?}"
    );
    assert_eq!(handle.deck_state(DeckId::D), DeckState::Playing);
    assert!(peak_db(&out) > -7.0, "the item on air stopped");
}

/// A ring-fed deck plays what another thread pushes, and reports `Started` on the sample
/// of `Play`. Offline render does not wait for its empty ring.
#[test]
fn ring_fed_deck_starts_on_its_sample_and_underruns_without_stalling() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let (mut tx, rx) = rtrb::RingBuffer::new(SR);
    for _ in 0..SR / 10 {
        let _ = tx.push(0.5);
        let _ = tx.push(0.5);
    }
    let options = LoadOptions {
        item_tag: Some(7),
        ..LoadOptions::default()
    };
    handle.load_ring(DeckId::D, rx, options).expect("load");
    let n = 2_000u64;
    handle.send(play(n, DeckId::D).for_item(7)).expect("send");
    let begun = std::time::Instant::now();
    let out = render_offline(&mut mixer, SR / 2);
    assert!(begun.elapsed() < Duration::from_secs(1), "render waited");
    assert!(out[(n as usize - 1) * 2] == 0.0 && out[n as usize * 2] > 0.1);
    let events: Vec<AudioEvent> = handle.events().collect();
    assert!(events.contains(&AudioEvent::Started {
        deck: DeckId::D,
        at_sample: n,
        item_tag: Some(7)
    }));
    assert!(events.contains(&AudioEvent::Underrun(DeckId::D)));
}
