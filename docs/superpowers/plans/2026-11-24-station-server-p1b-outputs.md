# Station Server, Phase 1b: Outputs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A station's one pushed stream is also served in any number of other formats and
bitrates, set in the server's config (`[[station.output]]`), each at `/<slug>/<mount>`.

**Architecture:** While a source is live, one transcoder thread per station taps the pushed
mount, decodes it to 48 kHz stereo and encodes once per output. Each output is an ordinary
`Mount` the transcoder writes to as a source would, so framing, header replay, late joining
and slow-listener handling are the code phase 1 already has. The transcoder decodes only
while some output has a listener, and encodes only the outputs that have one.

**Tech Stack:** what the workspace has: Symphonia and `radio-codecs` (decoding), the `opus`
and `ogg` crates and craft-audio's MP3 encoder (encoding). No new third-party crate.

**Spec:** `docs/superpowers/specs/2026-11-24-station-server-design.md` §5.1 (config), §5.2
(`GET /<slug>/<mount>`), §5.6. The pairing document's list of outputs and `default = true`
are phase 2 and are not built here.

## Global Constraints

- Run cargo as `mise exec -- cargo …`. For each task, for the crates it changed:
  `cargo fmt --all`, `cargo test -p <crates>`,
  `cargo clippy -p <crates> --all-targets -- -D warnings`.
- `unsafe_code = "forbid"`. Non-test code does not panic: no `unwrap`, `expect`, indexing
  that can fail, or `panic!` outside tests. Fix lints; never `allow` them.
- No new third-party dependency. If one seems needed, stop and ask (rule 12).
- Moving code and changing behaviour are separate commits (rule 3).
- Integration tests are one binary per crate (`tests/it/main.rs`, modules beside it). Tests
  assert behaviour and relations, not constants (rules 14–15). Never weaken, delete or
  `#[ignore]` a test to pass (rule 17). Tests use no fixed sleeps as proof: they wait for a
  condition with a deadline.
- Fixture names, titles and artists are invented ("Morning Station", "Cloud Signal"). No
  real keys, hosts or songs (rule 11).
- Nothing shows success for something that isn't working (rule 7): an output that can't
  run answers with a status and the reason in words, and is not listed in the stats.
- `/<slug>/stream` stays the pushed bytes, untouched, whatever this phase adds.
- The source's connection thread never decodes or encodes; only the transcoder thread does.
- Pass bytes to `SourceGuard::write` in pieces of 4 KiB or less (see its doc comment).
- Commits are one change each, in plain English saying what changed for the person
  running the server, ending with
  `Co-Authored-By: Claude Fable 5.2 <noreply@anthropic.com>`. Work on
  `feature/server-p1b-outputs`; do not push.

## Calls this plan makes

The spec leaves these open. Each is the plan author's call, not the operator's decision.

1. **Outputs need a 48 kHz stereo source.** The app always sends that. For any other source
   (44.1 kHz or mono from another client) the pushed mount works as before and the outputs
   are off, with the reason in words. No resampler in this phase.
2. **The encoders move from `radio-stream` to `radio-codecs`**, behind features, and
   `radio-stream` re-exports them. The server then depends on `radio-codecs` alone and does
   not link `radio-stream`'s WebRTC, UPnP and image code. `server` becomes a layer 2 crate.
3. **`default = true` on an output is not read yet.** It only matters to the pairing
   document (phase 2); until then the config refuses it as an unknown field.
4. **ICY titles are sent on every MP3 mount when the listener asks**, the pushed one too.
   The spec names MP3 outputs only. The audio bytes of the pushed mount are still untouched;
   the listener asked for the metadata blocks between them.
5. **An output's title and artist are the pushed mount's**, read from it for the stats and
   for ICY. Opus outputs also carry them in the stream, as a new chain on each change.
6. **An idle transcoder checks for listeners every 50 ms.** That is the wait the spec's
   "well under a second" allows; it costs no decoding.

## Review Focus

1. A title change (a new Ogg chain) while an output is being listened to: audio goes on.
   Tasks 1 and 6.
2. A source that is not 48 kHz stereo: the pushed mount plays, outputs say why they are
   off, never wrong pitch or silence. Tasks 1, 6 and 7.
3. The source reconnects in another format while listeners sit on outputs: they end with
   the old source, and new listeners hear the new one. Task 6.
4. The last listener of an output leaves and another joins a moment later, repeatedly:
   every join hears audio. Task 6.
