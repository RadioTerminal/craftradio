//! Capture devices run on their own thread. A deck controls their audible interval.
use radio_audio_device::{Input, PiController, Resampler};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::time::Duration;

/// A capture source held until its deck is released. Tests can supply a fake producer.
pub trait Capture: Send {
    fn error(&mut self) -> Option<String> {
        None
    }
}

pub type OpenInput = fn(&str, [u16; 2], rtrb::Producer<f32>) -> Result<Box<dyn Capture>, String>;

struct DeviceCapture {
    stop: Arc<AtomicBool>,
    errors: Receiver<String>,
}
impl Capture for DeviceCapture {
    fn error(&mut self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}
impl Drop for DeviceCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Open the selected input and convert its samples to the mixer rate.
pub fn open_input(
    name: &str,
    channels: [u16; 2],
    mut producer: rtrb::Producer<f32>,
) -> Result<Box<dyn Capture>, String> {
    let name = name.to_owned();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (errors_tx, errors) = mpsc::channel();
    std::thread::Builder::new()
        .name("live-input".into())
        .spawn(move || {
            let (raw_tx, mut raw_rx) = rtrb::RingBuffer::new(48_000 * 2);
            let setup = Input::open(Some(&name), channels, raw_tx)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    Resampler::new(48_000.0 / f64::from(input.sample_rate()), 2, 256)
                        .map(|r| (input, r))
                        .map_err(|e| e.to_string())
                });
            let (input, mut resampler) = match setup {
                Ok(pair) => {
                    let _ = ready_tx.send(Ok(()));
                    pair
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let mut block = [0.0; 512];
            let mut output = Vec::new();
            let mut drift = PiController::new();
            let capacity = producer.slots();
            while !stopped.load(Ordering::Acquire) {
                if let Ok(event) = input.events().try_recv() {
                    let _ = errors_tx.send(format!("Input device lost: {event:?}"));
                    break;
                }
                if raw_rx.slots() < block.len() {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                for sample in &mut block {
                    *sample = raw_rx.pop().unwrap_or(0.0);
                }
                output.clear();
                let ratio = drift.update((capacity - producer.slots()) as f32, 1024.0);
                if let Err(e) = resampler
                    .set_relative_ratio(ratio)
                    .and_then(|()| resampler.process(&block, &mut output))
                {
                    let _ = errors_tx.send(e.to_string());
                    break;
                }
                let n = (producer.slots() / 2 * 2).min(output.len());
                let _ = producer.push_entire_slice(&output[..n]);
            }
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|e| e.to_string())??;
    Ok(Box::new(DeviceCapture { stop, errors }))
}
