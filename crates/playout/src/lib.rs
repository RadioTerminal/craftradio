//! The playout runtime (L4, above `engine`) plays the planned timeline through the decks,
//! the outputs and the as-played log. The control thread of the playout is not real
//! time. It plans again every second, and pins the items on air and on the decks. It
//! loads the next entries `preload` before their start, and skips an item that fails to
//! load. It starts each entry on its exact sample. It logs each item at its start and at
//! its end, as the mixer reports them.

mod alarm;
mod clock;
mod core;
mod cue;
mod deadair;
mod device;
mod input;
pub use input::{Capture, OpenInput, open_input};
mod ops;
mod output;
mod resume;
mod streams;
mod view;
mod work;

pub use alarm::{Alarm, RaisedAlarm};
pub use clock::{Clock, ManualClock, SystemClock};
pub use view::{
    DeckPhase, DeckRole, DeckView, LocalOutput, OutputStatus, PlayoutView, StreamState,
};

use jiff::Timestamp;
use radio_audio::{MeterReader, MixerConfig};
use radio_engine::{AsPlayed, Command, Engine, EngineError, LogHandle, Outcome};
use radio_model::Timing;
use radio_scheduler::PlanOptions;
use radio_stream::IcecastConfig;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Where Program plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputChoice {
    /// A sound card (`None` is the default device). If playout cannot open it at 48
    /// kHz, playout uses [`OutputChoice::Null`] and raises [`Alarm::OutputFallback`].
    Device { name: Option<String> },
    /// No sound card. A system-clock timer drives the mixer (headless, Icecast only).
    Null,
    /// Nothing drives the mixer, and [`Playout::render`] renders on request. Use it with
    /// a [`ManualClock`] for a deterministic run.
    Offline,
}

/// Settings for [`Playout::start`].
#[derive(Clone)]
pub struct PlayoutConfig {
    pub clock: Arc<dyn Clock>,
    pub output: OutputChoice,
    /// The mixer settings, with the Program silence detector for dead air.
    pub mixer: MixerConfig,
    /// Playout loads a file at least this long before its start (default 30 s).
    pub preload: Duration,
    /// Playout connects an internet stream this long before its start (default 5 s).
    pub stream_preroll: Duration,
    /// The length of the planned timeline (default 3 h).
    pub horizon: Duration,
    /// Planner options. Playout always continues the playlist order between plans.
    pub plan: PlanOptions,
    /// Playout sends play and fade commands this long before their sample (default 300 ms).
    pub command_lead: Duration,
    /// While the emergency fallback plays, playout checks this often if the plan can play
    /// again (default 30 s).
    pub emergency_retry: Duration,
    /// Playout skips a stream without keep-alive after this much downtime (default 10 s).
    pub stream_timeout: Duration,
    /// The emergency fallback playlist: all audio files in this folder, in name order.
    pub fallback_folder: Option<PathBuf>,
    /// One Icecast mount that gets Program.
    pub icecast: Option<IcecastConfig>,
    /// The stream title for items that hide their own title.
    pub station_name: String,
    /// Playout sends each alarm to this URL as a JSON POST, but it never waits for it.
    pub webhook: Option<String>,
    /// The longest time [`Playout::dispatch`] waits for the mixer to confirm audio.
    pub confirm_timeout: Duration,
    /// Finds the cue points of a file without timing. It runs on the analysis lane. The
    /// default is [`analyse_file`]; tests put a slow or fake one here.
    pub analyse: fn(&Path) -> Option<Timing>,
    pub open_input: OpenInput,
}

/// The default analysis: [`radio_library::analyse`], keeping only the timing.
pub fn analyse_file(path: &Path) -> Option<Timing> {
    radio_library::analyse(path).ok().map(|a| a.timing)
}

