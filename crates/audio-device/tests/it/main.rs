//! Tests run without audio hardware: they only rely on behaviour that holds with or without
//! a sound card.
use std::time::{Duration, Instant};

use radio_audio_device::{
    AudioSource, NullOutput, Output, PiController, list_inputs, list_outputs, negotiate_rate,
    ratio_for,
};

struct Silence;
impl AudioSource for Silence {
    fn render(&mut self, out: &mut [f32], _channels: usize) {
        out.fill(0.0);
    }
}

#[test]
fn enumeration_does_not_panic() {
    // Any result is fine (CI has no devices); the call must return.
    let _ = list_outputs();
    let _ = list_inputs();
}

#[test]
fn null_output_renders_in_real_time() {
    let out = NullOutput::start(48_000, 2, Box::new(Silence)).expect("start");
    let clock = out.clock();
    let t = Instant::now();
    std::thread::sleep(Duration::from_secs(1));
    let frames = clock.frames() as f64;
    let secs = t.elapsed().as_secs_f64();
    let expected = 48_000.0 * secs;
    assert!(
        (frames - expected).abs() < expected * 0.05,
        "rendered {frames} frames in {secs}s"
    );
}

#[test]
fn drift_controller_pulls_buffer_toward_target() {
    assert!(ratio_for(0.2, 0.5) > 1.0, "below target must speed up");
    assert!(ratio_for(0.8, 0.5) < 1.0, "above target must slow down");
    // The integral part keeps pushing while the error persists.
    let mut pi = PiController::new();
    let first = pi.update(0.2, 0.5);
    let later = (0..20).map(|_| pi.update(0.2, 0.5)).last().unwrap_or(first);
    assert!(later > first);
}

#[test]
fn unknown_device_name_is_an_error_naming_it() {
    let name = "no-such-device-7f3a";
    let err = Output::open(Some(name), 48_000, Box::new(Silence))
        .err()
        .expect("must fail");
    assert!(err.to_string().contains(name), "message: {err}");
}

#[test]
fn rate_negotiation_prefers_48k_else_default() {
    assert_eq!(negotiate_rate(&[(8_000, 96_000)], 48_000, 44_100), 48_000);
    assert_eq!(negotiate_rate(&[(44_100, 44_100)], 48_000, 44_100), 44_100);
}

#[test]
fn null_output_reports_a_panicking_source() {
    struct Boom;
    impl AudioSource for Boom {
        fn render(&mut self, _: &mut [f32], _: usize) {
            panic!("boom");
        }
    }
    let out = NullOutput::start(48_000, 2, Box::new(Boom)).expect("start");
    let event = out.events().recv_timeout(Duration::from_secs(2));
    assert_eq!(
        event,
        Ok(radio_audio_device::DeviceEvent::Lost {
            name: "null".into()
        })
    );
}
