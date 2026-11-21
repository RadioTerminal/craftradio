//! Merges the edits of faders and drags: one command per 50 ms per control, then the last value.

use std::collections::HashMap;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use radio_engine::Command;

use crate::EngineSource;

/// The answers to merged commands, by control, that wait for a check.
pub(crate) type Replies = std::sync::Arc<
    std::sync::Mutex<
        Vec<(
            String,
            std::sync::mpsc::Receiver<Result<radio_engine::Outcome, radio_engine::EngineError>>,
        )>,
    >,
>;

/// The shortest time between two commands of one control.
pub(crate) const GAP: Duration = Duration::from_millis(50);
/// The merger looks for due values this often.
const POLL: Duration = Duration::from_millis(5);

/// A thread that merges commands by control.
pub(crate) struct Merger {
    tx: Option<Sender<(String, Command)>>,
    thread: Option<JoinHandle<()>>,
}

impl Merger {
    /// A merger that hands each merged command, with its control, to `sink`.
    pub(crate) fn start(sink: impl Fn(String, Command) + Send + 'static) -> Option<Self> {
        let (tx, rx) = channel::<(String, Command)>();
        let thread = std::thread::Builder::new()
            .name("ui-faders".into())
            .spawn(move || {
                let mut last: HashMap<String, Instant> = HashMap::new();
                let mut pending: HashMap<String, Command> = HashMap::new();
                loop {
                    let open = match rx.recv_timeout(POLL) {
                        Ok((key, cmd)) => {
                            if last.get(&key).is_none_or(|t| t.elapsed() >= GAP) {
                                sink(key.clone(), cmd);
                                pending.remove(&key);
                                last.insert(key, Instant::now());
                            } else {
                                pending.insert(key, cmd);
                            }
                            true
                        }
                        Err(RecvTimeoutError::Timeout) => true,
                        Err(RecvTimeoutError::Disconnected) => false,
                    };
                    let due: Vec<String> = pending
                        .keys()
                        .filter(|k| !open || last.get(*k).is_none_or(|t| t.elapsed() >= GAP))
                        .cloned()
                        .collect();
                    for key in due {
                        if let Some(cmd) = pending.remove(&key) {
                            sink(key.clone(), cmd);
                            last.insert(key, Instant::now());
                        }
                    }
                    if !open {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    /// Queues the latest value of a control.
    pub(crate) fn push(&self, key: String, cmd: Command) -> Result<(), String> {
        self.tx
            .as_ref()
            .ok_or("the fader thread has stopped")?
            .send((key, cmd))
            .map_err(|_| "the fader thread has stopped".to_owned())
    }
}

impl Drop for Merger {
    /// Sends the last values and stops the thread.
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl EngineSource {
    /// Takes the errors of merged commands that answered. A failed fader drops its shown
    /// value, so the snapshot shows the engine's value again.
    pub(crate) fn fader_errors(&mut self) -> Vec<String> {
        let mut errors: Vec<(String, String)> = Vec::new();
        let Ok(mut list) = self.replies.lock() else {
            return Vec::new();
        };
        list.retain(|(key, rx)| match rx.try_recv() {
            Ok(Ok(_)) => false,
            Ok(Err(e)) => {
                errors.push((key.clone(), e.to_string()));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        drop(list);
        for (key, _) in &errors {
            if let Some(strip) = key.strip_prefix("strip:") {
                self.strip_gain.remove(strip);
            }
            if let Some(rest) = key.strip_prefix("part:")
                && let Some((part, k)) = rest.split_once(':')
            {
                self.pending_settings
                    .remove(&(part.to_owned(), k.to_owned()));
            }
        }
        errors.into_iter().map(|(_, e)| e).collect()
    }

    /// Sends a merged change of control `key`. It does not wait for the engine. An error of
    /// an earlier merged command comes back here.
    pub(crate) fn merged(&mut self, key: String, cmd: Command) -> Result<usize, String> {
        if let Some(e) = self.fader_errors().into_iter().next() {
            return Err(e);
        }
        match &self.merger {
            Some(m) => m.push(key, cmd)?,
            None => {
                self.playout.send(cmd).map_err(|e| e.to_string())?;
            }
        }
        Ok(1)
    }

    /// Sends a fader change, merged by strip.
    pub(crate) fn fader(&mut self, cmd: Command) -> Result<usize, String> {
        let key = match &cmd {
            Command::MixerSetStrip { strip, gain_db, .. } => {
                if let Some(g) = gain_db {
                    self.strip_gain.insert(strip.clone(), *g);
                }
                format!("strip:{strip}")
            }
            _ => String::new(),
        };
        self.merged(key, cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn gain(db: f32) -> Command {
        Command::MixerSetStrip {
            strip: "A".into(),
            gain_db: Some(db),
            mute: None,
            pfl: None,
            route: None,
        }
    }

    #[test]
    fn a_fast_drag_sends_few_commands_and_ends_on_its_last_value() {
        let sent: Arc<Mutex<Vec<Command>>> = Arc::default();
        let log = Arc::clone(&sent);
        let merger = Merger::start(move |_, c| log.lock().unwrap().push(c)).unwrap();
        let start = Instant::now();
        for i in 0..40 {
            merger.push("A".into(), gain(-(i as f32))).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        let took = start.elapsed();
        std::thread::sleep(GAP * 3);
        let sent = sent.lock().unwrap().clone();
        let most = (took.as_millis() / GAP.as_millis()) as usize + 2;
        assert!(sent.len() <= most, "{} commands in {took:?}", sent.len());
        assert!(sent.len() >= 2, "the drag sends while it moves");
        assert_eq!(sent.last(), Some(&gain(-39.0)), "the last value follows");
        drop(merger);
    }
}
