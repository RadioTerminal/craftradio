//! The compressor and lookahead limiter on Program.

use radio_audio::{
    AudioError, Command, CommandKind, CompressorConfig, DeckId, LimiterConfig, LiveInput,
    LoadOptions, Mixer, MixerConfig, MixerHandle, ProgramDynamics, Strip, drain_offline,
    render_offline,
};

use crate::fixtures::{frames, peak_db, rms_db, tone_samples};

const SR: usize = 48_000;

fn limiter_only() -> ProgramDynamics {
    ProgramDynamics::default()
}

fn with_compressor() -> ProgramDynamics {
    ProgramDynamics {
        compressor: Some(CompressorConfig::default()),
        ..ProgramDynamics::default()
    }
}

/// Makes a mixer with `dynamics` that plays `samples` (stereo interleaved) on deck D from
/// sample 0.
fn ring_mixer(samples: &[f32], dynamics: ProgramDynamics) -> (Mixer, MixerHandle, LiveInput) {
    let (mut mixer, mut handle, live) = Mixer::new(MixerConfig::default());
    mixer.set_dynamics(Some(dynamics)).expect("dynamics");
    let (mut tx, rx) = rtrb::RingBuffer::new(samples.len().max(2));
    for s in samples {
        let _ = tx.push(*s);
    }
    handle
        .load_ring(DeckId::D, rx, LoadOptions::default())
        .expect("load");
    handle
        .send(Command::new(0, CommandKind::Play(DeckId::D)))
        .expect("play");
    (mixer, handle, live)
}

fn out_gain(handle: &mut MixerHandle, db: f32) {
    let gain = CommandKind::SetGain {
        strip: Strip::Out,
        db,
    };
    handle.send(Command::new(0, gain)).expect("gain");
}

/// `Mixer::render` with a compressor and limiter does not allocate.
#[test]
fn render_with_dynamics_does_not_allocate() {
    let tone = tone_samples(1.0, 1_000.0, -3.0);
    let (mut mixer, _handle, _live) = ring_mixer(&tone, with_compressor());
    let mut program = vec![0.0f32; 480 * 2];
    let mut pfl = vec![0.0f32; 480 * 2];
    assert_no_alloc::reset_violation_count();
    for _ in 0..50 {
        assert_no_alloc::assert_no_alloc(|| mixer.render(&mut program, &mut pfl));
    }
    assert_eq!(
        assert_no_alloc::violation_count(),
        0,
        "Mixer::render allocated"
    );
    assert!(peak_db(&program) > -20.0, "the deck is not audible");
}

/// A signal far over full scale comes out under the −1 dBFS ceiling. The Out meter shows
/// the limited signal.
#[test]
fn the_limiter_holds_the_ceiling_on_a_hot_signal() {
    let tone = tone_samples(1.0, 1_000.0, -3.0);
    let (mut mixer, mut handle, _live) = ring_mixer(&tone, limiter_only());
    out_gain(&mut handle, 12.0);
    let out = render_offline(&mut mixer, SR / 2);
    assert!(out.iter().all(|s| s.is_finite()), "Program is not finite");
    assert!(peak_db(&out) <= -0.99, "peak {} dBFS", peak_db(&out));
    assert!(peak_db(&out) > -3.0, "the limiter took too much");
    let meter = handle.meter_reader().read().strip(Strip::Out).peak_db;
    for ch in meter {
        assert!(ch <= -0.9, "the Out meter reads {ch} dBFS");
    }
}

/// A NaN from a deck does not make Program NaN.
#[test]
fn a_nan_from_a_deck_leaves_program_finite() {
    let mut samples = tone_samples(0.5, 1_000.0, -6.0);
    for s in samples.iter_mut().skip(1_000).take(64) {
        *s = f32::NAN;
    }
    for dynamics in [limiter_only(), with_compressor()] {
        let (mut mixer, _handle, _live) = ring_mixer(&samples, dynamics);
        let out = render_offline(&mut mixer, SR / 4);
        assert!(out.iter().all(|s| s.is_finite()), "Program is not finite");
        assert!(
            peak_db(frames(&out, 4_000, 8_000)) > -20.0,
            "Program went silent"
        );
    }
}

/// A step from silence to far over full scale stays under the ceiling from its first
/// frame.
#[test]
fn the_limiter_holds_the_ceiling_on_a_sudden_onset() {
    let mut samples = vec![0.0f32; 2_000 * 2];
    samples.extend((0..SR / 10).flat_map(|i| {
        let s = if i % 2 == 0 { 4.0f32 } else { -4.0 };
        [s, s]
    }));
    let (mut mixer, _handle, _live) = ring_mixer(&samples, limiter_only());
    let out = render_offline(&mut mixer, SR / 10);
    assert!(peak_db(&out) <= -0.99, "peak {} dBFS", peak_db(&out));
    assert!(peak_db(&out) > -6.0, "the onset is not audible");
}

