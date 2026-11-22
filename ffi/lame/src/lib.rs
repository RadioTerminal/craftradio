//! A small MP3 encoder on top of the LAME shared library (libmp3lame 3.100).
//!
//! This crate declares only the LAME functions that it calls. It links LAME dynamically, so a
//! person can replace the library (docs/mp3-release.md). All unsafe code stays in this file.

use std::ffi::{c_float, c_int, c_uchar, c_void};
use std::fmt;
use std::ptr::NonNull;

/// The C functions from `lame.h` that this crate calls. `lame_t` is an opaque pointer.
mod sys {
    use super::{c_float, c_int, c_uchar, c_void};

    unsafe extern "C" {
        pub fn lame_init() -> *mut c_void;
        pub fn lame_close(gfp: *mut c_void) -> c_int;
        pub fn lame_set_num_channels(gfp: *mut c_void, channels: c_int) -> c_int;
        pub fn lame_set_in_samplerate(gfp: *mut c_void, rate: c_int) -> c_int;
        pub fn lame_set_brate(gfp: *mut c_void, kbps: c_int) -> c_int;
        pub fn lame_set_bWriteVbrTag(gfp: *mut c_void, write: c_int) -> c_int;
        pub fn lame_init_params(gfp: *mut c_void) -> c_int;
        pub fn lame_encode_buffer_interleaved_ieee_float(
            gfp: *mut c_void,
            pcm: *const c_float,
            samples_per_channel: c_int,
            mp3buf: *mut c_uchar,
            mp3buf_size: c_int,
        ) -> c_int;
        pub fn lame_encode_flush(gfp: *mut c_void, mp3buf: *mut c_uchar, size: c_int) -> c_int;
    }
}

/// An error from LAME or from the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `lame_init` returned no encoder.
    Init,
    /// LAME refused a setting. The value is the name of the setting and the LAME code.
    Setting(&'static str, i32),
    /// The number of samples is not a multiple of the number of channels.
    PartialFrame,
    /// The input is too long for one call.
    TooLong,
    /// LAME returned an error code while it encoded.
    Encode(i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init => write!(f, "LAME could not start an encoder"),
            Self::Setting(name, code) => write!(f, "LAME refused {name} (code {code})"),
            Self::PartialFrame => write!(f, "the samples do not fill every channel"),
            Self::TooLong => write!(f, "too many samples for one call"),
            Self::Encode(code) => write!(f, "LAME failed to encode (code {code})"),
        }
    }
}

impl std::error::Error for Error {}

/// The settings of a constant-bitrate encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// 1 (mono) or 2 (stereo).
    pub channels: u8,
    /// The input sample rate in Hz. LAME resamples it when MP3 does not support it.
    pub sample_rate: u32,
    /// The bitrate in kbit/s.
    pub kbps: u32,
}

/// A LAME encoder. It frees the LAME state when it drops.
#[derive(Debug)]
pub struct Encoder {
    gfp: NonNull<c_void>,
    channels: usize,
}

// SAFETY: LAME keeps all encoder state behind `gfp` and has no thread-local state.
// `Encoder` owns `gfp` and gives no shared access to it, so a move to another thread is safe.
unsafe impl Send for Encoder {}

impl Encoder {
    /// Make an encoder with the given settings. LAME writes no Xing/VBR tag, because a stream
    /// has no start to put it at.
    pub fn new(settings: Settings) -> Result<Self, Error> {
        // SAFETY: `lame_init` takes no arguments and returns a new state or null.
        let raw = unsafe { sys::lame_init() };
        let gfp = NonNull::new(raw).ok_or(Error::Init)?;
        // From here, `drop` frees the state, also when a setting below fails.
        let encoder = Self {
            gfp,
            channels: usize::from(settings.channels),
        };
        let int = |v: u32| c_int::try_from(v).unwrap_or(c_int::MAX);
        encoder.set(
            "channels",
            c_int::from(settings.channels),
            sys::lame_set_num_channels,
        )?;
        encoder.set(
            "sample rate",
            int(settings.sample_rate),
            sys::lame_set_in_samplerate,
        )?;
        encoder.set("bitrate", int(settings.kbps), sys::lame_set_brate)?;
        encoder.set("VBR tag", 0, sys::lame_set_bWriteVbrTag)?;
        // SAFETY: `gfp` is a live state from `lame_init` that this encoder owns.
        let code = unsafe { sys::lame_init_params(encoder.gfp.as_ptr()) };
        if code < 0 {
            return Err(Error::Setting("parameters", code));
        }
        Ok(encoder)
    }

    fn set(
        &self,
        name: &'static str,
        value: c_int,
        setter: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    ) -> Result<(), Error> {
        // SAFETY: `setter` is a LAME setter that takes a live state and a value.
        let code = unsafe { setter(self.gfp.as_ptr(), value) };
        if code < 0 {
            Err(Error::Setting(name, code))
        } else {
            Ok(())
        }
    }

    /// Encode interleaved samples in the range -1.0 to 1.0. LAME keeps a short delay, so the
    /// output can be empty for a small input.
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, Error> {
        if self.channels == 0 || !samples.len().is_multiple_of(self.channels) {
            return Err(Error::PartialFrame);
        }
        let per_channel = samples.len() / self.channels;
        let frames = c_int::try_from(per_channel).map_err(|_| Error::TooLong)?;
        // LAME documents this worst case: 1.25 times the samples per channel, plus 7200 bytes.
        let capacity = per_channel + per_channel / 4 + 7200;
        let size = c_int::try_from(capacity).map_err(|_| Error::TooLong)?;
        let mut out = vec![0u8; capacity];
        // SAFETY: `samples` holds `frames` frames for `self.channels` channels, as set in `new`.
        // `out` has `size` writable bytes, and `gfp` is a live state that this encoder owns.
        let written = unsafe {
            sys::lame_encode_buffer_interleaved_ieee_float(
                self.gfp.as_ptr(),
                samples.as_ptr(),
                frames,
                out.as_mut_ptr(),
                size,
            )
        };
        let written = usize::try_from(written).map_err(|_| Error::Encode(written))?;
        out.truncate(written);
        Ok(out)
    }

    /// Encode the samples that LAME still holds, and pad the last frame.
    pub fn flush(&mut self) -> Result<Vec<u8>, Error> {
        let mut out = vec![0u8; 7200];
        let size = c_int::try_from(out.len()).unwrap_or(c_int::MAX);
        // SAFETY: `out` has `size` writable bytes, and `gfp` is a live state that this encoder owns.
        let written = unsafe { sys::lame_encode_flush(self.gfp.as_ptr(), out.as_mut_ptr(), size) };
        let written = usize::try_from(written).map_err(|_| Error::Encode(written))?;
        out.truncate(written);
        Ok(out)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: `gfp` came from `lame_init`, and this is its only `lame_close`.
        unsafe {
            sys::lame_close(self.gfp.as_ptr());
        }
    }
}
