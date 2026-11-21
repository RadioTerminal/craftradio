//! A small pool of worker threads for file I/O, so the control thread never waits on a
//! disk. Workers post each result back on one channel, and the control thread takes the
//! results on every tick.

use radio_audio::{Clip, Decoder};
use radio_engine::RowId;
use radio_model::{ItemId, Timing};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// The number of worker threads.
const WORKERS: usize = 3;

type Job = Box<dyn FnOnce() + Send>;

/// A result from a worker thread or from the log writer.
pub(crate) enum Done {
    /// A decoder for the load with this tag, or the reason it failed.
    Opened {
        tag: u64,
        result: Result<Decoder, String>,
    },
    /// The as-played row of the load with this tag.
    Row {
        tag: u64,
        result: Result<RowId, String>,
    },
    /// The audio files of the emergency folder, in name order.
    Folder(Vec<PathBuf>),
    /// The analysis of an item without timing.
    Analysed(ItemId, Option<Timing>),
    /// A decoded cartwall clip for a slot.
    Cart(usize, ItemId, Result<Clip, String>),
}

pub(crate) struct Workers {
    jobs: Option<Sender<Job>>,
    pub(crate) done_tx: Sender<Done>,
    pub(crate) done_rx: Receiver<Done>,
}

impl Workers {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for n in 0..WORKERS {
            let rx = Arc::clone(&rx);
            let _ = std::thread::Builder::new()
                .name(format!("playout-worker-{n}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(rx) => rx.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                });
        }
        let (done_tx, done_rx) = channel();
        Self {
            jobs: Some(tx),
            done_tx,
            done_rx,
        }
    }

    /// Runs `f` on a worker and posts what it returns.
    pub(crate) fn run(&self, f: impl FnOnce() -> Done + Send + 'static) {
        let done = self.done_tx.clone();
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Box::new(move || {
                let _ = done.send(f());
            }));
        }
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.jobs = None;
    }
}
