//! The MCP server over a real station: a playout on an in-memory station, its control channel,
//! and JSON-RPC lines as a client sends them on stdio.

use std::net::TcpListener;
use std::sync::Arc;

use radio_control::Control;
use radio_engine::{Engine, Tier, registry};
use radio_mcp::{Running, serve, tool_name};
use radio_playout::{ManualClock, OutputChoice, Playout, PlayoutConfig};
use serde_json::{Value, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

fn station() -> (Playout, Control) {
    let engine = Engine::open_in_memory().expect("an in-memory station");
    let clock = ManualClock::new("2026-11-21T13:00:00Z".parse().expect("a timestamp"));
    let playout = Playout::start(
        engine,
        PlayoutConfig::new(Arc::new(clock), OutputChoice::Null),
    )
    .expect("playout starts");
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let sender = playout.command_sender().expect("a running playout");
    let control =
        radio_control::serve(sender, listener, TOKEN.to_owned()).expect("the channel serves");
    (playout, control)
}

/// The replies of the server to `requests`, after the MCP handshake, by request id.
fn session(station: Running, requests: &[Value]) -> Vec<Value> {
    let mut lines = vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ];
    lines.extend_from_slice(requests);
    let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let mut output = Vec::new();
    serve(station, input.as_bytes(), &mut output).expect("the server runs");
    String::from_utf8(output)
        .expect("UTF-8 output")
        .lines()
        .map(|l| serde_json::from_str(l).expect("every output line is JSON"))
        .filter(|r: &Value| r["id"] != 0)
        .collect()
}

fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": name, "arguments": args}})
}

fn text(reply: &Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn every_engine_command_is_a_tool_a_client_accepts() {
    let replies = session(
        Running::new(Some(1), Some(TOKEN.into())),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})],
    );
    let tools = replies[0]["result"]["tools"]
        .as_array()
        .expect("a tool list");
    let specs = registry();
    assert_eq!(tools.len(), specs.len(), "one tool per command");
    let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for name in &names {
        assert!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{name} is a tool name every client accepts"
        );
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        specs.len(),
        "no two commands share a tool name"
    );
    for spec in &specs {
        let t = tools
            .iter()
            .find(|t| t["name"] == tool_name(spec.id))
            .expect("listed");
        assert_eq!(t["inputSchema"]["type"], "object", "{}", spec.id);
        assert_eq!(
            t["annotations"]["readOnlyHint"] == true,
            spec.tier == Tier::ReadOnly,
            "{} says whether it changes the station",
            spec.id
        );
    }
}

#[test]
fn a_tool_call_runs_on_the_running_station() {
    let (_playout, control) = station();
    let running = Running::new(Some(control.port()), Some(TOKEN.into()));
    let replies = session(
        running,
        &[
            call(1, "playlist_create", json!({"name": "Night Owls"})),
            call(2, "station_get", json!({})),
        ],
    );
    assert_ne!(
        replies[0]["result"]["isError"],
        true,
        "{}",
        text(&replies[0])
    );
    assert!(
        text(&replies[1]).contains("Night Owls"),
        "the station reads back the playlist the first call made"
    );
}

#[test]
fn without_a_running_station_a_call_is_an_error_result() {
    let free = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let replies = session(
        Running::new(Some(free), Some(TOKEN.into())),
        &[call(1, "station_get", json!({}))],
    );
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(text(&replies[0]).contains("craftradio running"));
}
