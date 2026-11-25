# Server Outputs from the App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A paired station edits its server outputs in Preferences › Server (and `craftradio-cli outputs`); the server saves them in `state_dir` and applies them live.

**Architecture:** The server gets `state_dir`, a saved-outputs file per station that overrides the config at start, a keyed `PUT /api/station/outputs`, and a live swap of a station's `Outputs` (with the transcoder moved into a per-station slot so the swap can restart it). The pairing document gains `outputs_editable`. The app PUTs from the adapter's background worker with the same hardened ureq agent as pairing, then re-runs `server.pair` with the returned document.

**Tech Stack:** Rust, the server's hand-written HTTP layer, serde_json, ureq, egui.

**Spec:** `docs/superpowers/specs/2026-11-25-server-outputs-design.md`

## Global Constraints

- AGENTS.md is binding: Rust only; no `allow`; no panics in non-test code; commands for engine state (rule 6); no placeholder shown as success (rule 7); theme tokens only (rule 8); settings in Preferences (rule 9); keyboard, labels starting with the visible label, words not only colour (rule 10); invented names in fixtures (rule 11); tests in `tests/it/` of each crate; never weaken a test.
- Run cargo as `mise exec -- cargo …`; fmt, test and clippy `--all-targets -D warnings` for each crate changed.
- The station key is never a command argument, a setting, a log line, part of the view, or in a URL. The app sends it only to the paired address (the `server:<address>` keychain entry), with `proxy(None)` and no redirects, as `fetch` in `crates/ui-engine/src/pairing.rs` does.
- Saved file: `<state_dir>/<slug>.outputs.json`, a JSON array of `{"mount": str, "format": "opus"|"mp3", "kbps": u32}`; written to a temp file in `state_dir` then renamed.
- API: `PUT /api/station/outputs`, body ≤ 16 KiB, at most 8 outputs; `200` + pairing document, `400` reason, `403` `this server keeps no state_dir: outputs are set in its config`, `411` no Content-Length, `413` too large, `401`/`429` as `GET /api/station`. A refused change changes nothing on disk or live.
- Pairing document field `outputs_editable: bool`; `VERSION` stays 1; the app reads a missing field as false (`#[serde(default)]`).
- Words: `Saving…`, `Saved`, the server's reason on failure; read-only hint `This server keeps its outputs in its config file.`

## Review Focus

1. A swap while a source is live and listeners are on outputs: no deadlock with the source thread ending at the same moment; the pushed stream's listeners are untouched; a new listener on a new output gets audio without the app reconnecting.
2. A swap racing a source's start or end: never two transcoders for one station, never a transcoder left running after the source ended.
3. A refused PUT (bad kbps, duplicate mount, 9 outputs, bad JSON, wrong key) leaves the file and the live outputs exactly as they were; a wrong key counts toward the lockout.
4. Restart after a PUT serves the saved outputs, not the config's.
5. The app: Save while unpaired or mid-pairing is not offered; a failed Save keeps the edited rows so nothing typed is lost.

---

### Task 1: The server stores and swaps outputs

**Files:**
- Modify: `crates/server/src/config.rs` (`state_dir: Option<PathBuf>`; a pub fn to read/check a saved list: `pub fn read_saved_outputs(path) -> Result<Vec<Output>, ServerError>`, and `Output`/`OutputFormat` get `Serialize`; reuse `check_outputs` logic for a bare list plus the 8 limit), `crates/server/src/server.rs` (`StationMounts { pushed, cap, outputs: Mutex<Arc<Outputs>>, transcoder: Mutex<Option<Transcoder>> }`; the source puts its transcoder in the slot and finishes it from there before its guard drops; `fn set_outputs(&self, slug, Vec<Output>) -> Result<(), String>` on `Shared`: write the file, then under the slot lock finish the old transcoder, replace the `Arc<Outputs>`, and spawn a new transcoder if the pushed mount has a source; every reader of `mounts.outputs` and of `station.outputs` (stats, `output_at`, `Document::for_station`) reads the current set), `crates/server/src/pairing.rs` (`outputs_editable`, `for_station` takes the outputs list), `crates/server/src/http.rs` (reading a body by `Content-Length` up to a limit, if not there), the route `PUT /api/station/outputs`.
- At start: for each station with a saved file in `state_dir`, the file's list replaces the config's; a bad file stops `Server::start` with an error naming the file.
- Test: `crates/server/tests/it/outputs_api.rs` (new, registered in `main.rs`; reuse `support.rs` and the e2e helpers that push a real Ogg/MP3 source).

- [ ] **Step 1: Failing tests:**
  - `put_outputs_adds_an_output_live`: a live Opus push, PUT `[{"mount":"low.opus","format":"opus","kbps":32}]` → 200, the body's document lists it and `outputs_editable` is true; a listener on `/<slug>/low.opus` gets Ogg audio; the push was not reconnected (the source connection is the same one; the `/stream` listener got no end).
  - `put_outputs_removes_an_output`: config has one output with a listener; PUT `[]` → the listener's connection ends, the path 404s, stats no longer list it.
  - `put_outputs_is_saved_and_survives_a_restart`: PUT, stop, start with the same config and `state_dir` → the document lists the saved outputs, not the config's.
  - `a_bad_saved_file_stops_the_server_naming_it`.
  - `refused_puts_change_nothing`: each of bad kbps, duplicate mount, a reserved mount (`stream`), 9 outputs, bad JSON → 400 with a reason; the file's bytes and the live document are unchanged.
  - `put_without_state_dir_is_403`; `put_with_a_wrong_key_is_401_and_counts_toward_the_lockout`; `put_without_a_key_is_401`; `a_body_over_16_kib_is_413`; `no_content_length_is_411`.
  - `a_swap_while_the_source_ends_leaves_no_transcoder`: loop PUTs while the source disconnects and reconnects; afterwards `decoded_blocks` stops growing once no one listens, and a new source gets working outputs.
