//! The local control channel of a running station (spec 6.2, docs/control.md): the CLI and the
//! MCP server reach the station through it.
//!
//! It is craft-control's channel: JSON lines on a loopback TCP port, behind a token made at each
//! launch and kept in a file only this user can read, so no other user on the machine can drive
//! the station. Every request runs an engine command on the playout, as a panel does:
//!
//! - `station.commands`: every command with its id, tier, summary and argument schema.
//! - `station.run`: one command, `{"id": "schedule.add", "args": {...}}`; the result is its
//!   outcome.

use std::ffi::OsString;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use craft_control::auth::{self, App};
use craft_control::server::{Hub, Protocol, Stop, Summary};
use craft_control::{ControlRequest, Effect};
use radio_engine::{Command, Tier, registry};
use radio_playout::CommandSender;
use serde_json::{Value, json};

pub use craft_control::client::{CallError, Connection, Request};

/// The station's control channel: its token files live in `<data folder>/radio-station-manager/
/// control`, and `CRAFTRADIO_CONTROL_DIR`, `CRAFTRADIO_CONTROL_TOKEN` and
/// `CRAFTRADIO_CONTROL_PORT` change them.
pub const APP: App = App {
    name: "CraftRadio",
    data_dir,
    env,
    utc,
    default_port: DEFAULT_PORT,
};

/// The port tried first, below the ephemeral ranges of every platform.
pub const DEFAULT_PORT: u16 = 17_470;

/// The methods this station answers.
pub const METHODS: [&str; 3] = ["station.commands", "station.run", "ui.screenshot"];

/// The longest request line accepted.
const MAX_LINE: usize = 1 << 20;
/// How long a command may take on the playout before the reply says it did not answer.
const REPLY_WAIT: Duration = Duration::from_secs(10);
/// How often the request thread looks whether the channel is closing.
const POLL: Duration = Duration::from_millis(100);

fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("radio-station-manager"))
}

fn env(name: &str) -> Option<OsString> {
    std::env::var_os(format!("CRAFTRADIO_{name}"))
}

fn utc(secs: u64) -> String {
    i64::try_from(secs)
        .ok()
        .and_then(|s| jiff::Timestamp::from_second(s).ok())
        .map(|t| t.to_string())
        .unwrap_or_else(|| secs.to_string())
}

/// What the channel's requests mean to craft-control's server.
struct Station;

impl Protocol for Station {
    fn summary(&self, method: &str, params: &Value) -> Summary {
        let id = params.get("id").and_then(Value::as_str);
        let spec = id.and_then(|id| registry().into_iter().find(|s| s.id == id));
        let effect = match (method, &spec) {
            ("station.commands" | "ui.screenshot", _) => Effect::ReadsOnly,
            (_, Some(s)) if s.tier == Tier::ReadOnly => Effect::ReadsOnly,
            _ => Effect::Changes,
        };
        Summary {
            what: match id {
                Some(id) => format!("`{method}` ({id})"),
                None => format!("`{method}`"),
            },
            effect,
            command: spec.map(|s| s.id),
            steps: None,
            long: false,
        }
    }

    fn document(&self) -> &'static str {
        "station"
    }

    fn change_event(&self) -> &'static str {
        "station.changed"
    }

    fn revision_method(&self) -> &'static str {
        "station.revision"
    }

    fn docs(&self) -> &'static str {
        "docs/control.md"
    }

    fn capabilities(&self) -> Value {
        json!({ "methods": METHODS })
    }
}

/// A request only the station's window can answer (`ui.screenshot`), handed to the UI thread.
/// Its `reply` is the whole answer: `{"ok": true, "result": …}` or `{"ok": false, "error": …}`.
pub struct WindowRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<Value>,
}

/// The window attached to the channel, and how to wake it for a request.
struct WindowLink {
    tx: Sender<WindowRequest>,
    wake: Box<dyn Fn() + Send + Sync>,
}

