# Station Server, Phase 1: Server Core — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `craftradio-server` binary that takes the app's existing Icecast push for several
stations, each behind its own key, and serves each as a public HTTP stream with titles.

**Architecture:** A new library crate `crates/server` (package `radio-server`) holds all
logic; `apps/radio-server` is a thin `clap` binary around it. One thread per connection, no
async runtime. A source connection feeds a `Mount`, which splits the bytes at Ogg page or MP3
frame boundaries and hands each chunk to every listener through a bounded queue.

**Tech Stack:** Rust 2024, `std::net`, `rustls` (optional TLS), `ring` (SHA-256, random),
`toml` + `serde` (config), `serde_json` (stats), `base64`, `percent-encoding`, `clap`,
`thiserror`. All are workspace dependencies already; this phase adds no new crate.

**Spec:** `docs/superpowers/specs/2026-11-24-station-server-design.md` (§5.1, §5.2, §7 phase 1).
Phases 1b–5 get their own plans; nothing here builds outputs, pairing, signalling, the
guest page or the relay.

## Global Constraints

- Run cargo as `mise exec -- cargo …`. For each task: `cargo fmt --all`,
  `cargo test -p radio-server`, `cargo clippy -p radio-server --all-targets -- -D warnings`
  (and `-p craftradio-server` once it exists).
- `unsafe_code = "forbid"`. Non-test code does not panic: no `unwrap`, `expect`, indexing
  that can fail, or `panic!` outside tests. Fix lints; never `allow` them.
- No new dependency. If one seems needed, stop and ask; it must pass `AGENTS.md` rule 12.
- No handwritten JavaScript in this phase.
- Integration tests are one binary: `crates/server/tests/it/main.rs` with modules beside it.
  Tests assert behaviour and relations, not constants (rules 14–15). Never weaken or
  `#[ignore]` a test to pass (rule 17).
- Fixture names, titles and artists are invented ("Morning Station", "Cloud Signal"). No
  real keys, hosts or songs in code, logs or fixtures (rule 11). Logs never contain a key.
- Nothing shows success for something that isn't working (rule 7): a refused source gets a
  status line, never silence.
- Commits are one change each, in plain English saying what changed for the person
  running the server, ending with
  `Co-Authored-By: Claude Fable 5.2 <noreply@anthropic.com>`. Work on the current branch;
  do not push.
- Spec values, verbatim: lockout is **5** wrong keys from one address within **60 s** →
  that address gets HTTP **429** on all keyed endpoints for **300 s**. Default
  `max_stream_listeners` is **200**. Over the cap the stream answers **503**. A second
  source on a live mount is refused. The mount a station pushes to is `/<slug>/stream`.

## Review Focus

1. **A listener that stops reading** (a phone that lost signal): it must be dropped when its
   queue fills, and must never slow the source or other listeners. → Task 4.