5. A burst from the source that overruns the transcoder's tap: outputs recover on their
   own. Task 6.

---

### Task 1: The server decodes a pushed stream

The riskiest part first: nothing in the workspace decodes Ogg/Opus across a title change
(`crates/audio/src/decode.rs` treats `ResetRequired` as the end).

**Files:**
- Modify: `crates/server/Cargo.toml` (`radio-codecs` and `symphonia` become dependencies),
  `crates/server/src/mount.rs`, `crates/server/src/lib.rs`, `AGENTS.md` (the `server` row:
  layer 2, depends on `codecs`)
- Create: `crates/server/src/decode.rs`
- Test: `crates/server/tests/it/decode.rs`, `crates/server/tests/it/mount.rs`

**Interfaces:**
- Produces, in `mount.rs`:
  - `Mount::tap(self: &Arc<Self>) -> Option<Listener>`: like `listen`, but not counted in
    the listener limit or in `MountStats::listeners`. `None` with no live source.
- Produces, in `decode.rs`:
  ```rust
  pub const RATE: u32 = 48_000;
  /// Interleaved stereo samples in one block: 20 ms.
  pub const BLOCK: usize = 1920;
  pub enum DecodeError {
      /// The source is not 48 kHz stereo.
      Unsupported { rate: u32, channels: usize },
      /// The bytes are not a stream the decoders read.
      Unreadable(String),
  }
  pub enum Decoded { Audio, Overrun, Ended }
  impl SourceDecoder {
      /// Reads the tap until the stream's format is known. `stop` ends any wait.
      pub fn open(tap: Listener, stop: Arc<AtomicBool>) -> Result<Self, DecodeError>;
      /// Appends the next packet's interleaved stereo samples to `out`.
      /// `Overrun`: the tap fell behind and was dropped. `Ended`: the source ended or
      /// `stop` was set.
      pub fn next(&mut self, out: &mut Vec<f32>) -> Result<Decoded, DecodeError>;
  }
  ```

- [ ] **Step 1: Write the failing tests** in `tests/it/decode.rs`. Feed a `Mount` from a
  thread with a tone made by the app's own encoders (`radio-stream`, a dev-dependency):
  - `opus_is_decoded_across_title_changes`: three chains (`OggMuxer::start` three times),
    the same number of packets in each. Assert the decoder returns more frames than two
    and a half chains hold, and that samples after the second change are not silent.
  - `mp3_is_decoded`: an MP3 tone comes out not silent.
  - `a_44_1_khz_source_is_refused`: MP3 frames at 44.1 kHz (craft-audio's encoder, a
    dev-dependency from the workspace, made in the test) give `Unsupported` with that rate, from `open` or the first `next`.
  - `an_overrun_is_told_apart_from_the_end`: write more chunks than a queue holds without
    reading; `next` gives `Overrun`. End the source instead; it gives `Ended`.
  - `stop_ends_a_decoder_waiting_on_a_silent_source`: `open` on a live mount that gets no
    bytes returns within a deadline once `stop` is set.
  - In `tests/it/mount.rs`, `a_tap_takes_no_listener_place`: a mount at its limit still
    gives a tap, and `stats().listeners` does not count it.