/// The attached window, shared by the channel and its request thread.
type Window = Arc<Mutex<Option<WindowLink>>>;

/// How long the window has to answer, a frame or a few.
const WINDOW_WAIT: Duration = Duration::from_secs(10);

/// Hands `method` to the attached window and waits for its answer.
fn ask_window(window: &Window, method: &str, params: Value) -> Value {
    let refuse = |error: &str| json!({ "ok": false, "error": error });
    let (reply, answer) = mpsc::channel();
    let sent = window.lock().ok().and_then(|link| {
        let link = link.as_ref()?;
        link.tx
            .send(WindowRequest {
                method: method.to_owned(),
                params,
                reply,
            })
            .ok()?;
        (link.wake)();
        Some(())
    });
    if sent.is_none() {
        return refuse(
            "no window: this station runs without one (--headless), so there is nothing to capture",
        );
    }
    answer
        .recv_timeout(WINDOW_WAIT)
        .unwrap_or_else(|_| refuse("the window did not answer"))
}

/// The answer to one request: `{"ok": true, "result": …}` or `{"ok": false, "error": …}`.
fn answer(sender: &CommandSender, window: &Window, method: &str, params: Value) -> Value {
    match method {
        "ui.screenshot" => ask_window(window, method, params),
        "station.commands" => {
            let list: Vec<Value> = registry()
                .into_iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "summary": s.summary,
                        "tier": s.tier,
                        "args_schema": s.args_schema,
                    })
                })
                .collect();
            json!({ "ok": true, "result": list })
        }
        "station.run" => match run(sender, params) {
            Ok(outcome) => json!({ "ok": true, "result": outcome }),
            Err(error) => json!({ "ok": false, "error": error }),
        },
        other => json!({
            "ok": false,
            "error": format!(
                "unknown method `{other}`: this station answers {}",
                METHODS.join(", ")
            ),
        }),
    }
}

/// Runs one command on the playout and returns its outcome.
fn run(sender: &CommandSender, params: Value) -> Result<Value, String> {
    let command: Command =
        serde_json::from_value(params).map_err(|e| format!("cannot read the command: {e}"))?;
    let reply = sender
        .send_with_reply(command)
        .map_err(|e| format!("the playout stopped: {e}"))?;
    let outcome = reply
        .recv_timeout(REPLY_WAIT)
        .map_err(|_| "the playout did not answer the command".to_owned())?
        .map_err(|e| e.to_string())?;
    serde_json::to_value(outcome).map_err(|e| format!("cannot write the outcome: {e}"))
}