impl PlayoutConfig {
    /// The defaults, with the given clock and output.
    pub fn new(clock: Arc<dyn Clock>, output: OutputChoice) -> Self {
        Self {
            clock,
            output,
            mixer: MixerConfig::default(),
            preload: Duration::from_secs(30),
            stream_preroll: Duration::from_secs(5),
            horizon: Duration::from_secs(3 * 3600),
            plan: PlanOptions::default(),
            command_lead: Duration::from_millis(300),
            stream_timeout: Duration::from_secs(10),
            emergency_retry: Duration::from_secs(30),
            fallback_folder: None,
            icecast: None,
            station_name: String::new(),
            webhook: None,
            confirm_timeout: Duration::from_secs(1),
            analyse: analyse_file,
            open_input,
        }
    }
}

impl Default for PlayoutConfig {
    /// The system clock and the default sound card.
    fn default() -> Self {
        Self::new(Arc::new(SystemClock), OutputChoice::Device { name: None })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlayoutError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("audio: {0}")]
    Audio(#[from] radio_audio::AudioError),
    #[error("output: {0}")]
    Output(String),
    #[error("could not start the playout thread: {0}")]
    Thread(std::io::Error),
    #[error("the playout thread has stopped")]
    Stopped,
    #[error("only an offline playout renders on request")]
    NotOffline,
}

pub(crate) type Reply = Sender<Result<Outcome, EngineError>>;

pub(crate) enum Request {
    Dispatch(Command, Reply),
    Render(usize, Sender<Result<Vec<f32>, PlayoutError>>),
    Alarms(mpsc::SyncSender<RaisedAlarm>),
}

/// Sends commands to a playout from another thread. It ignores the answers.
#[derive(Clone)]
pub struct CommandSender(Sender<Request>);

impl CommandSender {
    pub fn send(&self, cmd: Command) -> Result<(), PlayoutError> {
        self.send_with_reply(cmd).map(|_| ())
    }