- [ ] **Step 2: Run them; they fail** (`decode` and `tap` don't exist).
- [ ] **Step 3: Implement `Mount::tap`** and the `Listener` accessor the decoder needs to
  tell an overrun from the end.
- [ ] **Step 4: Implement `SourceDecoder`.** A Symphonia format reader over a `Read` that
  blocks on the tap (poll `recv` at 100 ms and check `stop`), probed without a hint, with
  `radio_codecs::registry()` making the decoder. On `ResetRequired`, or a track whose
  parameters changed, make a new decoder and go on.
  **Fallback, if Symphonia cannot go on across a chain:** end the reader at each page that
  begins a logical stream (the BOS flag, byte 5 bit 1 of the page) and open a fresh reader
  starting with that page.
- [ ] **Step 5: Run the tests; they pass.** Update the `AGENTS.md` row.
- [ ] **Step 6: Commit** ("The station server can decode the stream a station pushes").

### Task 2: The encoders live in `radio-codecs`

**Files:**
- Create: `crates/codecs/src/encode.rs`
- Modify: `crates/codecs/Cargo.toml`, `crates/codecs/src/lib.rs`,
  `crates/stream/Cargo.toml`, `crates/stream/src/output.rs`, `crates/stream/src/lib.rs`;
  delete `crates/stream/src/mp3.rs`; `AGENTS.md` (the `codecs` row: decoders and encoders)

**Interfaces:**
- Produces, in `radio_codecs::encode`:
  - feature `encode`: `OpusEncoder`, `OggMuxer<W>` exactly as they are today, and
    `EncodeError` (what they return instead of `StreamError`).
  - feature `encode-mp3`: `pub struct Mp3Encoder` with
    `pub fn new(kbps: u32) -> Result<Self, EncodeError>` and
    `pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, EncodeError>`.
- `radio-stream` keeps `OpusEncoder` and `OggMuxer` at their current paths by re-export,
  has `impl From<EncodeError> for StreamError`, and its `mp3` feature turns on
  `radio-codecs/encode-mp3`. No caller of `radio-stream` changes.

- [ ] **Step 1: Move the code**, unit tests with it. No behaviour change: this is a move.
- [ ] **Step 2: Run** `cargo test -p radio-codecs --all-features -p radio-stream
  -p radio-server` and clippy for the three; all pass as before. Check `radio-codecs`
  without the features builds no encoder (`cargo tree -p radio-audio -i opus` finds none).
- [ ] **Step 3: Commit** ("The Opus and MP3 encoders move to the codecs crate, so the
  station server can use them").

### Task 3: Outputs in the config

**Files:**
- Modify: `crates/server/src/config.rs`, `crates/server/Cargo.toml` (`radio-codecs`
  features `encode`, `encode-mp3`)
- Test: `crates/server/tests/it/config.rs`

**Interfaces:**
- Produces:
  ```rust
  #[serde(deny_unknown_fields)]
  pub struct Output { pub mount: String, pub format: OutputFormat, pub kbps: u32 }
  #[serde(rename_all = "lowercase")]
  pub enum OutputFormat { Opus, Mp3 }
  // Station gains: #[serde(rename = "output", default)] pub outputs: Vec<Output>
  ```

- [ ] **Step 1: Write the failing tests:**
  - `a_station_takes_any_number_of_outputs`: the spec's example (§5.1) parses, in order.
  - `an_output_cannot_take_a_fixed_path`: `stream`, `live`, `signal` and `feed` are each
    refused, and the message names the station and the mount.
  - `two_outputs_of_a_station_cannot_share_a_mount`; the same mount on two stations is fine.
  - `a_mount_name_is_one_path_segment`: empty, `a/b`, a space, upper case and a leading
    `.` or `-` are refused; `stream.mp3` and `low_64` are taken.
  - `a_bitrate_the_encoder_cannot_make_is_refused`: MP3 at 100 and Opus at 600 are refused
    with the format and the number in the message; MP3 at 192 and Opus at 64 are taken.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.** A mount is 1–32 of `a-z`, `0-9`, `-`, `.`, `_`, not starting
  with `-` or `.`. A bitrate is valid when the encoder accepts it
  (`OpusEncoder::new`, `Mp3Encoder::new`), so the server keeps no bitrate table of its own.
- [ ] **Step 4: Run the tests; they pass.**
- [ ] **Step 5: Commit** ("The server's config takes a station's outputs").

### Task 4: One listener limit for a station

**Files:**
- Modify: `crates/server/src/mount.rs`, `crates/server/src/server.rs` (where mounts are made)
- Test: `crates/server/tests/it/mount.rs`

**Interfaces:**
- Produces: `pub struct ListenerCap`, `ListenerCap::new(max: usize) -> Arc<ListenerCap>`,
  and `Mount::new(cap: Arc<ListenerCap>) -> Arc<Mount>` in place of `Mount::new(usize)`.
  Mounts made with one cap share its places. `MountStats::listeners` stays per mount.

- [ ] **Step 1: Write the failing tests:**
  - `mounts_of_one_station_share_the_limit`: two live mounts on a cap of 2; one listener
    on each; a third on either is `ListenError::Full`; dropping one admits it.
  - `a_listener_dropped_for_falling_behind_gives_its_place_back` once its `Listener` is
    dropped.
  - `a_source_ending_gives_every_place_back`.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.** The `Listener` holds its place and frees it on drop, so a
  place is counted exactly once however the listener ends. Update the existing call sites.
- [ ] **Step 4: Run the tests; they pass**, the phase 1 mount and server tests included.
- [ ] **Step 5: Commit** ("A station's listener limit covers all its mounts together").

### Task 5: Encoding into a mount

**Files:**
- Create: `crates/server/src/encode.rs`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/mount.rs`
- Test: `crates/server/tests/it/encode.rs`

**Interfaces:**
- Consumes: `OpusEncoder`, `OggMuxer`, `Mp3Encoder` (Task 2), `OutputFormat` (Task 3),
  `BLOCK` (Task 1).
- Produces:
  - `Mount::begin(self: &Arc<Self>, format: Format, name: String, kbps: Option<u32>)`
    (a third argument) and `MountStats::kbps: Option<u32>`.
  - ```rust
    impl OutputEncoder {
        /// `sink` is the output mount's source guard.
        pub fn new(format: OutputFormat, kbps: u32, sink: SourceGuard)
            -> Result<Self, EncodeError>;
        /// Begins a stretch of audio: for Opus a new chain with these tags, for MP3 a
        /// fresh encoder. Call before the first block, after a pause, and when the title
        /// changes (Opus).
        pub fn start(&mut self, title: &str, artist: Option<&str>) -> Result<(), EncodeError>;
        /// Encodes one block of exactly `BLOCK` samples into the mount.
        pub fn encode(&mut self, block: &[f32]) -> Result<(), EncodeError>;
    }
    ```

- [ ] **Step 1: Write the failing tests** (a tone in, a listener on the mount out, decoded
  as the phase 1 tests decode):
  - `an_opus_output_plays_and_carries_its_title`: the tone is heard, and the mount's
    stats show the title and artist `start` was given.
  - `an_mp3_output_plays`.
  - `a_higher_bitrate_makes_more_bytes`: five seconds at 64 and at 192 kbit/s, for each
    format; the larger is larger, and each is within a quarter of `kbps × seconds / 8`.
  - `a_listener_joining_after_a_restart_starts_at_the_new_chain`: after a second `start`,
    a new listener's first chunk is a page that begins a stream, and the tags that follow
    are the second title's.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.** The Ogg muxer writes through a `Write` over the guard that
  passes at most 4 KiB a call.
- [ ] **Step 4: Run the tests; they pass.**
- [ ] **Step 5: Commit** ("The server can encode Opus and MP3 into a mount").

### Task 6: The transcoder

**Files:**
- Create: `crates/server/src/transcode.rs`
- Modify: `crates/server/src/lib.rs`
- Test: `crates/server/tests/it/transcode.rs`

**Interfaces:**
- Consumes: `Mount::tap`, `SourceDecoder` (Task 1), `ListenerCap` (Task 4),
  `OutputEncoder` (Task 5), `Output` (Task 3).
- Produces:
  ```rust
  /// Why a live station's outputs are off.
  pub enum Refusal { Unsupported { rate: u32, channels: usize }, Unreadable }
  impl Outputs {
      /// A station's output mounts, sharing the station's listener limit.
      pub fn new(configs: &[Output], cap: &Arc<ListenerCap>) -> Arc<Outputs>;
      pub fn is_empty(&self) -> bool;
      pub fn mount(&self, name: &str) -> Option<&Arc<Mount>>;
      /// In config order.
      pub fn each(&self) -> impl Iterator<Item = (&Output, &Arc<Mount>)>;
      /// Set while the live source can't be transcoded; cleared when it ends.
      pub fn refusal(&self) -> Option<Refusal>;
      /// Blocks decoded since the server started; for tests of "idle costs nothing".
      pub fn decoded_blocks(&self) -> u64;
  }
  /// Starts the station's transcoder thread. Call right after the pushed mount's
  /// `begin` succeeded.
  pub fn spawn(pushed: Arc<Mount>, outputs: Arc<Outputs>) -> Transcoder;
  impl Transcoder {
      /// Stops and joins the thread; every output mount has ended when this returns.
      /// Call before the pushed mount's guard is dropped, so a new source never finds
      /// the old outputs still live.
      pub fn finish(self);
  }
  ```

**Behaviour:**
1. At start, tap the pushed mount and open the decoder. A `DecodeError` sets the refusal;
   the outputs never go live for this source.
2. Otherwise begin every output mount (its format, the pushed mount's name, its kbps).
3. Then, until the source ends or `finish` is called:
   - No output has a listener: hold no tap and no decoder; look again in 50 ms.
   - Some do: tap and decode block by block. An output that has just gained its first
     listener gets `start` with the pushed mount's current title and artist, then every
     block. One that lost its last listener gets no more blocks.
   - The pushed mount's title or artist changed: `start` again on the Opus outputs in use.
   - `Decoded::Overrun`: tap and open again, `start` the outputs in use, go on.
4. At the end, drop the encoders, which ends every output mount and its listeners.

- [ ] **Step 1: Write the failing tests**, on mounts directly, no sockets. A helper pushes
  a tone into the pushed mount in real time from a thread.
  - `an_output_nobody_listens_to_is_not_decoded`: once the outputs are live, wait for
    `decoded_blocks` to stop changing; push two more seconds; it has not grown. Join a
    listener; it grows.
  - `a_listener_on_an_output_hears_the_source`, for Opus source to MP3 output and MP3
    source to Opus output.
  - `every_join_after_a_pause_hears_audio`: join, hear, leave, five times in a row
    (Review Focus 4).
  - `a_title_change_reaches_an_opus_output_and_audio_goes_on` (Review Focus 1): the output
    mount's stats show the new title, and its listener still receives audio after it.
  - `a_source_that_is_not_48_khz_stereo_turns_outputs_off` (Review Focus 2): the outputs
    never go live, `refusal()` gives the rate, the pushed mount still has listeners served;
    after the source ends `refusal()` is `None`.
  - `the_source_ending_ends_every_output`: listeners of two outputs get `Ended`.
  - `a_new_source_in_another_format_is_heard_on_the_outputs` (Review Focus 3): `finish`,
    then begin the pushed mount as MP3 and `spawn` again; a new output listener hears it.
  - `outputs_recover_from_a_burst` (Review Focus 5): write two thousand pages without
    pause while an output is listened to, then go on in real time; the listener, or a new
    one if it was ended, receives audio pushed after the burst.
  - `finish_returns_while_the_source_is_silent`, within a deadline.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement** the behaviour above.
- [ ] **Step 4: Run the tests; they pass.** Run the file ten times to catch a race.
- [ ] **Step 5: Commit** ("A station's stream is re-encoded for its outputs while someone
  listens").

### Task 7: Outputs over HTTP and in the stats

**Files:**
- Modify: `crates/server/src/server.rs`, `crates/server/src/stats.rs`
- Test: `crates/server/tests/it/server.rs`, `crates/server/tests/it/support.rs`

**Interfaces:**
- Consumes: `Outputs`, `spawn`, `Transcoder::finish`, `Refusal` (Task 6).
- Produces: `GET /<slug>/<mount>`; `status_json(public_url, entries: &[StatsEntry])` with
  `pub struct StatsEntry { pub path: String, pub stats: MountStats }`, where `path` is
  `<slug>/<mount>`.

**Behaviour:**
- `source()` reads `Ice-Bitrate` (a whole number, else none) into `begin`, spawns the
  transcoder when the station has outputs, and calls `finish` before the guard drops.
- `GET` on an output: the stream, with the output format's content type. The station off
  air: 404, as for the pushed mount. Outputs refused: 503 saying in words what the source
  sends and what outputs need. Full: 503 as today. Source live but the output not begun
  yet: wait up to two seconds for it, then 503 "starting".
- `PUT`, `SOURCE` and `/admin/metadata` naming an output path are answered 404 like any
  unknown path. No key is checked there, so they neither count toward the lockout nor take
  a station's reserved place.
- Stats: one entry per live mount, a station's pushed mount first, then its outputs in
  config order. Each has `listenurl`, `server_name`, `server_type`, `listeners`, `title`,
  `artist`, and `bitrate` when known. An output's title and artist are the pushed mount's.
  A refused output is not listed.

- [ ] **Step 1: Write the failing tests** (sources pushed over a socket, as the phase 1
  tests do):
  - `an_output_is_served_at_its_mount` with its content type, and decodes to audio.
  - `an_output_of_a_station_off_air_is_404`.
  - `the_pushed_mount_is_served_as_pushed`: with outputs configured and listened to, the
    bytes a `/stream` listener gets are bytes the source sent.
  - `the_stats_list_the_pushed_mount_first_then_the_outputs`, each with its bitrate and
    listener count, and the title set through `/admin/metadata` on all of them.
    `source_entry` in `support.rs` still finds the pushed mount as the listener page does.
  - `a_station_s_limit_covers_its_outputs`: with a limit of 2, the third listener gets 503
    whichever mount it asks for.
  - `outputs_of_a_44_1_khz_source_say_why_they_are_off`: 503, the body names 44100 and
    48000, and the stats list the pushed mount only.
  - `a_push_to_an_output_path_is_not_a_keyed_request`: six `PUT`s with a wrong key to an
    output path are 404, and the right key on `/stream` is then accepted.
  - `stop_returns_with_outputs_being_listened_to`, within a deadline.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests; they pass**, all of phase 1's included.
- [ ] **Step 5: Commit** ("The server serves a station's outputs and lists them in the
  stats").

### Task 8: ICY titles for MP3 listeners

**Files:**
- Modify: `crates/server/src/server.rs`; create `crates/server/src/icy.rs`
- Test: `crates/server/tests/it/icy.rs`

**Interfaces:**
- Produces, in `icy.rs`:
  ```rust
  pub const METAINT: usize = 16_000;
  impl IcyWriter {
      pub fn new() -> Self;
      /// Appends `chunk` to `out`, with a metadata block after every `METAINT` audio
      /// bytes: `StreamTitle='<now>';` when `now` differs from the last one sent, else an
      /// empty block.
      pub fn write(&mut self, chunk: &[u8], now: &str, out: &mut Vec<u8>);
  }
  ```
- `now` is `artist - title`, or the title alone with no artist. A block holds at most
  4080 bytes; a longer text is cut at a character boundary.

**Behaviour:** a `GET` with `Icy-MetaData: 1` on an MP3 mount (an output or the pushed one)
is answered with `icy-metaint: 16000` and `icy-name`, and the body interleaved. Without
the header, or on an Opus mount, nothing changes. The app's `StreamInput` always asks, so
phase 1's end-to-end MP3 test now runs through this path too.

- [ ] **Step 1: Write the failing tests:**
  - `the_apps_parser_reads_the_title_and_the_audio_back`: bytes from `IcyWriter` go
    through `radio_stream::parse_icy`; the audio that comes out equals what went
    in, and the title is the one given; after a change, the new one.
  - `an_unchanged_title_is_sent_once`.
  - `a_title_too_long_for_a_block_is_cut_at_a_character`, and still parses.
  - `a_listener_who_asks_gets_titles`: over a socket, on an MP3 output of an Opus source;
    a title set by the source arrives in the stream.
  - `a_listener_who_does_not_ask_gets_plain_mp3`: no `icy-metaint`, and the body frames
    as MP3 from its first byte.
  - `an_opus_mount_ignores_the_request`.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests; they pass.**
- [ ] **Step 5: Commit** ("MP3 listeners who ask for titles get them in the stream").

### Task 9: The app end to end, and the setup notes

**Files:**
- Modify: `crates/server/tests/it/e2e.rs`,
  `packaging/server/craftradio-server.example.toml`, `packaging/server/README.md`,
  `docs/superpowers/specs/2026-11-24-station-server-design.md`

- [ ] **Step 1: Write the failing tests**, with the app's real Icecast output pushing:
  - `the_apps_opus_push_is_heard_as_mp3_by_the_apps_stream_input`: the app's `StreamInput`
    plays `/<slug>/stream.mp3`, and its `icy_title()` becomes the title the app set.
  - `the_apps_opus_push_is_heard_on_a_lower_opus_output`.
  - `the_apps_mp3_push_is_heard_on_an_opus_output`.
- [ ] **Step 2: Run them.** They should pass on the work of Tasks 1–8; a failure is a bug
  in an earlier task, to fix there in its own commit.
- [ ] **Step 3: Write the setup notes.** The example config gains two outputs. The README
  says what an output is, the mount name rule, the valid bitrates, that outputs need a
  48 kHz stereo source, what they cost (one decode per station and one encode per output,
  only while someone listens), and that the listener limit covers all mounts.
- [ ] **Step 4: Update the spec:** the status line (phase 1b built), §5.6 with the calls
  above (48 kHz stereo only, ICY on every MP3 mount, `default` left to phase 2), and what
  was left open.
- [ ] **Step 5: Run** `cargo test -p radio-server -p craftradio-server -p radio-codecs
  -p radio-stream`, clippy with `-D warnings` for the four, and `cargo fmt --all --check`.
- [ ] **Step 6: Commit** the tests, then the notes and spec, as two commits.