/// Answers requests until the channel closes. It owns the playout's sender, so the playout
/// can stop once this thread ends.
fn answer_requests(
    sender: CommandSender,
    window: Window,
    requests: Receiver<ControlRequest>,
    closing: &AtomicBool,
) {
    while !closing.load(Ordering::SeqCst) {
        match requests.recv_timeout(POLL) {
            Ok(req) => {
                let reply = answer(&sender, &window, &req.method, req.params);
                let _ = req.reply.send(reply);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// A running control channel. Drop it before the playout: it holds one of the playout's
/// senders until it stops.
pub struct Control {
    port: u16,
    token: String,
    stop: Option<Stop>,
    closing: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    window: Window,
    /// The token and discovery files to remove at the end, when [`start`] wrote them.
    files: Option<PathBuf>,
}

impl Control {
    /// The loopback port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Where its token file is, when it wrote one.
    pub fn token_file(&self) -> Option<&PathBuf> {
        self.files.as_ref()
    }

    /// Attaches the station's window: `ui.*` requests arrive on the returned receiver, and
    /// `wake` makes the window look at them now. Without a window, they are refused.
    pub fn attach_window(
        &self,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Receiver<WindowRequest> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut link) = self.window.lock() {
            *link = Some(WindowLink {
                tx,
                wake: Box::new(wake),
            });
        }
        rx
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.stop();
        }
        self.closing.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(path) = self.files.take() {
            auth::remove_token_file(&path, &self.token);
            APP.remove_current_port(self.port, &self.token);
        }
    }
}

/// Serves `listener` (loopback only) with `token`, running each request on the playout of
/// `sender`. It writes no file: [`start`] does that.
pub fn serve(
    sender: CommandSender,
    listener: TcpListener,
    token: String,
) -> std::io::Result<Control> {
    let port = listener.local_addr()?.port();
    let (tx, rx): (Sender<ControlRequest>, Receiver<ControlRequest>) = mpsc::channel();
    let hub = Arc::new(Hub::new(Station, || {}, MAX_LINE));
    let stop = hub.listen_stoppable(listener, token.clone(), tx, false)?;
    let closing = Arc::new(AtomicBool::new(false));
    let window: Window = Arc::default();
    let thread = {
        let (closing, window) = (Arc::clone(&closing), Arc::clone(&window));
        std::thread::Builder::new()
            .name("control-requests".into())
            .spawn(move || answer_requests(sender, window, rx, &closing))?
    };
    Ok(Control {
        port,
        token,
        stop: Some(stop),
        closing,
        thread: Some(thread),
        window,
        files: None,
    })
}

/// Starts the channel for a running station: on `port`, else on `CRAFTRADIO_CONTROL_PORT`, else
/// on [`DEFAULT_PORT`] or the next free one. It writes the token file and the discovery file
/// that clients read, and removes them when dropped.
pub fn start(sender: CommandSender, port: Option<u16>) -> Result<Control, String> {
    let port = port.or_else(|| {
        env("CONTROL_PORT")
            .and_then(|p| p.into_string().ok())
            .and_then(|p| p.trim().parse().ok())
    });
    let listener = match port {
        Some(p) => auth::bind_with_fallback(p, 1),
        None => auth::bind_with_fallback(DEFAULT_PORT, auth::PORT_TRIES),
    }
    .map_err(|e| format!("cannot listen on the loopback: {e}"))?;
    let token = APP
        .env_token()
        .or_else(craft_control::server::new_token)
        .ok_or("cannot make a token: the system gave no random bytes")?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let path = APP
        .token_path(port)
        .ok_or("the platform has no data folder for the token")?;
    APP.write_token_file(&path, &token)
        .map_err(|e| format!("cannot write the token file {}: {e}", path.display()))?;
    let mut control = serve(sender, listener, token).map_err(|e| e.to_string())?;
    control.files = Some(path);
    APP.write_current_port(control.port)
        .map_err(|e| format!("cannot write the discovery file: {e}"))?;
    Ok(control)
}

/// A connection to the running station: on `port`, else the one in the discovery file, else
/// the only station with a token file. The token is `token`, else `CRAFTRADIO_CONTROL_TOKEN`,
/// else the token file.
pub fn connect(port: Option<u16>, token: Option<&str>) -> Result<Connection, String> {
    let port = match port.or_else(|| APP.read_current_port()) {
        Some(p) => p,
        None => lone_station()?,
    };
    Ok(Connection::new(APP, &format!("127.0.0.1:{port}"), token).with_document("station"))
}

/// The port of the only station with a token file. The discovery file names the station
/// started last; when that one stops first, an earlier one may still run.
fn lone_station() -> Result<u16, String> {
    let dir = APP
        .current_port_path()
        .and_then(|p| p.parent().map(PathBuf::from))
        .ok_or("the platform has no data folder: pass --port")?;
    let mut ports: Vec<u16> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".token")?.parse().ok()
        })
        .collect();
    ports.sort_unstable();
    match ports.as_slice() {
        [port] => Ok(*port),
        [] => Err(format!(
            "no running station: start craftradio, or pass --port (no token file in {})",
            dir.display()
        )),
        many => Err(format!(
            "several stations run, on ports {}: pass --port",
            many.iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}