2. **A source that connects and goes silent** (the app's computer sleeps): the mount must
   end after the read timeout and free the mount for a reconnect, not stay "live" forever.
   → Task 5.
3. **A source that reconnects with another format** (Opus changed to MP3 in the app):
   listeners still attached must be ended, not fed MP3 bytes in an Ogg response. → Task 5.
4. **A client that sends headers slowly or without end**: the connection closes at the
   deadline or the size limit, and connections beyond the cap are refused; the server keeps
   serving others. → Tasks 3 and 8.
5. **`X-Forwarded-For` from a client that is not the trusted proxy**: it must be ignored,
   or anyone could lock out another address or dodge their own lockout. → Task 3.
6. **A title with quotes, control characters or invalid UTF-8**: `/status-json.xsl` must
   stay valid JSON. → Task 7.

## File Structure

```
crates/server/
  Cargo.toml
  src/lib.rs        re-exports, ServerError
  src/config.rs     Config, Station, parsing and validation
  src/key.rs        new_key, hash_key, key_matches
  src/lockout.rs    Lockout (wrong-key counting per address)
  src/http.rs       Request, read_request, auth and client-address helpers, responses
  src/framing.rs    Format, Framer (Ogg pages / MP3 frames, header pages, tags)
  src/mount.rs      Mount, SourceGuard, Listener, MountStats
  src/stats.rs      status_json
  src/server.rs     Server, ServerHandle, routing, connection threads, TLS
  tests/it/main.rs  + config.rs, lockout.rs, http.rs, framing.rs, mount.rs, server.rs, e2e.rs
apps/radio-server/
  Cargo.toml        package craftradio-server, bin craftradio-server
  src/main.rs       `run --config <path>` and `key`
packaging/server/
  craftradio-server.example.toml, Caddyfile, craftradio-server.service, README.md
```

---

### Task 1: Crate, config file and keys

**Files:**
- Create: `crates/server/Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/key.rs`
- Test: `crates/server/tests/it/main.rs`, `tests/it/config.rs`

**Interfaces:**
- Produces:
  - `pub enum ServerError { Config(String), Io(std::io::Error), Tls(String) }` (`thiserror`).
  - `pub struct Config { pub listen: SocketAddr, pub public_url: String, pub trusted_proxy: Option<IpAddr>, pub tls_cert: Option<PathBuf>, pub tls_key: Option<PathBuf>, pub stations: Vec<Station> }` — TOML key for `stations` is `station`; `#[serde(deny_unknown_fields)]`.
  - `pub struct Station { pub slug: String, pub name: String, pub key_hash: String, pub max_stream_listeners: usize }` (default 200).
  - `impl Config { pub fn parse(text: &str) -> Result<Self, ServerError>; pub fn load(path: &Path) -> Result<Self, ServerError>; pub fn station(&self, slug: &str) -> Option<&Station> }`
  - `pub fn new_key() -> Result<String, ServerError>` — 32 random bytes (`ring::rand::SystemRandom`), base64url without padding.
  - `pub fn hash_key(key: &str) -> String` — `"sha256:"` + lowercase hex of SHA-256.
  - `pub fn key_matches(key: &str, key_hash: &str) -> bool` — hashes `key`, compares the digests in constant time; `false` for a hash that doesn't parse.

- [ ] **Step 1: Write the failing tests** in `tests/it/config.rs`:

```rust
const TWO: &str = r#"
listen = "127.0.0.1:8080"
public_url = "https://radio.example.org"
[[station]]
slug = "morning"
name = "Morning Station"
key_hash = "sha256:00"
[[station]]
slug = "night"
name = "Night Station"
key_hash = "sha256:11"
max_stream_listeners = 5
"#;

#[test] fn parses_stations_with_default_cap() {
    let c = Config::parse(TWO).unwrap();
    assert_eq!(c.stations.len(), 2);
    assert_eq!(c.station("morning").unwrap().max_stream_listeners, 200);
    assert_eq!(c.station("night").unwrap().max_stream_listeners, 5);
    assert!(c.station("noon").is_none());
}
#[test] fn rejects_duplicate_slug()        // second [[station]] slug = "morning" → Err(Config) naming "morning"
#[test] fn rejects_slug_that_is_not_a_path_segment()  // "", "a/b", "..", "A b", "admin", "api", "status-json.xsl" → Err
#[test] fn rejects_unknown_field()         // `key = "plain"` on a station → Err
#[test] fn rejects_one_tls_path_without_the_other()
#[test] fn a_key_matches_only_its_own_hash() {
    let (a, b) = (new_key().unwrap(), new_key().unwrap());
    assert_ne!(a, b);
    assert!(key_matches(&a, &hash_key(&a)));
    assert!(!key_matches(&b, &hash_key(&a)));
    assert!(!key_matches(&a, "not a hash"));
}
```

- [ ] **Step 2:** Run `mise exec -- cargo test -p radio-server`. Expected: fails to compile (crate is empty).
- [ ] **Step 3: Implement.** `Cargo.toml`: `name = "radio-server"`, workspace version/edition/licence/lints; deps `ring`, `base64`, `serde`, `toml`, `serde_json`, `thiserror`, `percent-encoding`, `rustls`; dev-deps `radio-stream`, `rtrb`, `rcgen`, `tempfile`. Add `radio-server = { path = "crates/server" }` to `[workspace.dependencies]`. A slug is 1–32 characters of `a-z`, `0-9`, `-`, not starting with `-`, and not one of `admin`, `api`, `status-json.xsl`.
  The spec's `media`, `public_ip`, `max_webrtc_listeners`, `guest_key` and `[[station.output]]` keys belong to later phases and are not accepted yet.
- [ ] **Step 4:** Run tests, fmt, clippy. Expected: all pass, no warnings.
- [ ] **Step 5: Commit** — "The station server reads its stations and key hashes from a config file".

---

### Task 2: Wrong-key lockout

**Files:** Create `src/lockout.rs`; Test `tests/it/lockout.rs`.

**Interfaces:**
- Produces: `pub struct Lockout` (`Default`, `Send + Sync`, interior `Mutex`);
  `pub fn blocked(&self, addr: IpAddr, now: Instant) -> bool`;
  `pub fn failed(&self, addr: IpAddr, now: Instant)`.
  Time is always passed in, so tests need no sleeping.

- [ ] **Step 1: Failing tests:**

```rust
#[test] fn fifth_wrong_key_within_a_minute_locks_the_address_for_five_minutes() {
    let (l, t, a) = (Lockout::default(), Instant::now(), addr("203.0.113.9"));
    for i in 0..4 { l.failed(a, t + secs(i)); assert!(!l.blocked(a, t + secs(i))); }
    l.failed(a, t + secs(4));
    assert!(l.blocked(a, t + secs(5)));
    assert!(l.blocked(a, t + secs(4 + 299)));
    assert!(!l.blocked(a, t + secs(4 + 301)));
}
#[test] fn failures_spread_over_more_than_a_minute_do_not_lock()   // 5 failures 20 s apart → never blocked
#[test] fn one_address_locked_leaves_another_free()
#[test] fn old_entries_are_forgotten()   // 10_000 distinct addresses fail once; after 301 s and one more call, the map holds < 100
```

The last test needs `pub fn tracked(&self) -> usize` (doc-hidden).

- [ ] **Step 2:** Run; expect compile failure.
- [ ] **Step 3: Implement** with `HashMap<IpAddr, Entry { failures: VecDeque<Instant>, locked_until: Option<Instant> }>`; prune on each `failed`.
- [ ] **Step 4:** Tests, fmt, clippy pass.
- [ ] **Step 5: Commit** — "Five wrong keys in a minute lock an address out for five minutes".

---

### Task 3: Reading requests

**Files:** Create `src/http.rs`; Test `tests/it/http.rs`.

**Interfaces:**
- Produces:
  - `pub struct Request { pub method: String, pub path: String, pub query: Vec<(String, String)>, headers: Vec<(String, String)> }` — path and query values percent-decoded.
  - `pub fn read_request(stream: &mut impl Read, deadline: Instant) -> Result<Request, HttpError>` — reads up to the blank line and **no byte beyond it** (the source body follows on the same socket). `HttpError::{TooLarge, Timeout, Malformed, Io}`. Limit: 8192 bytes of head.
  - `impl Request { pub fn header(&self, name: &str) -> Option<&str>` (case-insensitive); `pub fn basic_password(&self) -> Option<String>`; `pub fn bearer(&self) -> Option<&str>`; `pub fn param(&self, name: &str) -> Option<&str> }`
  - `pub fn client_addr(peer: IpAddr, request: &Request, trusted_proxy: Option<IpAddr>) -> IpAddr` — the last entry of `X-Forwarded-For` only when `peer == trusted_proxy`; else `peer`.
  - `pub fn respond(stream: &mut impl Write, status: u16, reason: &str, headers: &[(&str, &str)], body: &[u8]) -> std::io::Result<()>` — HTTP/1.0, `Connection: close`, `Content-Length`.
  - The caller sets the socket read timeout; `read_request` treats `WouldBlock`/`TimedOut` after `deadline` as `Timeout`.

- [ ] **Step 1: Failing tests:** `parses_the_apps_source_request` (the exact head `output.rs` `connect()` writes: `PUT /morning/stream HTTP/1.0`, `Authorization: Basic …`, `Content-Type: audio/ogg`, `Ice-Name`; assert method, path, `basic_password()` equals the key, and that bytes after the blank line are still unread in the reader); `decodes_the_metadata_query` (`song=Cloud%20Signal%20%E2%80%93%20Echo` → `param("song") == "Cloud Signal – Echo"`, `mount=%2Fmorning%2Fstream`); `head_over_the_limit_is_too_large`; `a_head_that_never_ends_times_out` (a reader that yields one byte then `TimedOut` forever, deadline in the past → `Timeout`); `forwarded_address_counts_only_from_the_trusted_proxy` (same header: peer = proxy → forwarded address; peer = other → peer; no proxy configured → peer); `bearer_and_basic_are_told_apart`.
- [ ] **Step 2:** Run; expect compile failure.
- [ ] **Step 3: Implement**, reading byte by byte as `output.rs` does for replies.
- [ ] **Step 4:** Tests, fmt, clippy pass.
- [ ] **Step 5: Commit** — "The station server reads HTTP requests with a size limit and a deadline".

---

### Task 4: Framing and the mount

**Files:** Create `src/framing.rs`, `src/mount.rs`; Test `tests/it/framing.rs`, `tests/it/mount.rs`.

**Interfaces:**
- Produces (`framing.rs`):
  - `pub enum Format { Ogg, Mp3 }`; `Format::from_content_type(&str) -> Option<Format>` (`audio/ogg`, `application/ogg` → Ogg; `audio/mpeg` → Mp3); `Format::content_type(self) -> &'static str`.
  - `pub struct Framer`; `Framer::new(Format)`; `pub fn push(&mut self, bytes: &[u8], out: &mut Vec<Arc<[u8]>>)` — appends whole Ogg pages or whole MP3 frames; partial input waits for the next call; bytes before the first sync are dropped.
  - `pub fn headers(&self) -> Vec<Arc<[u8]>>` — Ogg: the pages of the current logical stream from its BOS page up to the last page with granule position 0 (OpusHead and OpusTags). It is replaced when a new BOS page arrives (the app starts a new logical stream on each title change). MP3: empty.
  - `pub fn tags(&self) -> Option<(String, Option<String>)>` — `TITLE` and `ARTIST` of the current OpusTags.
  - A buffer that grows past 256 KiB without a complete page or frame is cleared (garbage input).
- Produces (`mount.rs`):
  - `pub struct Mount` (`Send + Sync`); `Mount::new(max_listeners: usize) -> Arc<Mount>`.
  - `pub fn begin(self: &Arc<Self>, format: Format, name: String) -> Option<SourceGuard>` — `None` while another source is live.
  - `impl SourceGuard { pub fn write(&mut self, bytes: &[u8]) }`; dropping it ends the mount: every listener's `recv` returns `None`, and `begin` works again.
  - `pub fn listen(self: &Arc<Self>) -> Result<Listener, ListenError>` — `ListenError::{NoSource, Full}`. A new listener first receives `headers()`, then chunks from the next boundary on.
  - `impl Listener { pub fn format(&self) -> Format; pub fn recv(&self, timeout: Duration) -> Option<Arc<[u8]>> }`; dropping it frees its place.
  - `pub fn set_title(&self, title: String)` (MP3 sources; Ogg titles come from tags).
  - `pub fn stats(&self) -> MountStats`; `pub struct MountStats { pub live: bool, pub format: Option<Format>, pub listeners: usize, pub name: String, pub title: String, pub artist: String }`.
  - Each listener has a bounded queue of **256** chunks (`std::sync::mpsc::sync_channel`); `try_send` failing drops that listener. `write` never blocks on a listener.

- [ ] **Step 1: Failing tests.** Build Ogg input with `radio_stream::OggMuxer` and `OpusEncoder` (dev-dependency), as the app does:
  - `ogg_chunks_are_whole_pages_however_the_bytes_arrive` — feed the same stream in 1-byte, 7-byte and whole pieces; the concatenated chunks equal the input each time, and every chunk starts with `OggS`.
  - `a_new_title_replaces_the_header_pages` — `start("Cloud Signal", Some("Echo"), …)`, packets, then `start("Dawn", None, …)`: `tags()` goes from `("Cloud Signal", Some("Echo"))` to `("Dawn", None)`, and `headers()` changes.
  - `mp3_chunks_start_at_a_frame` — with `radio_stream::MP3`, encode a second of audio through the public MP3 path used by `tests/it/mp3.rs` in `crates/stream`, prepend 100 junk bytes; every chunk starts with a frame sync (`0xFF`, top 3 bits of the next byte set) and the junk is gone.
  - `a_listener_who_joins_late_gets_a_decodable_stream` — source writes 2 s, a listener joins, source writes 1 s more; the listener's bytes begin with the header pages and decode (Symphonia via `radio_stream::StreamInput` is heavier than needed: assert the first chunk is the BOS page and the second page onward carries a granule greater than the joiner missed, i.e. later than the last pre-join page).
  - `a_second_source_is_refused_until_the_first_ends`.
  - `a_listener_who_stops_reading_is_dropped_and_others_keep_up` — listener A never calls `recv`; B reads everything. Write 1000 chunks: `write` returns promptly each time (whole loop under 2 s), B received all of them in order, `stats().listeners` ends at 1.
  - `the_cap_refuses_one_listener_too_many` — cap 2: third `listen()` is `Err(Full)`; after one drops, it succeeds.
  - `ending_the_source_ends_every_listener`.
  - `garbage_input_does_not_grow_the_buffer` — 2 MiB of zeros: no chunk, and a following valid stream still frames.
- [ ] **Step 2:** Run; expect compile failure.
- [ ] **Step 3: Implement.** Ogg page length = 27 + segment count + sum of the lacing values (header byte 26 is the count; BOS is flag `0x02` in byte 5; granule is bytes 6–13, little-endian). MP3 frame length comes from the frame header (version, layer III, bitrate index, sample rate index, padding); resync on a header that doesn't parse.
- [ ] **Step 4:** Tests, fmt, clippy pass.
- [ ] **Step 5: Commit** — "A mount passes a source's stream to its listeners from a page or frame boundary".

---

### Task 5: The server: source in, stream out

**Files:** Create `src/server.rs`; Test `tests/it/server.rs`.

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces:
  - `pub struct Server; impl Server { pub fn start(config: Config) -> Result<ServerHandle, ServerError> }` — binds `config.listen` (port 0 allowed), one `Mount` per station, an accept thread, one thread per connection.
  - `impl ServerHandle { pub fn local_addr(&self) -> SocketAddr; pub fn stop(&self) }`; dropping stops.
  - Doc-hidden `Server::start_with(config, timeouts: Timeouts)`, `pub struct Timeouts { pub head: Duration, pub source_silence: Duration }` (defaults 5 s and 10 s) so tests don't wait.
  - Routing in this task:
    - `PUT` or `SOURCE /<slug>/stream`: unknown slug → 404. Address locked out → 429. `basic_password()` must match the station's `key_hash` (any user name); else `Lockout::failed` and 401 with `WWW-Authenticate: Basic`. Unknown `Content-Type` → 415. Mount busy → 403 with body "this mount already has a source". Otherwise write `HTTP/1.0 200 OK\r\n\r\n` at once, then copy the body into the `SourceGuard` until EOF, an error, or `source_silence` without a byte.
    - `GET /<slug>/stream`: unknown slug or no source → 404; full → 503; else `200` with `Content-Type` of the mount, `Cache-Control: no-cache, no-store`, `Access-Control-Allow-Origin: *`, no `Content-Length`, then chunks until the mount ends or a write fails. Write timeout 10 s.
    - Anything else → 404; a method other than GET/PUT/SOURCE/OPTIONS → 405. `OPTIONS` → 204 with the CORS header.

- [ ] **Step 1: Failing tests** (raw `TcpStream` clients against `127.0.0.1:0`; a helper `push(addr, slug, key, content_type) -> TcpStream` writes the app's request head and returns after reading the status line):
  - `the_right_key_gets_an_explicit_200_and_the_wrong_one_401`.
  - `a_listener_hears_what_the_source_sends` — push Ogg from `OggMuxer`; a GET reads back bytes that start with `OggS` and contain a page the source sent after the listener joined.
  - `an_unknown_station_is_404_for_source_and_listener`.
  - `a_second_source_gets_403_while_the_first_is_live_and_200_after_it_ends`.
  - `five_wrong_keys_then_the_right_key_is_429` — and a different loopback address (`127.0.0.2`, bind the client socket to it; skip the second half with a clear message where the OS lacks it — macOS needs the alias) is not locked. If binding `127.0.0.2` fails, assert the lockout half only.
  - `a_wrong_key_on_one_station_does_not_open_another` — station A's key on station B's mount → 401.
  - `a_source_that_goes_silent_frees_the_mount` — `source_silence` 200 ms: push, send one page, stop; within 2 s the listener's stream ends and a new source gets 200.
  - `a_source_that_returns_with_another_format_ends_old_listeners` — Ogg source, listener attached, source closes, MP3 source connects: the Ogg listener's connection ends and never receives a byte of the MP3 body; a new GET gets `audio/mpeg`.
  - `the_listener_cap_answers_503` — cap 1.
  - `a_slow_head_is_closed_at_the_deadline` — `head` 200 ms: connect, send `GET /mor`, wait; the server closes within 2 s, and a normal request made meanwhile is served.
- [ ] **Step 2:** Run; expect compile failure.
- [ ] **Step 3: Implement.** At most `sum(max_stream_listeners) + 2 × stations + 32` connection threads; beyond that, accept and close at once. `stop` sets a flag, ends every mount and unblocks `accept` by connecting to itself.
- [ ] **Step 4:** Tests, fmt, clippy pass.
- [ ] **Step 5: Commit** — "The station server takes a keyed Icecast source per station and serves it to listeners".

---

### Task 6: Titles

**Files:** Modify `src/server.rs`; Test `tests/it/server.rs`.

**Interfaces:**
- Produces: `GET /admin/metadata?mount=/<slug>/stream&mode=updinfo&song=<title>` with the station key as the Basic password → `Mount::set_title`, `200` with body `<?xml version="1.0"?><iceresponse><message>Metadata update successful</message><return>1</return></iceresponse>`. Unknown mount → 404; wrong key → 401 and counted by the lockout; `mode` other than `updinfo` or no `song` → 400; no live source → 400.

- [ ] **Step 1: Failing tests:** `a_title_update_with_the_station_key_is_taken` (then visible through Task 7's stats — until Task 7 lands, assert the 200 and the 401/404/400 cases); `a_title_update_needs_the_key_of_that_station`.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5: Commit** — "MP3 sources set their title on the station server as on Icecast".

---

### Task 7: Stats for the listener page

**Files:** Create `src/stats.rs`; Modify `src/server.rs`; Test `tests/it/server.rs`.

**Interfaces:**
- Produces: `pub fn status_json(public_url: &str, mounts: &[(String, MountStats)]) -> String` and `GET /status-json.xsl` (public, `Content-Type: application/json`, CORS header). Shape, as `site/index.html` reads it (`icestats.source[]` with `listenurl`, `title`, `artist`, `server_name`):

```json
{"icestats":{"source":[{"listenurl":"https://radio.example.org/morning/stream","server_name":"Morning Station","server_type":"audio/ogg","listeners":3,"title":"Cloud Signal","artist":"Echo"}]}}
```

  Only live mounts are listed. `source` is always an array. Built with `serde_json`, so any title is escaped.

- [ ] **Step 1: Failing tests:** `stats_list_live_mounts_with_their_titles` (Ogg source with tags → title and artist; MP3 source after a metadata update → title; a station with no source is absent; `listeners` rises when a listener attaches); `a_title_with_quotes_and_control_characters_keeps_the_json_valid` (`He said "hi"\u{1}\n` and a `song=` with invalid UTF-8 percent bytes `%FF%FE`: the body parses with `serde_json` and round-trips the valid title).
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5: Commit** — "The station server reports each live stream's title and listeners".

---

### Task 8: TLS from certificate files, and connection limits

**Files:** Modify `src/server.rs`; Test `tests/it/server.rs`.

**Interfaces:**
- Consumes: `Config::tls_cert`, `Config::tls_key` (PEM paths, both or neither — Task 1).
- Produces: with both set, every connection is `rustls` server TLS (ring provider, as `crates/stream` uses); a bad file is `ServerError::Tls` from `Server::start`, naming the path, never a panic. Without them, plain HTTP.

- [ ] **Step 1: Failing tests:** `with_certificate_files_the_server_speaks_tls` (`rcgen` certificate written to a `tempfile` dir; a `rustls` client trusting it pushes a source and reads the stream; a plain-HTTP request to the same port gets no HTTP reply); `a_missing_certificate_file_is_a_start_error`; `connections_over_the_limit_are_closed_and_the_rest_are_served` (cap 1, one station: open more idle connections than the limit from Task 5, then a real listener still gets its 200 once idle ones time out at `head`).
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5: Commit** — "The station server can serve TLS itself from certificate files".

---

### Task 9: The binary

**Files:**
- Create: `apps/radio-server/Cargo.toml`, `apps/radio-server/src/main.rs`
- Test: `apps/radio-server/tests/it/main.rs`

**Interfaces:**
- Consumes: `Config::load`, `Server::start`, `new_key`, `hash_key`.
- Produces: package `craftradio-server`, binary `craftradio-server`:
  - `craftradio-server run --config <path>` — loads, starts, prints one line `listening on <addr>, <n> station(s)` to stderr, runs until killed. A config error prints the error and exits with code 2. Never prints a key or hash.
  - `craftradio-server key` — prints to stdout exactly two lines: `key: <key>` and `key_hash = "<hash>"`, and to stderr "Put the key_hash line in the station's config. The key is shown once; keep it for the app."

- [ ] **Step 1: Failing tests** (run the built binary via `env!("CARGO_BIN_EXE_craftradio-server")`): `key_prints_a_key_and_the_hash_that_matches_it` (parse both lines; `key_matches` holds); `run_with_a_broken_config_exits_2_and_names_the_problem` (duplicate slug in a temp file → exit code 2, stderr contains the slug); `run_serves_the_configured_stations` (`listen = "127.0.0.1:0"`; read the address from stderr; a GET to `/status-json.xsl` answers 200; kill the child).
- [ ] **Step 2–4:** Fail, implement, pass. Also run `mise exec -- cargo clippy -p craftradio-server --all-targets -- -D warnings`.
- [ ] **Step 5: Commit** — "craftradio-server runs the station server and makes station keys".

---

### Task 10: The app's Icecast output against the server, and packaging

**Files:**
- Test: `crates/server/tests/it/e2e.rs`
- Create: `packaging/server/craftradio-server.example.toml`, `packaging/server/Caddyfile`, `packaging/server/craftradio-server.service`, `packaging/server/README.md`

**Interfaces:**
- Consumes: `radio_stream::{IcecastClient, IcecastConfig, StreamFormat, StreamStatus, StreamInput}` (dev-dependency), `Server::start`.

- [ ] **Step 1: Failing tests:**
  - `the_apps_opus_output_is_heard_through_the_server` — start the server on `127.0.0.1:0`; `IcecastClient::start` with host `127.0.0.1`, that port, mount `/morning/stream`, `tls: false`, `StreamFormat::Opus { kbps: 96 }`, fed a 440 Hz tone through an `rtrb` ring; wait until `status()` is `Connected`; open `StreamInput::open("http://127.0.0.1:<port>/morning/stream", …)` and assert non-silent samples arrive within 10 s (as `hears_audio` in `crates/stream/tests/it/tls.rs` does).
  - `the_apps_title_reaches_the_stats` — `set_title("Cloud Signal")` on the handle; within 5 s `/status-json.xsl` shows it. Run for Opus, and for MP3 under `#[cfg]` on `radio_stream::MP3`.
  - `the_app_reports_a_wrong_key_as_refused` — wrong password: `status()` never becomes `Connected` and carries an error that mentions `401`.
- [ ] **Step 2:** Run; fix whatever the real client exposes in Tasks 5–7 (this is the point of the task). Expected at the end: pass.
- [ ] **Step 3: Write packaging.** Example config with two invented stations and comments for every key. `Caddyfile`: `reverse_proxy 127.0.0.1:8080` with `flush_interval -1` and request buffering off, for one host. `README.md`: install, make a key, the Caddy route, the direct-TLS route (`tls_cert`/`tls_key`), the nginx note (`proxy_request_buffering off; proxy_buffering off; proxy_http_version 1.1;`), and how to set the app's Icecast output by hand (host, port, mount `/<slug>/stream`, any user, the key as password).
- [ ] **Step 4: Check the push through a real proxy.** If `caddy` is installed (`command -v caddy`), run the server and Caddy locally with `tls internal`, push with the e2e client trusting Caddy's root, and confirm audio arrives; record the Caddy version and result in the README under "Checked". If Caddy is not installed, write "Not checked through a proxy" there and say so in the task report. Do not claim it works unchecked.
- [ ] **Step 5:** `mise exec -- cargo fmt --all`, tests and clippy for both packages pass.
- [ ] **Step 6: Commit** — "The app's Icecast output is tested against the station server, with setup notes for a host".

---

## After this plan

Phase 1b (outputs) is next: it adds `[[station.output]]`, a decoder per station and an
encoder per output on top of `Mount`. Write its plan once this phase has merged, since it
builds on the `Mount` and `Framer` interfaces as they actually land.