    /// Sends a command, and returns the receiver of its answer.
    pub fn send_with_reply(
        &self,
        cmd: Command,
    ) -> Result<Receiver<Result<Outcome, EngineError>>, PlayoutError> {
        let (tx, rx) = mpsc::channel();
        self.0
            .send(Request::Dispatch(cmd, tx))
            .map_err(|_| PlayoutError::Stopped)?;
        Ok(rx)
    }
}

/// The running playout. A drop stops the control thread, ends the open log rows as
/// interrupted and closes the outputs.
pub struct Playout {
    tx: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
    view: Arc<Mutex<PlayoutView>>,
    meters: Mutex<MeterReader>,
    log: LogHandle,
}

/// The period of the control loop when a clock drives the mixer.
const TICK: Duration = Duration::from_millis(20);
/// The capacity of each alarm stream.
const ALARM_QUEUE: usize = 256;

impl Playout {
    /// Starts playout on the station of `engine`.
    pub fn start(engine: Engine, cfg: PlayoutConfig) -> Result<Playout, PlayoutError> {
        let log = engine.log_handle()?;
        let (tx, rx) = mpsc::channel::<Request>();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("playout-control".into())
            .spawn(move || {
                let core = match core::Core::new(engine, cfg) {
                    Ok(core) => {
                        let _ = ready_tx.send(Ok((core.view(), core.meter_reader())));
                        core
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                run(core, &rx);
            })
            .map_err(PlayoutError::Thread)?;
        let (view, meters) = ready_rx.recv().map_err(|_| PlayoutError::Stopped)??;
        Ok(Playout {
            tx: Some(tx),
            thread: Some(thread),
            view,
            log,
            meters: Mutex::new(meters),
        })
    }

    fn request(&self, req: Request) -> Result<(), PlayoutError> {
        self.tx
            .as_ref()
            .ok_or(PlayoutError::Stopped)?
            .send(req)
            .map_err(|_| PlayoutError::Stopped)
    }

    /// Runs an engine command. An audio command waits for the mixer to confirm it, for
    /// `confirm_timeout` at most. Then `applied_to_audio` tells if the mixer confirmed.
    pub fn dispatch(&self, cmd: Command) -> Result<Outcome, PlayoutError> {
        let rx = self.send(cmd)?;
        rx.recv()
            .map_err(|_| PlayoutError::Stopped)?
            .map_err(PlayoutError::from)
    }

    /// Sends a command and returns at once, and the result arrives on the receiver. An
    /// offline playout confirms audio commands only during [`Playout::render`].
    pub fn send(
        &self,
        cmd: Command,
    ) -> Result<Receiver<Result<Outcome, EngineError>>, PlayoutError> {
        let (tx, rx) = mpsc::channel();
        self.request(Request::Dispatch(cmd, tx))?;
        Ok(rx)
    }

    /// The latest snapshot, with fresh meters.
    pub fn snapshot(&self) -> PlayoutView {
        let mut view = self
            .view
            .lock()
            .map(|v| v.clone())
            .unwrap_or_else(|p| p.into_inner().clone());
        if let Ok(mut m) = self.meters.lock() {
            view.meters = m.read();
        }
        view
    }

    /// A handle that sends commands from another thread, without their answers. Drop every
    /// handle before the playout, or the drop of the playout waits for them.
    pub fn command_sender(&self) -> Option<CommandSender> {
        self.tx.clone().map(CommandSender)
    }

    /// The items on the decks now. It costs no copy of the snapshot.
    pub fn deck_items(&self) -> [Option<radio_model::ItemId>; 4] {
        let read = |v: &PlayoutView| std::array::from_fn(|i| v.decks[i].item);
        self.view
            .lock()
            .map(|v| read(&v))
            .unwrap_or_else(|p| read(&p.into_inner()))
    }

    /// The revision of the engine state in the snapshot. It costs no copy.
    pub fn revision(&self) -> u64 {
        self.view
            .lock()
            .map(|v| v.revision)
            .unwrap_or_else(|p| p.into_inner().revision)
    }

    /// A meter reader for a meter display at 30 fps.
    pub fn meters(&self) -> MeterReader {
        self.meters
            .lock()
            .map(|m| m.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    /// A new alarm stream. Each call gets its own receiver. It holds 256 alarms; when it
    /// is full, playout drops the newest and counts them in [`PlayoutView::alarms_dropped`].
    pub fn alarms(&self) -> Result<Receiver<RaisedAlarm>, PlayoutError> {
        let (tx, rx) = mpsc::sync_channel(ALARM_QUEUE);
        self.request(Request::Alarms(tx))?;
        Ok(rx)
    }

    /// The as-played rows from `since`, oldest first. It reads the log on the writer
    /// thread of the engine, not on the control thread.
    pub fn history(&self, since: Timestamp, limit: u32) -> Result<Vec<AsPlayed>, PlayoutError> {
        Ok(self.log.history(since, limit)?)
    }

    /// A handle to the as-played log for another thread, for example a UI.
    pub fn log(&self) -> LogHandle {
        self.log.clone()
    }

    /// Offline only: one control tick at the current clock time, then `frames` frames of
    /// Program (stereo interleaved, 48 kHz). Render 10 ms or less per call, and move the
    /// clock by the same time between calls.
    pub fn render(&self, frames: usize) -> Result<Vec<f32>, PlayoutError> {
        let (tx, rx) = mpsc::channel();
        self.request(Request::Render(frames, tx))?;
        rx.recv().map_err(|_| PlayoutError::Stopped)?
    }
}

impl Drop for Playout {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(mut core: core::Core, rx: &Receiver<Request>) {
    let offline = core.is_offline();
    let mut last_tick = Instant::now();
    loop {
        let wait = TICK.saturating_sub(last_tick.elapsed());
        match rx.recv_timeout(wait) {
            Ok(Request::Dispatch(cmd, reply)) => core.dispatch(cmd, reply),
            Ok(Request::Render(frames, reply)) => {
                let _ = reply.send(core.render(frames));
            }
            Ok(Request::Alarms(tx)) => core.subscribe(tx),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            if !offline {
                core.tick();
            }
            core.expire_replies();
        }
    }
    core.shutdown();
}