/// The output does not depend on how the callback splits the frames into blocks.
#[test]
fn block_size_does_not_change_the_output() {
    let tone = tone_samples(0.5, 440.0, -2.0);
    let render = |sizes: &[usize]| {
        let (mut mixer, mut handle, _live) = ring_mixer(&tone, with_compressor());
        out_gain(&mut handle, 6.0);
        let total = SR / 5;
        let mut out = vec![0.0f32; total * 2];
        let mut pos = 0;
        let mut i = 0;
        while pos < total {
            let n = sizes[i % sizes.len()].min(total - pos);
            mixer.render(&mut out[pos * 2..(pos + n) * 2], &mut []);
            pos += n;
            i += 1;
        }
        out
    };
    let whole = render(&[SR / 5]);
    let split = render(&[1, 7, 511, 512, 513, 1_031]);
    assert!(whole == split, "the block split changed Program");
}

/// An impulse comes out exactly `latency_frames()` frames late.
#[test]
fn program_is_late_by_the_reported_latency() {
    let at = 1_000;
    let mut samples = vec![0.0f32; 4_000 * 2];
    samples[at * 2] = 0.5;
    samples[at * 2 + 1] = 0.5;
    let (mut mixer, _handle, _live) = ring_mixer(&samples, limiter_only());
    let latency = mixer.latency_frames();
    assert!(latency > 0, "a lookahead limiter has latency");
    let out = render_offline(&mut mixer, 4_000);
    let loudest = out
        .chunks(2)
        .enumerate()
        .max_by(|a, b| a.1[0].abs().total_cmp(&b.1[0].abs()))
        .map(|(i, _)| i);
    assert_eq!(loudest, Some(at + latency));
}

/// The compressor makes a loud tone quieter than the limiter alone does.
#[test]
fn the_compressor_lowers_a_loud_signal() {
    let tone = tone_samples(1.0, 1_000.0, -6.0);
    let level = |dynamics| {
        let (mut mixer, _handle, _live) = ring_mixer(&tone, dynamics);
        let out = render_offline(&mut mixer, SR / 2);
        rms_db(frames(&out, SR / 4, SR / 2))
    };
    let limited = level(limiter_only());
    let compressed = level(with_compressor());
    assert!(
        compressed < limited - 3.0,
        "compressed {compressed} dB, limited {limited} dB"
    );
}

/// The limiter lowers both channels by the same gain, so the stereo image stays.
#[test]
fn the_limiter_links_the_channels() {
    let samples: Vec<f32> = (0..SR / 2).flat_map(|_| [0.5f32, 0.05]).collect();
    let (mut mixer, mut handle, _live) = ring_mixer(&samples, limiter_only());
    out_gain(&mut handle, 12.0);
    let out = render_offline(&mut mixer, SR / 4);
    let tail = frames(&out, SR / 8, SR / 4);
    let (l, r) = (tail[0], tail[1]);
    assert!(l < 0.9, "the left channel is not limited: {l}");
    assert!((r / l - 0.1).abs() < 0.005, "left {l}, right {r}");
}

/// The mixer refuses an out-of-range setting and keeps the processing it had.
#[test]
fn the_mixer_refuses_an_out_of_range_setting() {
    let (mut mixer, _handle, _live) = Mixer::new(MixerConfig::default());
    mixer.set_dynamics(Some(limiter_only())).expect("dynamics");
    let before = mixer.latency_frames();
    let bad = ProgramDynamics {
        limiter: Some(LimiterConfig {
            lookahead_ms: 0.01,
            ..LimiterConfig::default()
        }),
        ..ProgramDynamics::default()
    };
    let result = mixer.set_dynamics(Some(bad));
    assert!(matches!(result, Err(AudioError::Dynamics(_))), "{result:?}");
    assert_eq!(mixer.latency_frames(), before);
}

/// New processing from the handle reaches a running mixer: the limiter starts to hold the
/// ceiling at the next block, and the swap does not allocate in `render`.
#[test]
fn the_handle_changes_the_processing_while_the_mixer_runs() {
    let tone = tone_samples(1.0, 1_000.0, -3.0);
    let (mut mixer, mut handle, _live) = ring_mixer(&tone, limiter_only());
    mixer.set_dynamics(None).expect("soft clip");
    out_gain(&mut handle, 12.0);
    let _ = render_offline(&mut mixer, SR / 8);
    assert_eq!(mixer.latency_frames(), 0, "the soft clip has no lookahead");
    handle.set_dynamics(Some(limiter_only())).expect("limiter");
    let mut program = vec![0.0f32; 480 * 2];
    assert_no_alloc::reset_violation_count();
    assert_no_alloc::assert_no_alloc(|| mixer.render(&mut program, &mut []));
    assert_eq!(assert_no_alloc::violation_count(), 0, "the swap allocated");
    assert!(mixer.latency_frames() > 0, "the mixer took the limiter");
    let limited = render_offline(&mut mixer, SR / 8);
    assert!(
        peak_db(frames(&limited, SR / 16, SR / 8)) <= -0.99,
        "the limiter holds the ceiling after the swap"
    );
}