- [ ] **Step 2: Run, expect failures.**
- [ ] **Step 3: Implement.** Keep one lock order everywhere (slot, then nothing else that the source path takes in the other order). `GET /api/station` also gets `outputs_editable`.
- [ ] **Step 4: Run `mise exec -- cargo test -p craftradio-server` (and the server crate's package name, whichever `crates/server/Cargo.toml` says), clippy, fmt.**
- [ ] **Step 5: Commit** ("The server saves a station's outputs in state_dir and changes them live through PUT /api/station/outputs").

### Task 2: The app sends outputs (model, engine, adapter, CLI)

**Files:**
- Modify: `crates/model/src/server.rs` (`PairingDocument.outputs_editable` with `#[serde(default)]`; `PairingOutput` stays; a `pub struct OutputSpec { mount, format, kbps }` with serde for the PUT body), `crates/engine/src/pairing.rs` (store `server.outputs_editable`; `PAIRED` grows to 14), `crates/ui-engine/src/pairing.rs` (`pub fn put_outputs(address: &str, key: &str, outputs: &[OutputSpec]) -> Result<PairingDocument, PutError>` with the same agent config as `fetch`; `PutError` keeps the server's 400/403 text in words and maps 401 to key refused), `crates/ui-engine/src/bg.rs` (a job for it), `crates/ui-engine/src/lib.rs` or the pairing module (on success dispatch `server.pair { address, document }`; the view state below), `crates/ui-egui/src/view.rs` (`PairingView` gets `outputs: Vec<OutputRow { mount, format: OutputFormatView, kbps }>`, `outputs_editable: bool`, `outputs_state: OutputsState { Idle, Saving, Saved, Failed(String) }`; an `Action::SaveServerOutputs(Vec<OutputRow>)` or the crate's equivalent), `apps/radio-cli` (`outputs --data-dir` prints one line per output `<mount> <format> <kbps> <url>`; `outputs set --data-dir [<mount>=<format>:<kbps> …]`; refuses when not paired, `key refused` exits non-zero as `pair` does).
- Test: `crates/ui-engine/tests/it/pairing.rs` (or a new `server_outputs.rs`) against a real `craftradio-server` started in-process as the pairing tests do; `apps/radio-cli/tests/it/pairing.rs` (or `outputs.rs`); `crates/engine` test for the flag.

- [ ] **Step 1: Failing tests:** `saving_outputs_updates_the_settings_and_the_view` (Save → `Saving…` then `Saved`, `server.outputs` holds the new list); `a_refused_save_keeps_the_rows_and_says_why` (bad kbps → `Failed(reason)`, settings unchanged); `an_old_server_document_reads_as_not_editable`; `the_key_goes_only_to_the_paired_address` (a rewritten `server.address` setting → no request reaches the new address; reuse how the pairing tests prove it); CLI `outputs_set_then_list`, `outputs_set_unpaired_fails`.
- [ ] **Step 2–4:** implement; run model, engine, ui-engine, radio-cli tests, clippy, fmt.
- [ ] **Step 5: Commit** ("The app and craftradio-cli send a station's outputs to its server").

### Task 3: Preferences › Server shows and edits the outputs

**Files:**
- Modify: `crates/ui-egui/src/server_page.rs` (the Outputs section, only when `PairingView.paired` and state `Paired`; rows: format combo `Opus`/`MP3`, kbps field, mount field, `Remove`; then `Add output` and `Save`; `Save` off while `Saving…` or while nothing changed; the state line; read-only rows plus the hint when `!outputs_editable`), `crates/ui-egui/src/theme.rs` (any size it needs as a token).
- Test: `crates/ui-egui/tests/it/server_page.rs`.

- [ ] **Step 1: Failing tests:** `outputs_show_only_when_paired`; `add_edit_remove_then_save_sends_the_rows`; `save_is_off_while_saving`; `a_failed_save_shows_the_reason_and_keeps_the_rows`; `not_editable_shows_read_only_with_the_hint`; each control's accessible name starts with its visible label and the rows are reachable by Tab.
- [ ] **Step 2–4:** implement; run ui-egui tests, clippy, fmt. Look at it with the crate's preview/mock and attach a screenshot path to the report.
- [ ] **Step 5: Commit** ("Preferences › Server lists the station's outputs and saves changes to the server").

### Task 4: Packaging and docs

**Files:** `packaging/server/Dockerfile` (`install -d -o craftradio -g craftradio -m 0750 /var/lib/craftradio-server`, `VOLUME /var/lib/craftradio-server`), `packaging/server/craftradio-server.example.toml` (`state_dir` with a comment), `packaging/server/craftradio-server.service` (`StateDirectory=craftradio-server`), `packaging/server/README.md` (a "Change outputs from the app" section: `state_dir`, the Docker volume `-v craftradio-state:/var/lib/craftradio-server`, the saved file overrides the config, listeners of changed outputs reconnect; the Outputs section points to it), the app `README.md` (Preferences › Server › Outputs and `craftradio-cli outputs`), the spec's status line.

- [ ] Write them; check the Docker build still works if docker is available (`docker build -f packaging/server/Dockerfile .`), else say it was not run. Commit ("The server guide says how to let the app change outputs").
