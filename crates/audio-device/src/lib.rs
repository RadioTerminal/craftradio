//! Audio devices for the radio station manager (layer L3).
//!
//! * [`Output`] plays an [`AudioSource`] on a cpal output device and exposes the master audio
//!   clock ([`SampleClock`]).
//! * [`NullOutput`] is the headless / no-sound-card case: a thread paced by the system clock
//!   drives the same [`AudioSource`] with the same kind of clock.
//! * [`Input`] captures two device channels into an `rtrb` ring buffer.
//! * [`Resampler`] and [`ratio_for`] provide drift compensation for secondary devices.
//!
//! The data callbacks never allocate, lock or perform I/O. Device errors are reported on a
//! channel as [`DeviceEvent`]s.
//!
//! This crate does not depend on `radio-audio`; the glue code implements [`AudioSource`].

mod clock;
mod device;
mod drift;
mod error;
mod input;
mod null;
mod output;

pub use clock::SampleClock;
pub use device::{DeviceInfo, list_inputs, list_outputs, negotiate_rate};
pub use drift::{PiController, Resampler, ratio_for};
pub use error::{DeviceError, DeviceEvent};
pub use input::Input;
pub use null::NullOutput;
pub use output::Output;

/// Something that produces audio for an output. Implemented by the mixer glue code.
///
/// `render` runs on the real-time audio thread: it must not allocate, lock or do I/O.
pub trait AudioSource: Send + 'static {
    /// Fill `out` with interleaved `f32` samples. `out.len()` is a multiple of `channels`.
    /// Write silence for anything that has no audio.
    fn render(&mut self, out: &mut [f32], channels: usize);
}
