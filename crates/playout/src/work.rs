//! Worker threads for file I/O, so the control thread never waits on a disk. Workers
//! post each result back on one channel, and the control thread takes the results on
//! every tick. The log query lane handles store reads. The fast lane opens files, lists
//! the fallback folder and decodes carts. The slow lane analyses files and evaluates
//! rules scripts, so this work never delays an open.

use radio_audio::{Clip, Decoder};
use radio_engine::RowId;
use radio_model::{ItemId, Timing};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// The threads of the fast lane.
const FAST_WORKERS: usize = 2;
/// The threads of the slow lane.
const SLOW_WORKERS: usize = 1;

type Job = Box<dyn FnOnce() + Send>;

/// A result from a worker thread or from the log writer.
pub(crate) enum Done {
    Query,
    Prepared(
        Result<radio_engine::Command, radio_engine::EngineError>,
        crate::Reply,
    ),
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

/// One lane: a job queue and its threads. After a stop, the threads drop the jobs that
/// still wait.
struct Lane {
    jobs: Option<Sender<Job>>,
    stop: Arc<AtomicBool>,
}

impl Lane {
    fn new(name: &str, threads: usize) -> Self {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let stop = Arc::new(AtomicBool::new(false));
        for n in 0..threads {
            let (rx, stop) = (Arc::clone(&rx), Arc::clone(&stop));
            let _ = std::thread::Builder::new()
                .name(format!("playout-{name}-{n}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(rx) => rx.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(_) if stop.load(Ordering::Relaxed) => return,
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                });
        }
        Self {
            jobs: Some(tx),
            stop,
        }
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }
}

impl Drop for Lane {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.jobs = None;
    }
}

pub(crate) struct Workers {
    fast: Lane,
    slow: Lane,
    queries: Lane,
    pub(crate) done_tx: Sender<Done>,
    pub(crate) done_rx: Receiver<Done>,
}

impl Workers {
    pub(crate) fn new() -> Self {
        let (done_tx, done_rx) = channel();
        Self {
            fast: Lane::new("io", FAST_WORKERS),
            slow: Lane::new("analyse", SLOW_WORKERS),
            queries: Lane::new("queries", 1),
            done_tx,
            done_rx,
        }
    }

    fn job(&self, f: impl FnOnce() -> Done + Send + 'static) -> Job {
        let done = self.done_tx.clone();
        Box::new(move || {
            let _ = done.send(f());
        })
    }

    /// Runs `f` on the fast lane (opens, listings, carts) and posts what it returns.
    pub(crate) fn run(&self, f: impl FnOnce() -> Done + Send + 'static) {
        self.fast.send(self.job(f));
    }

    /// Reads logs without holding up file opens or analysis.
    pub(crate) fn run_query(&self, f: impl FnOnce() -> Done + Send + 'static) {
        self.queries.send(self.job(f));
    }

    /// Runs `f` on the slow lane (analysis) and posts what it returns.
    pub(crate) fn run_slow(&self, f: impl FnOnce() -> Done + Send + 'static) {
        self.slow.send(self.job(f));
    }
}