/// The handle refuses an out-of-range setting, and the mixer keeps its processing.
#[test]
fn the_handle_refuses_an_out_of_range_setting() {
    let (mut mixer, mut handle, _live) = Mixer::new(MixerConfig::default());
    mixer.set_dynamics(Some(limiter_only())).expect("dynamics");
    let before = mixer.latency_frames();
    let bad = ProgramDynamics {
        limiter: Some(LimiterConfig {
            lookahead_ms: 0.01,
            ..LimiterConfig::default()
        }),
        ..ProgramDynamics::default()
    };
    let result = handle.set_dynamics(Some(bad));
    assert!(matches!(result, Err(AudioError::Dynamics(_))), "{result:?}");
    let _ = render_offline(&mut mixer, 256);
    assert_eq!(mixer.latency_frames(), before);
}

/// Without a limiter, the compressor still runs and the soft clip keeps Program finite and
/// under full scale.
#[test]
fn a_compressor_without_a_limiter_still_soft_clips() {
    let tone = tone_samples(0.5, 1_000.0, -3.0);
    let dynamics = ProgramDynamics {
        compressor: Some(CompressorConfig::default()),
        limiter: None,
    };
    let (mut mixer, mut handle, _live) = ring_mixer(&tone, dynamics);
    out_gain(&mut handle, 12.0);
    let out = render_offline(&mut mixer, SR / 4);
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(peak_db(&out) <= 0.0, "peak {} dBFS", peak_db(&out));
}

/// An offline render that ends drains the limiter lookahead: the frames of the render are
/// the input frames plus `latency_frames()`, and the last input frame is in the render,
/// `latency_frames()` from its end.
#[test]
fn an_offline_render_keeps_its_last_frames() {
    const N: usize = 4_800;
    // A quiet ramp, so the limiter leaves every sample as it is, and the last one is loud
    // enough to find.
    let samples: Vec<f32> = (0..N)
        .flat_map(|i| {
            let s = 0.25 * (i + 1) as f32 / N as f32;
            [s, s]
        })
        .collect();
    let (mut mixer, _handle, _live) = ring_mixer(&samples, limiter_only());
    let latency = mixer.latency_frames();
    assert!(latency > 0, "the limiter has a lookahead");
    let mut out = render_offline(&mut mixer, N);
    out.extend(drain_offline(&mut mixer));
    assert_eq!(out.len() / 2, N + latency, "the render has every frame");
    let last = out[(N + latency - 1) * 2];
    let want = samples[(N - 1) * 2];
    assert!(
        (last - want).abs() < 1e-3,
        "the last input frame {want} ends the render, not {last}"
    );
}

/// A swap from one chain to another frees nothing on the render thread: the old chain goes
/// back to the handle.
#[test]
fn a_swap_between_two_chains_does_not_allocate_or_free_in_render() {
    let tone = tone_samples(1.0, 1_000.0, -3.0);
    let (mut mixer, mut handle, _live) = ring_mixer(&tone, limiter_only());
    let _ = render_offline(&mut mixer, 480);
    handle
        .set_dynamics(Some(with_compressor()))
        .expect("compressor");
    let mut program = vec![0.0f32; 480 * 2];
    assert_no_alloc::reset_violation_count();
    assert_no_alloc::assert_no_alloc(|| mixer.render(&mut program, &mut []));
    assert_eq!(
        assert_no_alloc::violation_count(),
        0,
        "the swap allocated or freed"
    );
}

/// Every corner of the ranges that the engine accepts prepares in the processors.
#[test]
fn the_engine_ranges_sit_inside_the_processor_ranges() {
    use radio_model::{DynamicsSettings, dynamics_range as r};
    let ends = |(lo, hi): (f32, f32)| [lo, hi];
    for threshold_db in ends(r::THRESHOLD_DB) {
        for ratio in ends(r::RATIO) {
            for attack_ms in ends(r::ATTACK_MS) {
                for release_ms in ends(r::COMP_RELEASE_MS) {
                    for makeup_db in ends(r::MAKEUP_DB) {
                        for ceiling_db in ends(r::CEILING_DB) {
                            for limit_release in ends(r::LIMIT_RELEASE_MS) {
                                let d = DynamicsSettings {
                                    compressor: radio_model::CompressorSettings {
                                        enabled: true,
                                        threshold_db,
                                        ratio,
                                        attack_ms,
                                        release_ms,
                                        makeup_db,
                                    },
                                    limiter: radio_model::LimiterSettings {
                                        enabled: true,
                                        ceiling_db,
                                        release_ms: limit_release,
                                    },
                                };
                                assert_eq!(d.validate(), Ok(()));
                                let (mut mixer, _h, _l) = Mixer::new(MixerConfig::default());
                                let result = mixer.set_dynamics(Some(ProgramDynamics::from(&d)));
                                assert!(result.is_ok(), "{d:?}: {result:?}");
                            }
                        }
                    }
                }
            }
        }
    }
}
