//! Where the mixer runs: a sound card, a system-clock timer, or offline on request.

use radio_audio::{CHANNELS, Mixer};
use radio_audio_device::AudioSource;

/// Frames per pass through the scratch buffer.
const SCRATCH_FRAMES: usize = 1024;

/// The mixer as an [`AudioSource`], with an optional tap of Program for Icecast.
pub(crate) struct ProgramSource {
    pub(crate) mixer: Mixer,
    pub(crate) scratch: Vec<f32>,
    pub(crate) tap: Option<rtrb::Producer<f32>>,
}

impl ProgramSource {
    pub(crate) fn new(mixer: Mixer, tap: Option<rtrb::Producer<f32>>) -> Self {
        Self {
            mixer,
            scratch: vec![0.0; SCRATCH_FRAMES * CHANNELS],
            tap,
        }
    }
}

/// Pushes Program into the tap as far as it has room; a slow encoder loses audio, never
/// the mixer.
pub(crate) fn feed_tap(tap: &mut Option<rtrb::Producer<f32>>, program: &[f32]) {
    if let Some(tap) = tap {
        let room = tap.slots() / CHANNELS * CHANNELS;
        if let Some(part) = program.get(..room.min(program.len())) {
            let _ = tap.push_entire_slice(part);
        }
    }
}

impl AudioSource for ProgramSource {
    fn render(&mut self, out: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        for chunk in out.chunks_mut(SCRATCH_FRAMES * channels) {
            let frames = chunk.len() / channels;
            let Some(program) = self.scratch.get_mut(..frames * CHANNELS) else {
                chunk.fill(0.0);
                continue;
            };
            self.mixer.render(program, &mut []);
            feed_tap(&mut self.tap, program);
            for (d, s) in chunk
                .chunks_exact_mut(channels)
                .zip(program.as_chunks::<CHANNELS>().0)
            {
                if channels == 1 {
                    d[0] = (s[0] + s[1]) * 0.5;
                } else {
                    d.fill(0.0);
                    d[0] = s[0];
                    d[1] = s[1];
                }
            }
        }
    }
}
