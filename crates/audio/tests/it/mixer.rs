use std::time::Duration;

use radio_audio::{
    AudioError, AudioEvent, Clip, Command, CommandKind, DeckId, DeckState, Decoder, LoadOptions,
    Mixer, MixerConfig, Strip, render_offline,
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
    let mut monitor = vec![0.0f32; 480 * 2];
    let mut loudest = f32::NEG_INFINITY;
    assert_no_alloc::reset_violation_count();
    for _ in 0..100 {
        assert_no_alloc::assert_no_alloc(|| mixer.render(&mut program, &mut monitor));
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

#[test]
fn snd_is_pre_fader_and_never_added_to_program() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let (mut tx, rx) = rtrb::RingBuffer::new(4096);
    while tx.slots() >= 2 {
        tx.push(0.25).unwrap();
        tx.push(0.25).unwrap();
    }
    handle
        .load_ring(DeckId::A, rx, LoadOptions::default())
        .unwrap();
    handle
        .send(Command::new(
            0,
            CommandKind::SetGain {
                strip: Strip::Deck(DeckId::A),
                db: f32::NEG_INFINITY,
            },
        ))
        .unwrap();
    handle.set_snd(Strip::Deck(DeckId::A), true);
    handle.send(play(0, DeckId::A)).unwrap();
    let (program, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    assert!(
        monitor.iter().skip(512).all(|x| *x > 0.1),
        "SND must receive the pre-fader signal"
    );
    assert!(
        program.iter().skip(512).all(|x| x.abs() < 0.001),
        "SND leaked into Program"
    );
    handle.set_snd(Strip::Deck(DeckId::A), false);
    let (_, monitor) = radio_audio::render_offline_buses(&mut mixer, 128);
    assert!(monitor.iter().all(|x| *x == 0.0));
}

#[test]
fn captured_deck_uses_live_strip() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let (mut tx, rx) = rtrb::RingBuffer::new(4096);
    while tx.slots() >= 2 {
        tx.push(0.25).unwrap();
        tx.push(0.25).unwrap();
    }
    handle
        .load_ring(
            DeckId::A,
            rx,
            LoadOptions {
                live_bus: true,
                ..LoadOptions::default()
            },
        )
        .unwrap();
    handle
        .send(Command::new(
            0,
            CommandKind::SetGain {
                strip: Strip::Deck(DeckId::A),
                db: f32::NEG_INFINITY,
            },
        ))
        .unwrap();
    handle.set_snd(Strip::Live, true);
    handle.send(play(0, DeckId::A)).unwrap();
    let (program, monitor) = radio_audio::render_offline_buses(&mut mixer, 512);
    assert!(program.iter().skip(256).all(|x| *x > 0.1));
    assert!(monitor.iter().all(|x| *x > 0.1));
    handle
        .send(Command::new(
            handle.now(),
            CommandKind::SetGain {
                strip: Strip::Live,
                db: f32::NEG_INFINITY,
            },
        ))
        .unwrap();
    let (program, monitor) = radio_audio::render_offline_buses(&mut mixer, 512);
    assert!(program.iter().skip(512).all(|x| x.abs() < 0.001));
    assert!(monitor.iter().all(|x| *x > 0.1));
}

/// Loads a deck with a constant level on both channels, enough for 2048 frames, and plays it.
fn constant_deck(handle: &mut radio_audio::MixerHandle, deck: DeckId, level: f32) {
    let (mut tx, rx) = rtrb::RingBuffer::new(4096);
    while tx.slots() >= 2 {
        tx.push(level).unwrap();
        tx.push(level).unwrap();
    }
    handle.load_ring(deck, rx, LoadOptions::default()).unwrap();
    handle.send(play(0, deck)).unwrap();
}

#[test]
fn program_snd_puts_what_listeners_hear_on_the_monitor() {
    for dynamics in [None, Some(radio_audio::ProgramDynamics::default())] {
        let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
        mixer.set_dynamics(dynamics).unwrap();
        constant_deck(&mut handle, DeckId::A, 0.25);
        handle.set_snd(Strip::Out, true);
        let (program, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
        assert!(program.iter().skip(1024).all(|x| x.abs() > 0.1));
        assert_eq!(&monitor[1024..], &program[1024..], "dynamics: {dynamics:?}");
    }
}

#[test]
fn program_snd_is_silent_while_program_is_muted() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    constant_deck(&mut handle, DeckId::A, 0.25);
    handle
        .send(Command::new(
            0,
            CommandKind::SetGain {
                strip: Strip::Out,
                db: f32::NEG_INFINITY,
            },
        ))
        .unwrap();
    handle.set_snd(Strip::Out, true);
    let (_, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    assert!(monitor.iter().skip(512).all(|x| x.abs() < 0.001));
}

#[test]
fn monitor_never_exceeds_full_scale_with_many_snds() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    for deck in DeckId::ALL {
        constant_deck(&mut handle, deck, 0.9);
        handle.set_snd(Strip::Deck(deck), true);
    }
    handle.set_snd(Strip::Out, true);
    let (_, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    assert!(monitor.iter().skip(512).any(|x| x.abs() > 0.5));
    assert!(monitor.iter().all(|x| x.abs() <= 1.0));
}

#[test]
fn no_snd_means_a_silent_monitor() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    constant_deck(&mut handle, DeckId::A, 0.25);
    let (program, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    assert!(program.iter().skip(512).any(|x| x.abs() > 0.1));
    assert!(monitor.iter().all(|x| *x == 0.0));
}

/// Listening to air while pre-listening a deck is the common monitor use: the two together
/// pass the soft limit, they are not cut off at full scale.
#[test]
fn program_and_a_source_snd_together_are_soft_limited() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    constant_deck(&mut handle, DeckId::A, 0.5);
    handle.set_snd(Strip::Deck(DeckId::A), true);
    handle.set_snd(Strip::Out, true);
    let (_, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    let loudest = monitor.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    assert!(loudest > 0.6, "both are heard: {loudest}");
    assert!(
        loudest < 0.9,
        "under the −1 dBFS ceiling, not clipped: {loudest}"
    );
}

/// A fade-out on a deck whose feed has stalled still ends the deck: the fade counts time,
/// not frames that arrived.
#[test]
fn fade_out_ends_a_deck_whose_ring_is_empty() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(SR);
    for s in tone_samples(0.1, 1_000.0, -6.0) {
        let _ = tx.push(s);
    }
    handle
        .load_ring(DeckId::D, rx, LoadOptions::default())
        .expect("load");
    handle.send(play(0, DeckId::D)).expect("send");
    // The ring runs dry after 0.1 s; the fade starts after that.
    handle
        .send(Command::new(
            (SR / 4) as u64,
            CommandKind::FadeOut {
                deck: DeckId::D,
                over: Duration::from_millis(500),
            },
        ))
        .expect("send");

    render_offline(&mut mixer, 2 * SR);
    assert!(
        handle
            .events()
            .any(|e| e == AudioEvent::DeckEnded(DeckId::D)),
        "the starved deck never ended"
    );
}

/// A second fade-out on a deck that already fades never raises the level again.
#[test]
fn a_second_fade_out_does_not_raise_the_level() {
    let path = tone_wav("fade-twice", 48_000, 3.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::C, &path, LoadOptions::default())
        .expect("load");
    wait_ready(&handle, DeckId::C);
    handle.send(play(0, DeckId::C)).expect("send");
    let fade = |at: usize| {
        Command::new(
            at as u64,
            CommandKind::FadeOut {
                deck: DeckId::C,
                over: Duration::from_secs(1),
            },
        )
    };
    let second = SR / 2 + SR * 8 / 10;
    handle.send(fade(SR / 2)).expect("send");
    handle.send(fade(second)).expect("send");

    let out = render_offline(&mut mixer, 3 * SR);
    let before = peak_db(frames(&out, second - SR / 20, second));
    let after = peak_db(frames(&out, second, second + SR / 20));
    assert!(
        after <= before + 0.5,
        "the level rose from {before} to {after} dBFS at the second fade"
    );
}

/// A file that stops decoding in the middle ends its deck with `DecodeFailed` before
/// `DeckEnded`, so the end does not look like the end of the file.
#[test]
fn a_decode_failure_in_the_middle_of_a_file_is_reported() {
    let path = tone_wav("decode-fails", 48_000, 2.0, 1_000.0, -6.0);
    let half = std::fs::metadata(&path).expect("fixture").len() / 2;
    let reader = crate::fixtures::FailingFile::new(&path, half);
    let decoder =
        radio_audio::Decoder::from_reader(reader, Some("wav"), Duration::ZERO).expect("open");
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let options = LoadOptions {
        item_tag: Some(3),
        ..LoadOptions::default()
    };
    handle
        .load_decoder(DeckId::A, decoder, options)
        .expect("load");
    wait_ready(&handle, DeckId::A);
    handle.send(play(0, DeckId::A)).expect("send");

    let mut events = Vec::new();
    let begun = std::time::Instant::now();
    while !events.contains(&AudioEvent::DeckEnded(DeckId::A)) {
        assert!(
            begun.elapsed() < Duration::from_secs(10),
            "the deck never ended"
        );
        render_offline(&mut mixer, SR / 10);
        events.extend(handle.events());
        std::thread::sleep(Duration::from_millis(1));
    }
    let failed = events.iter().position(|e| {
        *e == AudioEvent::DecodeFailed {
            deck: DeckId::A,
            item_tag: Some(3),
        }
    });
    let ended = events
        .iter()
        .position(|e| *e == AudioEvent::DeckEnded(DeckId::A));
    assert!(failed.is_some(), "no failure in {events:?}");
    assert!(failed < ended, "the failure came after the end");
}

/// A seek on a deck on air changes to the new position without a gap: Program stays
/// audible through the change, no underrun is reported, and the item ends early by the
/// time that the seek jumped over.
#[test]
fn a_seek_on_air_jumps_without_a_gap_and_the_item_ends_sooner() {
    let path = tone_wav("seek-on-air", 48_000, 3.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let options = LoadOptions {
        item_tag: Some(7),
        ..LoadOptions::default()
    };
    handle.load(DeckId::A, &path, options).expect("load");
    wait_ready(&handle, DeckId::A);
    handle.send(play(0, DeckId::A)).expect("send");
    let _ = render_offline(&mut mixer, SR / 10);

    let decoder = Decoder::open_at(&path, Duration::from_millis(2_500)).expect("open");
    handle
        .seek_decoder(DeckId::A, decoder, Some(7), 42)
        .expect("seek");
    let mut events = Vec::new();
    let mut rendered = SR / 10;
    let mut ended_at = None;
    while ended_at.is_none() && rendered < 3 * SR {
        let out = render_offline(&mut mixer, SR / 100);
        rendered += SR / 100;
        events.extend(handle.events());
        if events.iter().any(|e| matches!(e, AudioEvent::DeckEnded(_))) {
            ended_at = Some(rendered);
        } else {
            assert!(peak_db(&out) > -7.0, "a gap at frame {rendered}");
        }
    }
    let seeked = events.iter().find_map(|e| match e {
        AudioEvent::Seeked {
            deck: DeckId::A,
            item_tag: Some(7),
            id: 42,
            at_sample,
        } => Some(*at_sample),
        _ => None,
    });
    assert!(seeked.is_some(), "no Seeked event: {events:?}");
    assert!(
        !events.iter().any(|e| matches!(e, AudioEvent::Underrun(_))),
        "the seek made an underrun: {events:?}"
    );
    let ended_at = ended_at.expect("the item never ended");
    assert!(
        ended_at < 2 * SR,
        "the item played to its old end ({ended_at} frames)"
    );
}

/// A seek keeps what the deck does: a paused deck stays paused and silent, and plays
/// from the new position when it continues.
#[test]
fn a_seek_on_a_paused_deck_keeps_it_paused() {
    let path = tone_wav("seek-paused", 48_000, 3.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    handle
        .load(DeckId::B, &path, LoadOptions::default())
        .expect("load");
    wait_ready(&handle, DeckId::B);
    handle.send(play(0, DeckId::B)).expect("send");
    let _ = render_offline(&mut mixer, SR / 10);
    let now = handle.now();
    handle
        .send(Command::new(now, CommandKind::Pause(DeckId::B)))
        .expect("pause");
    let _ = render_offline(&mut mixer, SR / 10);
    assert_eq!(handle.deck_state(DeckId::B), DeckState::Paused);

    let decoder = Decoder::open_at(&path, Duration::from_millis(2_500)).expect("open");
    handle
        .seek_decoder(DeckId::B, decoder, None, 1)
        .expect("seek");
    let out = render_offline(&mut mixer, SR / 10);
    assert!(out.iter().all(|s| *s == 0.0), "a paused deck made sound");
    assert_eq!(handle.deck_state(DeckId::B), DeckState::Paused);
    assert!(
        handle
            .events()
            .any(|e| matches!(e, AudioEvent::Seeked { id: 1, .. })),
        "no Seeked event"
    );

    let now = handle.now();
    handle.send(play(now, DeckId::B)).expect("continue");
    let out = render_offline(&mut mixer, SR);
    assert!(
        peak_db(frames(&out, 0, SR / 4)) > -7.0,
        "no audio after the seek"
    );
    assert!(
        handle
            .events()
            .any(|e| matches!(e, AudioEvent::DeckEnded(_))),
        "the deck did not end 0.5 s after the new position"
    );
}

/// A seek that was made for another item does nothing to the item on the deck.
#[test]
fn a_seek_for_another_item_is_dropped() {
    let path = tone_wav("seek-stale", 48_000, 1.0, 1_000.0, -6.0);
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    let options = LoadOptions {
        item_tag: Some(2),
        ..LoadOptions::default()
    };
    handle.load(DeckId::C, &path, options).expect("load");
    wait_ready(&handle, DeckId::C);
    handle.send(play(0, DeckId::C)).expect("send");
    let _ = render_offline(&mut mixer, SR / 10);

    let decoder = Decoder::open_at(&path, Duration::ZERO).expect("open");
    handle
        .seek_decoder(DeckId::C, decoder, Some(1), 9)
        .expect("seek");
    let _ = render_offline(&mut mixer, SR);
    let events: Vec<_> = handle.events().collect();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AudioEvent::Seeked { .. })),
        "a stale seek was taken: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, AudioEvent::DeckEnded(_))),
        "the item did not play to its own end: {events:?}"
    );
}
