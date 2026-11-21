use std::sync::mpsc::{Receiver, Sender, channel};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use crate::device::{Chosen, Direction, choose};
use crate::{DeviceError, DeviceEvent};

/// A cpal input stream that pushes two chosen device channels into a ring buffer.
pub struct Input {
    _stream: cpal::Stream,
    events_rx: Receiver<DeviceEvent>,
    name: String,
    sample_rate: u32,
}

impl Input {
    /// Open input device `name` (`None` for the default). `channels` are the zero-based device
    /// channels captured as left and right; a channel the device lacks is captured as silence.
    /// Frames are written to `producer` as interleaved stereo `f32`; when the ring is full the
    /// newest samples are dropped (the callback never waits).
    pub fn open(
        name: Option<&str>,
        channels: [u16; 2],
        producer: rtrb::Producer<f32>,
    ) -> Result<Input, DeviceError> {
        let chosen = choose(name, Direction::Input, None)?;
        let (events_tx, events_rx) = channel();
        let stream = match chosen.format {
            SampleFormat::F32 => build::<f32>(&chosen, channels, producer, events_tx),
            SampleFormat::I16 => build::<i16>(&chosen, channels, producer, events_tx),
            SampleFormat::U16 => build::<u16>(&chosen, channels, producer, events_tx),
            SampleFormat::I32 => build::<i32>(&chosen, channels, producer, events_tx),
            other => Err(DeviceError::Unsupported(format!(
                "sample format {other} on {}",
                chosen.name
            ))),
        }?;
        stream
            .play()
            .map_err(|e| DeviceError::Stream(format!("{}: {e}", chosen.name)))?;
        Ok(Input {
            _stream: stream,
            events_rx,
            name: chosen.name,
            sample_rate: chosen.config.sample_rate,
        })
    }

    /// Rate of the captured frames (the device's own; resample with `Resampler` if needed).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Name of the device in use.
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// Events from the device (loss). Poll with `try_recv`.
    pub fn events(&self) -> &Receiver<DeviceEvent> {
        &self.events_rx
    }
}

fn build<T>(
    chosen: &Chosen,
    pick: [u16; 2],
    mut producer: rtrb::Producer<f32>,
    events: Sender<DeviceEvent>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(chosen.config.channels.max(1));
    let [left, right] = pick.map(usize::from);
    let data_cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
        for frame in data.chunks_exact(channels) {
            let l = frame.get(left).map_or(0.0, |s| f32::from_sample(*s));
            let r = frame.get(right).map_or(0.0, |s| f32::from_sample(*s));
            // Whole frames only, so L/R never slip when the ring is full.
            if producer.slots() >= 2 {
                let _ = producer.push(l);
                let _ = producer.push(r);
            }
        }
    };
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
        .build_input_stream::<T, _, _>(chosen.config, data_cb, err_cb, None)
        .map_err(|e| DeviceError::Stream(format!("{}: {e}", chosen.name)))
}
