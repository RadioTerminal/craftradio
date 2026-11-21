use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::device::{Chosen, Direction, choose};
use crate::{AudioSource, DeviceError, DeviceEvent, SampleClock};

/// Largest block handed to `AudioSource::render` in one call (frames).
const MAX_BLOCK_FRAMES: usize = 4096;

/// Owns the source inside the stream callback and hands it back when the stream is dropped,
/// so `reopen` can move it into a new stream without any lock on the audio thread.
struct SourceHolder {
    source: Option<Box<dyn AudioSource>>,
    give_back: Sender<Box<dyn AudioSource>>,
}

impl Drop for SourceHolder {
    fn drop(&mut self) {
        if let Some(source) = self.source.take() {
            let _ = self.give_back.send(source);
        }
    }
}

/// Default time `reopen` waits for the old stream to give the source back.
const DEFAULT_RECLAIM_TIMEOUT: Duration = Duration::from_secs(2);

/// A cpal output stream playing an [`AudioSource`].
pub struct Output {
    stream: Option<cpal::Stream>,
    parked: Option<Box<dyn AudioSource>>,
    give_back_tx: Sender<Box<dyn AudioSource>>,
    give_back_rx: Receiver<Box<dyn AudioSource>>,
    events_tx: Sender<DeviceEvent>,
    events_rx: Receiver<DeviceEvent>,
    clock: SampleClock,
    requested: Option<String>,
    prefer_rate: u32,
    reclaim_timeout: Duration,
    name: String,
    sample_rate: u32,
    channels: u16,
}

impl Output {
    /// Open output device `name` (`None` for the default) and start playing `source`.
    ///
    /// The rate is `prefer_rate` when the device supports it, otherwise the device default;
    /// [`Output::sample_rate`] reports what was used.
    pub fn open(
        name: Option<&str>,
        prefer_rate: u32,
        source: Box<dyn AudioSource>,
    ) -> Result<Output, DeviceError> {
        let (give_back_tx, give_back_rx) = channel();
        let (events_tx, events_rx) = channel();
        let mut output = Output {
            stream: None,
            parked: Some(source),
            give_back_tx,
            give_back_rx,
            events_tx,
            events_rx,
            clock: SampleClock::new(),
            requested: name.map(str::to_string),
            prefer_rate,
            reclaim_timeout: DEFAULT_RECLAIM_TIMEOUT,
            name: String::new(),
            sample_rate: 0,
            channels: 0,
        };
        output.start()?;
        Ok(output)
    }

    /// How long `reopen` waits for the old stream to release the source (default 2 s).
    /// Some backends drop the callback closure late.
    pub fn with_reclaim_timeout(mut self, timeout: Duration) -> Self {
        self.reclaim_timeout = timeout;
        self
    }

    /// Drop the current stream and open a new one on the same device request, keeping the
    /// source and the clock. Use after a [`DeviceEvent::Lost`].
    ///
    /// If the old stream has not released the source within the reclaim timeout, this
    /// returns [`DeviceError::SourceLost`] and the `Output` has no stream. Calling `reopen`
    /// again succeeds if the source arrived in the meantime; otherwise re-create the
    /// `Output` with a new source.
    pub fn reopen(&mut self) -> Result<(), DeviceError> {
        if let Some(stream) = self.stream.take() {
            drop(stream);
            self.reclaim_source(self.reclaim_timeout);
        }
        self.start()
    }

    /// The master audio clock: frames rendered so far.
    pub fn clock(&self) -> SampleClock {
        self.clock.clone()
    }

    /// Rate actually used.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channel count of the stream.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Name of the device in use.
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// Events from the device (loss). Poll with `try_recv`.
    pub fn events(&self) -> &Receiver<DeviceEvent> {
        &self.events_rx
    }

    fn reclaim_source(&mut self, wait: Duration) {
        if self.parked.is_none() {
            self.parked = self.give_back_rx.recv_timeout(wait).ok();
        }
    }

    fn start(&mut self) -> Result<(), DeviceError> {
        // A late-dropped closure may have returned the source since the last attempt.
        if self.parked.is_none() {
            self.parked = self.give_back_rx.try_recv().ok();
        }
        let source = self.parked.take().ok_or(DeviceError::SourceLost)?;
        let chosen = match choose(
            self.requested.as_deref(),
            Direction::Output,
            Some(self.prefer_rate),
        ) {
            Ok(c) => c,
            Err(e) => {
                self.parked = Some(source);
                return Err(e);
            }
        };
        let holder = SourceHolder {
            source: Some(source),
            give_back: self.give_back_tx.clone(),
        };
        let result = build(&chosen, holder, &self.clock, &self.events_tx);
        match result {
            Ok(stream) => {
                self.name = chosen.name;
                self.sample_rate = chosen.config.sample_rate;
                self.channels = chosen.config.channels;
                self.stream = Some(stream);
                Ok(())
            }
            Err(e) => {
                // The failed build dropped the holder, which sent the source back.
                self.reclaim_source(Duration::from_millis(200));
                Err(e)
            }
        }
    }
}

fn build(
    chosen: &Chosen,
    holder: SourceHolder,
    clock: &SampleClock,
    events: &Sender<DeviceEvent>,
) -> Result<cpal::Stream, DeviceError> {
    let stream = match chosen.format {
        SampleFormat::F32 => build_typed::<f32>(chosen, holder, clock, events),
        SampleFormat::I16 => build_typed::<i16>(chosen, holder, clock, events),
        SampleFormat::U16 => build_typed::<u16>(chosen, holder, clock, events),
        SampleFormat::I32 => build_typed::<i32>(chosen, holder, clock, events),
        other => {
            return Err(DeviceError::Unsupported(format!(
                "sample format {other} on {}",
                chosen.name
            )));
        }
    }?;
    stream
        .play()
        .map_err(|e| DeviceError::Stream(format!("{}: {e}", chosen.name)))?;
    Ok(stream)
}

fn build_typed<T>(
    chosen: &Chosen,
    mut holder: SourceHolder,
    clock: &SampleClock,
    events: &Sender<DeviceEvent>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(chosen.config.channels.max(1));
    let clock = clock.clone();
    // Allocated here, before the stream starts; the callback only reuses it.
    let mut scratch = vec![0.0f32; MAX_BLOCK_FRAMES * channels];
    let data_cb = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        for chunk in data.chunks_mut(MAX_BLOCK_FRAMES * channels) {
            let frames = chunk.len() / channels;
            let block = &mut scratch[..frames * channels];
            block.fill(0.0);
            if let Some(source) = holder.source.as_mut() {
                source.render(block, channels);
            }
            for (out, sample) in chunk.iter_mut().zip(block.iter()) {
                *out = T::from_sample(*sample);
            }
            // A host buffer that is not a whole number of frames: keep the tail silent.
            let tail = frames * channels;
            if tail < chunk.len() {
                chunk[tail..].fill(T::from_sample(0.0f32));
            }
            clock.advance(frames);
        }
    };
    let events = events.clone();
    let name = chosen.name.clone();
    let err_cb = move |err: cpal::Error| {
        if matches!(
            err.kind(),
            cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated
        ) {
            let _ = events.send(DeviceEvent::Lost { name: name.clone() });
        }
    };
    chosen
        .device
        .build_output_stream::<T, _, _>(chosen.config, data_cb, err_cb, None)
        .map_err(|e| DeviceError::Stream(format!("{}: {e}", chosen.name)))
}
