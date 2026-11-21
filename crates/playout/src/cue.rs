//! The cue output consumes only the PFL ring. It cannot read Program.
use radio_audio_device::{AudioSource, Output};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub(crate) struct Cue {
    pub(crate) open: Arc<AtomicBool>,
    pub(crate) select: Sender<String>,
    pub(crate) errors: Receiver<String>,
}
struct CueSource {
    ring: Option<rtrb::Consumer<f32>>,
    give_back: Sender<rtrb::Consumer<f32>>,
}
impl Drop for CueSource {
    fn drop(&mut self) {
        if let Some(ring) = self.ring.take() {
            let _ = self.give_back.send(ring);
        }
    }
}
impl AudioSource for CueSource {
    fn render(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        if channels == 0 {
            return;
        }
        let Some(ring) = &mut self.ring else { return };
        let limit = (out.len() / channels * 2 * 3).max(2);
        // Bound this work even while the mixer produces more frames.
        let discard = ring.slots().saturating_sub(limit) / 2;
        for _ in 0..discard {
            let _ = ring.pop();
            let _ = ring.pop();
        }
        for frame in out.chunks_exact_mut(channels) {
            if ring.slots() < 2 {
                break;
            }
            let left = ring.pop().unwrap_or(0.0);
            let right = ring.pop().unwrap_or(0.0);
            if channels == 1 {
                frame[0] = (left + right) * 0.5;
            } else {
                frame[0] = left;
                frame[1] = right;
            }
        }
    }
}
impl Cue {
    pub(crate) fn start(ring: rtrb::Consumer<f32>) -> Self {
        let open = Arc::new(AtomicBool::new(false));
        let worker_open = Arc::clone(&open);
        let (select, names) = mpsc::channel::<String>();
        let (tx, errors) = mpsc::channel();
        let spawn_error = tx.clone();
        let worker = std::thread::Builder::new()
            .name("cue-output".into())
            .spawn(move || {
                let (give_back, returned) = mpsc::channel();
                let mut ring = Some(ring);
                let mut output: Option<Output> = None;
                let mut current = String::new();
                let mut changed = false;
                loop {
                    match names.recv_timeout(Duration::from_millis(10)) {
                        Ok(name) => {
                            current = name;
                            changed = true;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if changed {
                        worker_open.store(false, Ordering::Release);
                        if let Some(out) = output.as_mut() {
                            if let Some(source) = out.take_source() {
                                drop(source);
                                output = None;
                            } else {
                                continue;
                            }
                        }
                        changed = false;
                    }
                    if ring.is_none() {
                        ring = returned.try_recv().ok();
                    }
                    if let Some(out) = &output {
                        if let Ok(event) = out.events().try_recv() {
                            let _ = tx.send(format!("Cue output lost: {event:?}"));
                            worker_open.store(false, Ordering::Release);
                            changed = true;
                        }
                        continue;
                    }
                    if let Some(mut ready) = ring.take() {
                        while ready.pop().is_ok() {}
                        if current.is_empty() {
                            ring = Some(ready);
                            continue;
                        }
                        let source = Box::new(CueSource {
                            ring: Some(ready),
                            give_back: give_back.clone(),
                        });
                        match Output::open(Some(&current), 48_000, source) {
                            Ok(out) if out.sample_rate() == 48_000 => {
                                output = Some(out);
                                worker_open.store(true, Ordering::Release);
                            }
                            Ok(_) => {
                                let _ = tx.send("Cue device must run at 48000 Hz".into());
                                current.clear();
                            }
                            Err(e) => {
                                let _ = tx.send(format!("Cue output: {e}"));
                                current.clear();
                            }
                        }
                    }
                }
            });
        if let Err(error) = worker {
            let _ = spawn_error.send(format!("Cue output thread: {error}"));
        }
        Self {
            open,
            select,
            errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cue_discards_old_frames_when_callbacks_fall_behind() {
        let (mut producer, consumer) = rtrb::RingBuffer::new(100);
        for value in 0..40 {
            producer.push(value as f32).unwrap();
        }
        let (give_back, _) = mpsc::channel();
        let mut source = CueSource {
            ring: Some(consumer),
            give_back,
        };
        let mut out = [0.0; 4];
        source.render(&mut out, 2);
        assert!(out[0] >= 28.0, "old cue frames must be discarded");
        assert!(out[2] > out[0], "frame order must stay intact");
    }
    #[test]
    fn cue_device_source_reads_only_the_pfl_ring() {
        let (mut producer, consumer) = rtrb::RingBuffer::new(16);
        producer.push_entire_slice(&[0.2, 0.3, 0.4, 0.5]).unwrap();
        let (give_back, returned) = mpsc::channel();
        let mut source = CueSource {
            ring: Some(consumer),
            give_back,
        };
        let mut out = [1.0; 12];
        source.render(&mut out, 4);
        assert_eq!(
            out,
            [0.2, 0.3, 0.0, 0.0, 0.4, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
        drop(source);
        assert!(
            returned.try_recv().is_ok(),
            "device changes must retain the cue ring"
        );
    }
}
