# Station Server Phase 2: Pairing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On a fresh station, a server address plus the station key gives a working Icecast output to `craftradio-server`, with no other field touched and no restart.

**Architecture:** The server answers `GET /api/station` with a versioned JSON pairing document for the station whose key is presented. The app keeps the key in the keychain; a background job reads it, fetches the document over HTTPS, and runs the engine command `server.pair { address, document }`, which writes ordinary `icecast.*`, `webrtc.public_stream` and `server.*` settings. Playout now always holds an Icecast tap, so a changed Icecast setting (from pairing or by hand) applies at once. A Preferences page "Server" and a CLI `pair` / `unpair` drive it.

**Tech Stack:** Rust; `ureq` 3 with rustls (already in the workspace) for the fetch; `keyring` through the existing `SecretStore`; serde_json.

**Spec:** `docs/superpowers/specs/2026-11-24-station-server-design.md` §5 (`GET /api/station`), §6.1 (pairing), §7 phase 2.

## Global Constraints

- AGENTS.md is binding: rule 6 (every change is an engine command; panels, CLI and MCP all reach it), rule 7 (no placeholder shown as success), rule 8 (theme tokens), rule 9 (every setting lives in Preferences; the Server page is a page of its rail), rule 10, rule 11 (no secrets in code, logs or fixtures; invented names), rule 12 (licences), no `allow`, no panics in non-test code, tests in `tests/it/`, crates depend only on lower layers (server is L2 and never depends on the app; the app may dev-depend on `radio-server` for tests).
- **The key is never a command argument**, never in settings, logs, the station view, MCP output or a URL. It lives in the keychain only. `server.pair` carries the address and the fetched document (which holds no key).
- The station key is sent to the server as the Basic password (any user), as the Icecast push already does. A wrong key counts toward the server's existing lockout.
- The pairing document is JSON with `"version": 1`.
- Pairing writes `icecast.*` (host, port, mount, user, TLS, public URL; the key stored as that mount's password in the keychain), `webrtc.public_stream = <public_url>/<slug>/stream`, and a `server.*` group (address, slug, and what the document offered). Signalling, feed and guest fields are phases 3–5: the document leaves them out and the app writes nothing for them.
- The app fetches the document again at each start. Settings changed by hand after pairing are overwritten on the next fetch; the Server page says so. `server.unpair` restores the defaults for the settings pairing wrote and removes the key from the keychain.
- States, in words (rule 7): `Not paired`; `Paired` (reachable, key accepted — the only success state); `Paired — server unreachable: <reason>`; `Paired — key refused`. The headless app prints the same.
- Call (this plan): Icecast settings apply without a restart, because the done check says "no other field touched" and a restart would be one more step. This also ends "restart to apply" for Icecast settings typed by hand.

## Review Focus

1. A server that is down at start: the app still starts, keeps the last paired settings, and says `Paired — server unreachable`, without blocking the UI thread.
2. A key that is wrong or was rotated on the server: `Paired — key refused`, no retry loop that walks the server's lockout.
3. An address typed with a path, a trailing slash, `http://` to a remote host, or no scheme: one clear rule (scheme required; `http` only for loopback) and a message in words.
4. Unpair while Icecast is connected: the connection closes and the strip says it is not set up.
5. A document of a newer `version`: refused in words, settings untouched.

---

### Task 1: The server answers `GET /api/station`

**Files:**
- Create: `crates/server/src/pairing.rs` (the document type and its building)
- Modify: `crates/server/src/server.rs` (route, auth by key across stations), `crates/server/src/lib.rs`
- Test: `crates/server/tests/it/pairing.rs`

**Interfaces:**
- Produces: `radio_server::pairing::Document { version: u32, slug: String, name: String, public_url: String, icecast: IcecastTarget { host: String, port: u16, tls: bool, mount: String, user: String }, stream_url: String, outputs: Vec<OutputEntry { mount: String, format: String, kbps: u32, url: String }> }` — serde `Serialize + Deserialize`, so the app's tests can read it. `icecast` is derived from `public_url` (scheme gives TLS and default port), mount `/<slug>/stream`, user `source`.

- [ ] **Step 1: Failing tests:** `the_station_key_gets_its_pairing_document` (two stations; each key gets its own slug; `version == 1`; `icecast.mount == "/<slug>/stream"`; `stream_url` ends with `/<slug>/stream`; outputs listed in config order); `a_wrong_key_is_refused_and_counts_toward_the_lockout` (401, and after the lockout's limit, 429); `no_key_is_refused_without_counting`; `the_document_never_contains_the_key_or_its_hash`.
- [ ] **Step 2: Run, expect failures.**
- [ ] **Step 3: Implement.** The station is the one whose `key_hash` matches the presented key; check every station's hash (no early exit) so timing does not tell which slug exists. Reuse `authorized`'s lockout and challenge logic; split it so it can run without a known station.
- [ ] **Step 4: Run `radio-server` tests, clippy, expect pass.**
- [ ] **Step 5: Commit** ("The server hands a station its pairing document at /api/station").

### Task 2: Icecast settings apply while the app runs

**Files:**
- Modify: `crates/stream/src/output.rs` (`IcecastHandle::reconfigure(Option<IcecastConfig>)`; a worker started with no config waits, status `Stopped`), `crates/playout/src/core.rs` and `lib.rs` (always open an Icecast tap; start the worker with `cfg.icecast` or none; a `Playout`/`CommandSender` way to hand a new `Option<IcecastConfig>`), `crates/ui-engine/src/icecast.rs` and `lib.rs` (when the Icecast settings or the keychain password change, build the config with `icecast_config` and hand it to playout), `apps/radio/src/main.rs` (unchanged start path still works)
- Test: `crates/stream/tests/it/off_air.rs` (or a new `reconfigure.rs`), `crates/playout/tests/it/streams.rs`, `crates/ui-engine/tests/it/icecast.rs`

**Interfaces:**
- Consumes: the off-air worker (`Control`, `settle`, `set_on_air`).
- Produces: `IcecastHandle::reconfigure(&self, cfg: Option<IcecastConfig>)`; the playout entry the adapter calls (name it in the report).

- [ ] **Step 1: Failing tests:** `reconfigure_moves_the_source_to_the_new_mount` (server A sees the source leave, server B sees it arrive); `reconfigure_to_none_closes_the_source` (status `Stopped`, strip says not set up); `a_station_with_no_icecast_at_start_connects_once_set_up` (playout started without Icecast; settings and a keychain password added; a test server sees the source); `off_air_still_wins_after_reconfigure`.
- [ ] **Step 2–4:** implement, run stream, playout and ui-engine tests and clippy.
- [ ] **Step 5: Commit** ("Icecast settings take effect while the app runs").

### Task 3: The engine commands `server.pair` and `server.unpair`

**Files:**
- Modify: `crates/model/src/` (a `server` module: `SERVER_PART = "server"`, `PairingDocument` mirroring Task 1's `Document` field for field, deserialize-only, `version` checked), `crates/engine/src/command.rs` (two commands, tier as `settings.set`), the engine's command handling
- Test: `crates/engine/tests/it/` (a new `pairing.rs`)

**Interfaces:**
- Produces: `Command::ServerPair { address: String, document: PairingDocument }` (`"server.pair"`), `Command::ServerUnpair` (`"server.unpair"`).

- [ ] **Step 1: Failing tests:** `pairing_writes_icecast_webrtc_and_server_settings` (exact keys from the Global Constraints); `pairing_again_overwrites_hand_edits`; `unpair_restores_the_defaults_pairing_wrote_and_leaves_other_settings`; `a_newer_document_version_is_refused_and_changes_nothing`; `the_commands_are_listed_for_the_cli_and_mcp` (they appear in the command registry with summaries).
- [ ] **Step 2–4:** implement, run model and engine tests and clippy.
- [ ] **Step 5: Commit** ("The engine pairs a station with a server and unpairs it").

### Task 4: The adapter pairs: keychain, fetch, refetch at start, states

**Files:**
- Modify: `crates/ui-engine/src/icecast.rs` (`SecretStore::delete`; `KeyringStore` and `MemoryStore` implement it), `crates/ui-engine/src/bg.rs` (a `Job::Pair { address, key: Option<String> }` that stores a given key, reads it back, fetches `<address>/api/station` with ureq + rustls, and returns the document or a reason), a new `crates/ui-engine/src/pairing.rs` (the state machine and `PairingView`), `crates/ui-engine/src/lib.rs` (refetch at start when `server.address` is set; dispatch `server.pair`; unpair removes the key), `crates/ui-egui/src/view.rs` (`PairingView { address: String, state: PairState }`, `PairState::{NotPaired, Pairing, Paired, Unreachable(String), KeyRefused}`), `crates/ui-engine/Cargo.toml` (`ureq` from the workspace, dev-dependency `radio-server`)
- Test: `crates/ui-engine/tests/it/pairing.rs` (against a real `radio-server` on loopback, plain HTTP)

**Interfaces:**
- Consumes: Task 1's server, Task 2's live Icecast apply, Task 3's commands.
- Produces: `StationView::pairing: PairingView`; an `Edit`/action the GUI sends: `Pair { address, key }` (the key goes only to the keychain job) and `Unpair`.

- [ ] **Step 1: Failing tests:** `address_and_key_pair_and_icecast_connects` (the phase 2 done check: fresh station, pair, the server sees the source, state `Paired`, no other field set by the test); `a_wrong_key_says_key_refused_and_does_not_retry`; `a_server_that_is_down_says_unreachable_and_keeps_settings`; `the_app_refetches_at_start_and_overwrites_hand_edits`; `unpair_removes_the_key_and_closes_icecast`; `an_address_without_a_scheme_or_http_to_a_remote_host_is_refused_in_words`; `the_key_never_reaches_settings_or_the_snapshot`.
- [ ] **Step 2–4:** implement, run ui-engine tests and clippy; `cargo check --workspace --all-targets`.
- [ ] **Step 5: Commit** ("The app pairs with a station server from its address and key").

### Task 5: The Preferences page "Server"

**Files:**
- Modify: `crates/ui-egui/src/preferences.rs` (a `Page::Server` in the rail, after WebRTC), a new `crates/ui-egui/src/server_page.rs` (address field, key field masked, Connect / Disconnect, the state in words, the note that pairing overwrites hand edits of Icecast fields), `crates/ui-egui/src/mock/` (the preview fills `PairingView`; its Connect says it needs the real app, rule 7), `crates/ui-egui/src/theme.rs` (tokens it needs)
- Test: `crates/ui-egui/tests/it/server_page.rs`

- [ ] **Step 1: Failing tests:** `connect_sends_pair_with_the_typed_address_and_key`; `the_key_field_is_masked_and_cleared_after_connect`; `each_state_shows_in_words_and_only_paired_looks_ok`; `disconnect_sends_unpair`; `the_page_is_reachable_by_keyboard_and_names_start_with_labels`.
- [ ] **Step 2–4:** implement, run ui-egui tests and clippy.
- [ ] **Step 5: Commit** ("Preferences has a Server page: address, key, Connect").

### Task 6: The CLI, the headless line and the docs

**Files:**
- Modify: `apps/radio-cli/src/main.rs` (`pair <address> --data-dir <dir>` reads the key from standard input or `CRAFTRADIO_SERVER_KEY`, stores it with `KeyringStore`, fetches, runs `server.pair`; `unpair --data-dir <dir>`), `apps/radio/src/main.rs` (headless prints the pairing state at start and on change), `packaging/server/README.md` (pairing replaces "Set the app's Icecast output by hand" as the main route; the hand route stays as a fallback), `README.md`, the spec's status line
- Test: `apps/radio-cli/tests/it/` and `apps/radio/tests/it/main.rs`

- [ ] **Step 1: Failing tests:** `cli_pair_reads_the_key_from_stdin_and_never_prints_it`; `cli_pair_with_a_wrong_key_exits_nonzero_with_key_refused`; `headless_prints_the_pairing_state`.
- [ ] **Step 2–4:** implement, run the app and CLI tests and clippy.
- [ ] **Step 5: Commit** ("craftradio-cli pairs a station, and the docs say how").
