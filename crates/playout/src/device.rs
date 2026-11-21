//! Device recovery runs away from the playout control thread.
use radio_audio::{CHANNELS, SAMPLE_RATE};
use radio_audio_device::{AudioSource, DeviceEvent, NullOutput, Output, SampleClock};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::time::{Duration, Instant};

pub(crate) enum State {
    Lost(String),
    Running(String, SampleClock),
    Paced(SampleClock),
}

pub(crate) struct Device {
    pub(crate) events: Receiver<State>,
    stop: Arc<AtomicBool>,
}

impl Drop for Device {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Device {
    pub(crate) fn open(
        name: Option<String>,
        source: Box<dyn AudioSource>,
    ) -> Result<(Self, String, SampleClock), String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("output-recovery".into())
            .spawn(move || {
                let out = match Output::open(name.as_deref(), SAMPLE_RATE, source) {
                    Ok(out) if out.sample_rate() == SAMPLE_RATE => {
                        out.with_reclaim_timeout(Duration::ZERO)
                    }
                    Ok(_) => {
                        let _ = ready_tx.send(Err("Output device must run at 48000 Hz".into()));
                        return;
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok((out.device_name().to_owned(), out.clock())));
                recover(out, tx, stopped);
            })
            .map_err(|e| e.to_string())?;
        let (name, clock) = ready_rx.recv().map_err(|e| e.to_string())??;
        Ok((Self { events, stop }, name, clock))
    }
}

trait Backend {
    fn name(&self) -> String;
    fn clock(&self) -> SampleClock;
    fn lost(&mut self) -> Option<String>;
    fn take_source(&mut self) -> Option<Box<dyn AudioSource>>;
    fn park_source(&mut self, source: Box<dyn AudioSource>);
    fn reopen_on(&mut self, name: Option<&str>) -> bool;
}
impl Backend for Output {
    fn name(&self) -> String {
        self.device_name().to_owned()
    }
    fn clock(&self) -> SampleClock {
        self.clock()
    }
    fn lost(&mut self) -> Option<String> {
        self.events()
            .try_recv()
            .ok()
            .map(|DeviceEvent::Lost { name }| name)
    }
    fn take_source(&mut self) -> Option<Box<dyn AudioSource>> {
        self.take_source()
    }
    fn park_source(&mut self, source: Box<dyn AudioSource>) {
        self.park_source(source);
    }
    fn reopen_on(&mut self, name: Option<&str>) -> bool {
        self.request_device(name);
        self.reopen().is_ok() && self.sample_rate() == SAMPLE_RATE
    }
}
fn recover(mut out: impl Backend, tx: mpsc::Sender<State>, stopped: Arc<AtomicBool>) {
    let original = out.name();

    let mut lost = false;
    let mut null = None;
    let mut backoff = Duration::from_secs(1);
    let mut retry = Instant::now();
    while !stopped.load(Ordering::Acquire) {
        while let Some(name) = out.lost() {
            if !lost {
                lost = true;
                backoff = Duration::from_secs(1);
                retry = Instant::now() + backoff;
                let _ = tx.send(State::Lost(name));
            }
        }
        if lost {
            if null.is_none()
                && let Some(source) = out.take_source()
            {
                match NullOutput::start(SAMPLE_RATE, CHANNELS as u16, source) {
                    Ok(paced) => {
                        let _ = tx.send(State::Paced(paced.clock()));
                        null = Some(paced);
                    }
                    Err(_) => return,
                }
            }
            if Instant::now() >= retry
                && let Some(paced) = null.take()
            {
                let Ok(source) = paced.into_source() else {
                    return;
                };
                out.park_source(source);
                let mut opened = out.reopen_on(Some(&original));
                if !opened {
                    opened = out.reopen_on(None);
                }
                if opened {
                    lost = false;
                    let _ = tx.send(State::Running(out.name(), out.clock()));
                } else {
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    retry = Instant::now() + backoff;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Source {
        tap: rtrb::Producer<f32>,
    }
    impl AudioSource for Source {
        fn render(&mut self, out: &mut [f32], _: usize) {
            out.fill(0.25);
            for sample in out {
                let _ = self.tap.push(*sample);
            }
        }
    }
    type Attempts = Arc<Mutex<Vec<(Option<String>, Instant)>>>;
    struct Fake {
        source: Option<Box<dyn AudioSource>>,
        lost: bool,
        calls: Attempts,
    }
    impl Backend for Fake {
        fn name(&self) -> String {
            "Test card".into()
        }
        fn clock(&self) -> SampleClock {
            SampleClock::new()
        }
        fn lost(&mut self) -> Option<String> {
            if std::mem::take(&mut self.lost) {
                Some(self.name())
            } else {
                None
            }
        }
        fn take_source(&mut self) -> Option<Box<dyn AudioSource>> {
            self.source.take()
        }
        fn park_source(&mut self, source: Box<dyn AudioSource>) {
            self.source = Some(source);
        }
        fn reopen_on(&mut self, name: Option<&str>) -> bool {
            let mut calls = self.calls.lock().unwrap();
            calls.push((name.map(str::to_owned), Instant::now()));
            calls.len() >= 4
        }
    }

    #[test]
    fn lost_device_keeps_tap_running_and_retries_name_then_default_with_backoff() {
        let (tap, mut audio) = rtrb::RingBuffer::new(96000);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = Fake {
            source: Some(Box::new(Source { tap })),
            lost: true,
            calls: Arc::clone(&calls),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        let worker = std::thread::spawn(move || recover(fake, tx, stopped));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            State::Lost(_)
        ));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            State::Paced(_)
        ));
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            audio.pop().unwrap(),
            0.25,
            "the Icecast tap still receives audio"
        );
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                State::Running(_, _) => break,
                State::Paced(_) => {
                    while audio.pop().is_ok() {}
                    std::thread::sleep(Duration::from_millis(80));
                    assert_eq!(
                        audio.pop().unwrap(),
                        0.25,
                        "tap continues after a failed reopen"
                    );
                }
                State::Lost(_) => {}
            }
        }
        stop.store(true, Ordering::Release);
        worker.join().unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.iter().map(|c| c.0.as_deref()).collect::<Vec<_>>(),
            vec![Some("Test card"), None, Some("Test card"), None]
        );
        assert!(calls[0].1.duration_since(start) >= Duration::from_secs(1));
        assert!(calls[2].1.duration_since(calls[0].1) >= Duration::from_secs(2));
    }
}
