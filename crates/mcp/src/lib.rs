//! The MCP server of the station (spec 6.2): every engine command as a tool, run on the running
//! station through its control channel (docs/control.md). It speaks newline-delimited JSON-RPC
//! on stdio through craft-mcp-server; only protocol lines go to stdout.

use craft_mcp_server::{Effect, Info, Options, Server, Tools, text_result, tool};
use radio_control::{Connection, Request};
use radio_engine::{CommandSpec, Tier, registry};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// Longest accepted request line.
const MAX_LINE: usize = 1 << 20;

const INSTRUCTIONS: &str = "CraftRadio runs a radio station: four decks, a main playlist plus \
named playlists woven in by rules, fixed-time events, a cartwall, and local and Icecast output. \
Each tool is one engine command and runs on the station that is on air now, as a panel of the \
app would. Tools marked on-air change what the station plays: confirm with the person first. \
Read the state with station_get; history_list and history_export_csv read the as-played log.";

/// Where tool calls go: the running station.
pub trait Station {
    /// Runs control method `method` with `params`, and returns its result.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
}

/// The station running on this machine, reached through its control channel. It connects at
/// the first call, and again after the station restarts.
pub struct Running {
    port: Option<u16>,
    token: Option<String>,
    conn: Option<Connection>,
}

impl Running {
    /// The station on `port` (else the one started last), with `token` (else the token file).
    pub fn new(port: Option<u16>, token: Option<String>) -> Self {
        Running {
            port,
            token,
            conn: None,
        }
    }
}

impl Station for Running {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if self.conn.is_none() {
            self.conn = Some(radio_control::connect(self.port, self.token.as_deref())?);
        }
        let conn = self.conn.as_mut().ok_or("no connection")?;
        let result = conn.call(Request::new(method, params));
        result.map_err(|e| {
            // Found again at the next call, when the station has restarted on another port.
            self.conn = None;
            format!(
                "{} (is craftradio running with its control channel?)",
                e.message()
            )
        })
    }
}

/// The MCP name of command `id`: `schedule.add` is `schedule_add`.
pub fn tool_name(id: &str) -> String {
    id.replace('.', "_")
}

/// The tool definition of one engine command.
fn definition(spec: &CommandSpec) -> Value {
    let mut schema = spec.args_schema.as_value().clone();
    if schema.get("type").is_none() {
        schema["type"] = json!("object");
    }
    if schema.get("properties").is_none() {
        schema["properties"] = json!({});
    }
    let (effect, desc) = match spec.tier {
        Tier::ReadOnly => (Effect::ReadOnly, format!("{}. Read-only.", spec.summary)),
        // On air: it can stop, replace or delete what plays, so clients ask before it runs.
        Tier::OnAir => (
            Effect::Replaces,
            format!(
                "{}. On air: changes the running station (`{}`).",
                spec.summary, spec.id
            ),
        ),
    };
    tool(&tool_name(spec.id), spec.summary, &desc, schema, effect)
}

/// The station's commands as MCP tools.
pub struct StationTools<S: Station> {
    station: S,
    specs: Vec<CommandSpec>,
}

impl<S: Station> StationTools<S> {
    pub fn new(station: S) -> Self {
        StationTools {
            station,
            specs: registry(),
        }
    }

    /// The text of a command's outcome, with what a person must know of it.
    fn report(outcome: &Value) -> String {
        let mut text =
            serde_json::to_string_pretty(outcome).unwrap_or_else(|_| outcome.to_string());
        if let Some(note) = outcome.get("note").and_then(Value::as_str) {
            text.push_str(&format!("\nNote: {note}"));
        }
        let required = outcome["audio_required"].as_bool() == Some(true);
        let applied = outcome["applied_to_audio"].as_bool() == Some(true);
        if required && !applied {
            text.push_str("\nNote: the mixer did not confirm this command; check the decks.");
        }
        text
    }
}

impl<S: Station> Tools for StationTools<S> {
    fn instructions(&self) -> String {
        INSTRUCTIONS.to_owned()
    }

    fn list(&mut self) -> Vec<Value> {
        self.specs.iter().map(definition).collect()
    }

    fn call(&mut self, name: &str, args: &Value) -> Value {
        let Some(spec) = self.specs.iter().find(|s| tool_name(s.id) == name) else {
            return text_result(format!("unknown tool `{name}`"), true);
        };
        let empty = args.as_object().is_none_or(|a| a.is_empty());
        let takes_args = spec.args_schema.as_value()["properties"]
            .as_object()
            .is_some_and(|p| !p.is_empty());
        // A command without arguments is written without `args`.
        let command = if empty && !takes_args {
            json!({ "id": spec.id })
        } else {
            json!({ "id": spec.id, "args": args })
        };
        match self.station.call("station.run", command) {
            Ok(outcome) => text_result(Self::report(&outcome), false),
            Err(error) => text_result(error, true),
        }
    }
}

/// The MCP server over `station`.
pub fn server<S: Station>(station: S) -> Server<StationTools<S>> {
    let info = Info {
        name: "craftradio",
        title: "CraftRadio",
        version: env!("CARGO_PKG_VERSION"),
        max_line: MAX_LINE,
    };
    Server::new(info, StationTools::new(station)).with_options(Options {
        tool_errors_as_results: true,
        ..Options::default()
    })
}

/// Serves MCP on `input` and `output` until `input` closes.
pub fn serve<S: Station>(
    station: S,
    input: impl BufRead,
    output: impl Write,
) -> std::io::Result<()> {
    server(station).serve(input, output)
}
