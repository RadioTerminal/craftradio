# Radio Station Manager — Design

Date: 2026-11-21 · Status: draft for approval

## 1. Intent

A radio automation and playout app: four players (decks A–D) driven by a playlist that
shows, for every item, the time it will start. The playlist mixes music, jingles, ads,
live sound-card input and internet streams; items can be pinned to an exact wall-clock time
and the playlist rearranges itself so they land on time. A cartwall fires jingles over
whatever is on air. Everything can be scripted in JavaScript. Output goes to the local sound
card and/or Icecast.

**Assumed (not yet confirmed by the user):** it serves a real station that runs unattended
24/7, with live shows and pre-recorded shows on top. This is why reliability and timing
accuracy outrank features.

## 2. Goals

1. Four decks (A–D) with crossfades, cue points and pre-loading, so the next item is always
   ready before it is needed.
2. Playlist with an estimated start time per row, updated live as things play.
3. Items from three kinds of source: audio files, live sound-card input, internet streams.
4. Fixed-time events that land on time, using the agreed strategy (§6.3).
5. Any number of named playlists (jingles, ads, shows, news…) woven into the timeline by rules.
6. Cartwall: carts that play over whatever is on air, with optional ducking.
7. JavaScript scripting of events, playlist changes, carts and custom rules.
8. Outputs: local audio device(s) and Icecast, with now-playing metadata.
   WebRTC listening (added 2026-11-22): `--webrtc <port>` serves Program peer to peer to
   browsers over WHEP, with public STUN for the address outside the NAT. The listener page is
   `site/index.html`, on GitHub Pages too. It is for quick trials, not a CDN: each listener
   costs the station's own upload, and there is no TURN.
   Listen link (added 2026-11-22): the station keeps an Ed25519 key in its data folder, and
   `https://radioterminal.github.io/craftradio/#<public key>` plays it from anywhere. The
   station answers offers through public WebTorrent trackers over `wss://` (no port-forward),
   signs each answer, and the page checks the signature. Title, artist and cover art go to
   listeners on a WebRTC data channel.
9. Every action also exists as a CLI command and an MCP tool, following the template.

## 3. Non-goals (v1)

- Remote or multi-operator use over the network (that was approach B, not chosen).
- A browser UI (the template's `apps/*-web`); the desktop app is the only UI.
- Music scheduling at the level of full traffic/music-scheduling suites (complex
  dayparting, reconciliation with billing systems). Rules cover the common cases; scripts
  cover the rest.
- HLS / DASH stream inputs, video, RDS hardware control.
- Audio processing beyond gain, fades, ducking, and one compressor and limiter on Program
  (no multiband processing).

## 4. Anti-goals (failure even if it "works")

- **Dead air or audio dropouts** caused by the UI, a script, disk or network.
- **Fixed events landing late** beyond their tolerance without an alarm.
- **Start-time estimates that drift** from what actually happens.
- A script error or a slow script affecting playback.
- Losing the playlist or the as-played log after a crash or power cut.

## 5. Constraints and decisions

- **Template:** [storytold/craft-anything](https://github.com/storytold/craft-anything)
  as the template for how it's built. That means a Rust (edition 2024) Cargo workspace,
  `crates/*` for the libraries and `apps/*` for the binaries, an egui 0.36 / eframe desktop UI,
  a test-kit crate, and every action exposed as a CLI command and an MCP tool.
- **Platforms (user):** Windows, Linux and macOS in v1. Audio uses cpal's default backends
  (WASAPI, ALSA/PulseAudio/PipeWire, CoreAudio). ASIO and JACK are optional build features
  later (my call). CI builds and tests on all three, as the template does.
- **Architecture: approach A (chosen by the user), one app.** A single process holds the
  decks, mixer, scheduler, scripts, outputs and the UI. `--headless` runs it without a window.
- **Fixed-time strategy (chosen by the user):** shuffle the order first, then fill the gap
  from a pool of beds and jingles, then trim or fade only as a fallback, with a per-event
  tolerance.

## 6. Design

### 6.1 Approaches considered

| | Approach | Trade-off | Verdict |
|---|---|---|---|
| **A** | One app: engine and egui UI in one process, `--headless` flag | Simplest to build and ship. A UI crash takes the audio with it; remote control only through the local CLI/MCP | **Chosen by the user** |
| B | A separate engine process; the desktop and web UIs talk to it over WebSocket/CLI/MCP | A UI crash can't stop the audio, and remote/multi-operator use comes for free. Costs an API and state sync up front | Not chosen; §6.2 keeps the door open |
| C | Rust scheduling and UI driving Liquidsoap or GStreamer for the audio | Streaming is proven from day one. Weak control over exact timing and per-deck behaviour; a big non-Rust dependency | Rejected |

### 6.2 Workspace layout

```
crates/
  model/     items, sources, cue points, playlist, rules, events (pure data, serde)
  timing/    start-time estimation and fixed-time fitting (pure, deterministic)
  audio/     decoding, decks, mixer and buses, inputs, outputs, encoders, Icecast client
  library/   media library: folder scanning, tags, categories, cue points (SQLite)
  scheduler/ named playlists, pick orders and rules; merges them into the on-air timeline
  script/    JavaScript runtime (Boa), event hooks, sandbox
  engine/    ties everything together; owns state; the command API; persistence; as-played log
  control/   local control socket (JSON-RPC) used by the CLI and MCP
  mcp/       MCP server exposing the command API
  ui-egui/   decks, playlist, cartwall, meters, library browser, settings
  testkit/   fake clock, offline renderer, fixture media
apps/
  radio/     desktop app (eframe); `--headless` runs only the engine
  radio-cli/ CLI: talks to a running station over the control socket, or runs offline tools
```

**Command API (my call):** every action (load deck, play, insert item, fire cart, set rule,
skip, end a live item…) is a typed command in `engine`. The UI, scripts, CLI and MCP all go
through it, the same pattern the template uses for its tools. If approach B is wanted later,
the engine can move into its own process without the UI changing much.

**How the CLI reaches a running app (my call):** the app listens on a local control socket
(a Unix socket on Linux/macOS, a named pipe on Windows). It's for local control only, not a
network API.

### 6.3 Timing and fixed-time events

**The clock (my call, after review).**
- **Wall-clock time owns `fixed_at`.** That's the system's monotonic clock, kept accurate
  by NTP. A sound card's sample clock drifts by tens of ppm, which adds up to seconds per
  day, so it can't be the reference for exact times.
- **The audio runs on the master output device's sample clock.** If there's no sound card
  (headless, Icecast only), a timer paced by the system clock drives the audio instead.
- **The engine measures the gap between the two clocks continuously.** It feeds the
  measured offset into the start-time estimates, and the fill/trim steps absorb it.
- **Clock changes:** if NTP jumps the clock, the estimates re-anchor and the change is logged.
- **Time zone:** `fixed_at` is local time in the station's time zone. Events whose time
  doesn't exist at a DST change are moved to the next valid time; events whose time occurs
  twice take the first one.
- **Estimates** come from that clock plus each upcoming item's cue-in, segue (mix) point and
  crossfade length. They're recomputed whenever something changes and at least once a second.
- **Accurate starts:** every play / stop / fade command carries the sample time it should
  happen at, and the audio callback runs it at exactly that sample. That way, timing from the
  engine thread (10–50 ms jitter) never reaches the air.

**Item timing fields:** `cue_in`, `intro_end`, `segue` (where the next item starts),
`cue_out`, `fade_in`, `fade_out`. Files get defaults from silence detection when they're
imported (my call); editors can adjust them.

**Fixed events.** Any item can be marked `fixed_at: <time>` with a `tolerance` (default
±1 s, my call) and a `mode`:
- `soft`: it may start early or late within its tolerance.
- `hard`: it starts exactly on time. If the plan can't make it, the item on air is faded
  out at that moment.

**Fitting, in this order (agreed):**
1. **Reorder.** In the window between now (or the previous fixed event) and the fixed event,
   items marked `movable` (music by default; ads and pinned items are not) are reordered,
   and dropped or added from the rotation pool, to bring the start of the fixed event as
   close as possible. This is a bounded search (subset sum/knapsack over item lengths); rotation
   rules (artist separation, etc.) stay hard constraints.
2. **Fill.** A remaining gap is filled from a fill pool (beds, jingles, short instrumentals)
   tagged by length.
3. **Trim.** As a last resort, the last item before the event is faded early, or (for
   `soft` events) the event starts slightly late, within tolerance.
4. **Alarm.** If the result is still outside tolerance, raise a visible alarm and log it.
   A `soft` event that cannot start inside its tolerance still airs, late, as soon as the
   item in its way hands over.

**Locked items in the way (operator, 2026-11-22).** Locked items never move: not their
order and not their start time. A `hard` event still cuts in at its time, and a locked item
it overlaps starts right after it. A `soft` event never moves a locked item. When locked
items are in its way, it waits until the whole run of locked items in its way has ended (the
first point where it no longer overlaps a locked item), then airs there, late, with the
`FixedLate` alarm.

**When it re-plans (my call):** the fit is re-run whenever the playlist or timing changes. It
becomes *locked* N minutes before the event (default 10), and after that only steps 2–3
apply, so the on-air order stops shuffling just before the operator reads it.

**Fitting rules (my calls, after review):**
- **Stable plan:** a new plan replaces the current one only if it improves the fit by more
  than a threshold (default 0.5 s), so the visible playlist doesn't churn.
- **Frozen items:** items already loaded on a deck are never moved.
- **Order-dependent lengths:** an item's effective length depends on which item follows it
  (the segue and crossfade differ by pair), so the search scores whole orderings, not just a
  sum of lengths.
- **Hard events:** a `hard` fade *finishes* at the event time; it doesn't start there.
- **Ads are never trimmed or faded early.**
- **Live item of unknown length just before a fixed event:** only the hard cut at the event
  applies, and the UI shows a countdown to it.
- **v1 scope:** the reorder step only rearranges items already in the window. Adding or
  dropping items from rotation inside the search is deferred; the fill step covers the
  remaining gap.

**Live and stream items** have a planned length (for example "live show 60:00") used for
estimates. They end on an operator action, a command or script, a fixed event that follows,
or silence detection (my call).

### 6.4 Sources

- **File:** decoded with symphonia (MP3, AAC-LC/M4A, FLAC, WAV, Ogg/Vorbis).
  - Opus and HE-AAC fall back to libopus / fdk-aac if symphonia doesn't cover them; check
    this before building.
  - **Streaming decode:** each deck holds a ring buffer of a few seconds, refilled by the
    decoder pool, not whole files. If the ring underruns, the deck outputs silence and an
    alarm is raised; the callback never blocks.
  - **One sample rate:** the engine runs at one internal rate (default 48 kHz) and resamples
    everything at decode time.
  - Files are checked when loaded onto a deck (at least 30 s ahead). A missing or broken file
    is skipped with an alarm, never discovered on air.
- **Live input:** a sound-card device and channel pair (cpal). Usable as a playlist item or
  as an always-on mic/line channel with its own fader (my call).
- **Show (user):** a scheduled block that can last an hour or more. It can be a pre-recorded
  file (MP3 or any supported format), a live input or a stream. A file show uses its real
  length; live and stream shows use a planned length.
- **Internet stream:** HTTP / Icecast / Shoutcast (MP3, AAC, Ogg). It connects and buffers
  before its start time (pre-roll set per item). If it drops, it reconnects; if it stays
  down past a timeout, the playlist moves on and raises an alarm. **Keep-alive (user, 2026-11-21):** a URL item has `keep_alive` (default **on**): while it's on air, a dropped stream never makes the
  playlist fall through to the next track. The engine keeps reconnecting (backoff 1–30 s, no give-up)
  until the item's planned end or an operator/command ends it. While it's down, the fill pool's bed plays
  under a "stream reconnecting" alarm (silence only if there's no bed), and the silence detector doesn't skip a
  keep-alive item. With `keep_alive` off, the old timeout-then-move-on behaviour applies. ICY metadata from the
  stream is read and logged. The stream's clock differs from ours, so a drift-compensating
  resampler keeps its buffer level steady.

### 6.5 Decks, mixer and buses

- **Four decks, A–D.** The playlist assigns the next items to free decks in round-robin, and
  loads and pre-rolls each one at least 30 s ahead (my call). An operator can also load a
  deck by hand.
- **Buses (my call):**
  - Program: decks plus live inputs, which feeds the outputs.
  - Cart: the cartwall, mixed into Program after ducking.
  - Cue/PFL: pre-listening on a separate device, never on air.
- **Ducking:** a cart (or a voice input) can lower Program by a set dB, with attack and
  release times.
- **Program dynamics:** an optional compressor, then a lookahead limiter, on Program.
  - *Signal flow:* decks, carts and live input mix into Program; Program goes through the
    Out fader, then the compressor, then the limiter, and from there to the local output,
    the Icecast encoder and the Out meter. PFL/Cue is not processed.
  - *Defaults:* the limiter is on, with a −1 dBFS true-peak ceiling, 50 ms release and a
    fixed 1.5 ms lookahead (72 frames at 48 kHz), so Program has one known latency. The
    compressor is off; when turned on it starts at −18 dBFS threshold, 2.5:1, 15 ms
    attack, 250 ms release and 0 dB makeup (gentle broadcast levelling; the reasons are in
    `radio_model::CompressorSettings::default`). The compressor has a hard knee. With the
    limiter off, a soft clip keeps Program under −1 dBFS.
  - *Command:* `mixer.set_dynamics` with `compressor` {`enabled`, `threshold_db`,
    `ratio`, `attack_ms`, `release_ms`, `makeup_db`} and `limiter` {`enabled`,
    `ceiling_db`, `release_ms`}; absent fields keep their values. The engine refuses
    values outside the ranges in `radio_model::dynamics_range` with an error. Playout
    applies a change live and without a glitch: the control thread prepares the new
    processors; the audio thread plays the last 21 ms of input into them, so their
    lookahead and envelopes match, then crossfades old → new over 15 ms with both
    running, without allocating. Every chain has the same latency (without a limiter, a
    delay of the same length takes its place), so the two stay aligned. A change that
    comes during a fade waits for its end, and the last one wins.
  - *Where it lives:* stored in SQLite as its own row (part `dynamics`, key `program`);
    without a row, playout uses its configured default. In the UI it is Mixer ⚙ →
    Processing (the Out strip's menu links there).
  - *Gain reduction:* the meters carry no gain-reduction data yet, so no window shows a GR
    reading. A GR meter appears only when the processors report real values; the UI never
    draws an estimated or fake one.
  - *Offline renders:* Program is the lookahead late. An offline render of N timeline
    frames that ends calls `drain_offline` once and has N + latency frames, ending with the
    last input frame.
- **Loudness:** each file item on a deck gets a gain that brings it to a target level, so
  the limiter isn't doing the leveling (user, 2026-11-21: "transitions are smooth").
  - *Measure:* EBU R128 integrated loudness and true peak (ebur128 `TRUE_PEAK`, 4× oversampled),
    on the playout analysis lane. Playout measures the planned items that start within 15
    minutes (at most 12 entries), and any file an operator loads, so the result is normally
    ready before air. A silent or unreadable file is measured once per run and plays at unity.
    Files of the emergency fallback folder aren't station items, so they aren't measured; they
    use their tags only.
  - *Cache:* the engine keeps each measurement by file content hash (`settings_by_part`, part
    `loudness`, key = hash) with the item → hash map, so a file is measured once and a
    restart keeps it. Items with the same file share it. `item.set_loudness` stores one.
  - *Tag fallback:* until a file is measured, playout reads `REPLAYGAIN_TRACK_GAIN`/`_PEAK`
    (ReplayGain 2.0, reference −18 LUFS) or the Opus `R128_TRACK_GAIN` (Q7.8 dB, reference
    −23 LUFS) and converts it to the station target. With neither, the item plays at unity and
    the deck says "not measured".
  - *Gain:* target − loudness + the item trim, then the clamps: a boost never lifts the true
    peak over −1 dBTP (it limits a boost, never turns it into a cut); at most the max boost
    (default +12 dB) and at least −24 dB. A tag without a peak gets no boost. The clamps apply
    after the trim, so a trim can't defeat the peak guard. With normalisation off, or with no
    known loudness, the item plays at unity and its trim does not apply either.
  - *On the deck:* an item gain stage before the strip fader. The gain goes with the deck load
    and applies from the first sample. A change while the item is on air (a late measurement,
    a trim, a settings change) ramps linearly over 1.5 s; on a deck that isn't playing it
    changes at once. The render does no allocation for it.
  - *Settings:* station-level on/off (default on), target −30…−10 LUFS (default −18), max boost
    0…+12 dB, through `mixer.set_loudness` (stored as part `dynamics`, key `loudness`, applied
    live). In the UI: Mixer ⚙ → Processing → Loudness, above the compressor.
  - *Per item:* a manual trim (±12 dB) through `item.set_gain_trim`, undoable, in the metadata
    editor (Loudness → Gain trim).
  - *Shown:* each deck card shows "RG −3.2 dB" (measured), "tag −2.1 dB" (from a tag),
    "not measured" or "RG off", with a tooltip of the LUFS, the peak, the target, the trim and
    any clamp. It shows the gain that playout sent to the mixer, never one it didn't.
- **Several devices:** one output device is the master clock. Every other device (live
  input, PFL card) goes through a drift-compensating resampler. Losing a device triggers a
  re-open with an alarm.

### 6.6 Playlists and rules (one general mechanism)

**Everything is a named playlist (user).** You can create as many as you want and give each
a name: Jingles, Ads, Shows, News, Station IDs, Christmas promos, and so on. Ads aren't a
special feature; they're just another playlist with rules.

- **Main playlist:** exactly one playlist is the *main* one, the music shown on the left.
  When nothing else is due, it plays.
- **Inserted playlists:** every other playlist is shown on the right. Its **rules** decide
  when its items are woven into the on-air timeline.

**Folder-backed playlists (user, 2026-11-21).**
- **Any folder can be a playlist.** Any folder can be added to the library, and a folder
  becomes a playlist whose items are that folder's audio files, optionally including
  subfolders. The usual workflow: build a playlist from a folder, then use it as the main
  playlist.
- **Keeping it in sync:** a folder-backed playlist re-syncs when the folder changes ("watch
  for changes"). New files are added, removed files are dropped, and cue points and loudness
  are analysed in the background.
- **Model:** `Playlist.source: Option<FolderSource { path, recursive, watch }>`.
- **Rules apply as usual:** a folder-backed playlist can carry rules like any other, so a
  folder of jingles with "every 3 tracks" works as well.

**A playlist has:**
- a name and colour, and on/off;
- its items (files, live inputs, streams, shows);
- a **pick order** for which of its items plays next: in order, shuffle, weighted, or
  least-recently-played;
- one or more **rules**.

**A rule has a trigger, a placement and constraints.**
- **Trigger:**
  - every N tracks of the main playlist;
  - every N minutes;
  - at clock positions (":20 and :50");
  - at a fixed date/time, optionally repeating (daily, weekdays, weekly);
  - on an event fired by a script.
- **Placement:**
  - between tracks (the default);
  - fixed-time `soft` or `hard`, with a tolerance (§6.3);
  - how many items to take per trigger (for example "3 ads per break").
- **Constraints, on the rule or on any playlist:**
  - never back to back with the same playlist, or with a named other one;
  - minimum gap between plays of one item;
  - artist / title separation (mainly for the main playlist);
  - active dayparts and valid-from / valid-to dates;
  - a cap on plays per day or per hour.
- **Item validity:** any item in any playlist can carry valid-from / valid-to dates and a
  daily play cap. That covers ad campaigns without making ads special.

**Rules fill the upcoming timeline ahead of time (agreed),** so their results are visible and
editable before they air. When two rules want the same slot, a playlist's priority decides
the order (my call); a `hard` fixed-time rule always wins.

**Rotation for the main playlist** is the same mechanism: its pick order (weighted by
category per daypart), plus separation constraints. Simple hour templates (an hour as a
list of slots) are a rule type that can come later.

**Proof of airing:** the as-played log records which playlist and rule every item came
from. Exports are CSV for the whole log and a scheduled-vs-aired report per playlist (my
call), which covers ad proof without an ads-specific feature. Importing from a traffic
system is deferred.

**Rules are JavaScript (user, 2026-11-21).** A playlist's rules *are* a script. There's
nothing special about them: a person, or an agent, can edit them by hand. The visual rule
editor (⚙ on each playlist) is only a convenient way to write that script. It generates a
declarative form:

```js
pick("shuffle");
rule({ at: { minutes: [20, 50] }, place: "between", take: 3,
       constraints: [notAdjacentToSelf(), maxPerDay(24)] });
```

- **How the script runs (my call):** the rules script runs in the embedded JS engine
  (rquickjs, §6.8) when it's saved or loaded, never per track. Its `rule(...)` / `pick(...)`
  calls produce the same typed `Rule` / `PickOrder` data the scheduler uses. This keeps the
  scheduler pure Rust and deterministic, and the time and memory limits from §6.8 apply.
- **Hand edits:** a script that has been edited by hand stays the source of truth. The
  visual editor shows it as "edited by hand" and only overwrites it if asked.
- **Custom logic:** procedural logic (functions that decide insertions at run time) goes
  through the event hooks in §6.8. It's deferred past v1, and its results are still checked
  against the same constraints.

### One-off scheduled items (Ruling 38)

A one-off plays one source at a set local time, from **Add one-off** in Upcoming or the
command `schedule.add`.

- **Source:** an audio file, a library item, a URL stream (keep-alive and a planned length)
  or the live input (a planned length). These are the existing item sources.
- **Start:** a local date and time in the station time zone. The dialog starts at the next
  full hour. The check uses jiff: a time that the clocks skip (DST gap) is an error; a time
  that happens twice (DST fold) airs the first time, and the dialog and the command say so.
  A one-off with no repeat cannot start in the past.
- **Timing:** `hard` (cuts in at the time) or `soft` with a tolerance (default ±1 s). The
  normal fit (reorder → fill → trim) places it, and the fitter never drops it.
- **Repeat:** none, daily, Monday to Friday or weekly, as the At trigger has them.
- **Storage:** the system playlist "One-off" holds the items. It has no tab, its items are
  not library files, and playlist commands that would delete, rename, switch off or edit it
  fail. Each item has one At rule with the constraint `onlyItem("<id>")`, so the rule plays
  that item only. The engine writes the rules script from the rules after each change, so a
  person or an agent can read and edit it like any other script.
- **Commands:** `schedule.add` (returns the item id), `schedule.update` and `schedule.remove`,
  with Undo and Redo. `radio-cli cmd --data-dir <folder> '<command JSON>'` runs them on a
  station folder while the app is closed.
- **After air:** the as-played log and History keep the airing. Then playout removes a
  one-off with no repeat and its rule. This is no undo step. A one-off with no repeat that
  passed its time and tolerance without air (for example, the app was closed) raises the
  alarm `OneOffMissed`, and then it leaves too. The clean-up waits while a one-off is on a
  deck or starts soon, because it renumbers the rules of the playlist.

### 6.7 Cartwall

- Pages of carts (my call: a 4×4 grid per page, several pages, keyboard shortcuts).
- A cart plays on the Cart bus, over whatever is on air, independent of the decks and the
  playlist.
- Per cart: file, gain, optional ducking amount, mode (one-shot / loop / restart on press),
  and a colour.
- Several carts can play at once; one key stops all carts.
- Fired carts are written to the as-played log.

### 6.8 JavaScript scripting

- **Engine (my call, after review):** QuickJS through `rquickjs`, because it can interrupt a
  running script and cap its memory; Boa is probably missing both. It's a C dependency,
  like the encoders. Switch to Boa only if checking shows it really supports both. The
  runtime runs on its own thread, never on the audio callback or the engine thread.
- **Scripts can only act through the command API.** They can't touch audio buffers. So the
  worst a script can do is send bad commands, which the engine checks.
- **Events available to scripts:** item started / ended, hour / minute boundary, fixed event
  locked, input level crossed a threshold, silence detected, stream dropped, cart fired, and
  a timer.
- **Script API:** read the playlist and the library; insert / move / remove items; fire carts;
  set deck gains; schedule a timer; log; add a custom rule.
- **Safety:** a time budget per call (script interrupted when it's exceeded), a memory limit,
  and no file or network access unless granted in settings. Errors are shown in the UI and
  logged; a script that keeps failing is turned off.

### 6.9 Outputs

- Local audio device(s) through cpal; more than one output allowed (for example main plus
  a backup sound card).
- Icecast source client (HTTP PUT / SOURCE). Formats: Ogg/Opus and MP3. MP3 uses craft-audio's
  native encoder (pure Rust, MIT OR Apache-2.0) behind the `mp3` feature. Ogg/Opus uses libopus,
  a C dependency and an accepted exception to pure Rust.
- **v1 scope (my call, after review):** one local output device plus PFL, and one Icecast
  mount. More devices and mounts are deferred; the design allows them.
- **Now-playing metadata:**
  - It's updated on music items only; jingles and ads keep the previous title.
  - MP3 uses Icecast's admin metadata endpoint; Ogg/Opus uses in-band comments
    (chained streams).
  - Uses HTTP PUT, falling back to SOURCE for old servers. Auto-reconnect with backoff.
- The encoders run on their own threads fed from a ring buffer, so a slow network never
  stalls the program audio. If the ring overflows, the stream is dropped and reconnected;
  the program audio never waits.

### 6.10 Threads and reliability in a single process

| Thread | Job | Rule |
|---|---|---|
| Audio callback | mix decks / inputs / carts, write outputs | real-time: no allocation, no locks, no I/O; talks only through lock-free queues (e.g. `rtrb`) |
| Engine | state, commands, timing, scheduler | never blocks on UI, scripts or network |
| Decoder pool | decode files and streams into deck buffers | |
| Script | Boa runtime | time-boxed calls |
| Encoders / Icecast | encode and send | one thread per output |
| UI | egui | reads snapshots of engine state; sends commands |

**What approach A means for reliability (accepted):** a crash anywhere stops the
audio. The mitigations:
- Keep the audio thread minimal.
- UI panics are caught at frame level where possible.
- `--headless` for stations that want to run without a UI.
- A watchdog / service supervisor (systemd, launchd, a Windows service) restarts the app.
- On restart, playback resumes at the item that should be playing now (state is persisted
  continuously, §6.11).

### 6.11 Persistence and logs

- SQLite (my call) for the library, playlists, rules, carts, scripts and settings.
- Engine state (the playlist and the current items) is written every second on a background
  writer thread, using SQLite in WAL mode, never on the engine thread.
- **Resuming after a crash (my call, after review):**
  - Music and jingles resume at the item that *should* be playing now, at the position
    it would have reached, based on wall-clock time.
  - An ad cut off mid-way is played again from the start, and the cut-off airing is logged
    as `interrupted`.
  - A live input or stream resumes straight away.
- **As-played log (my call):** every item that went to air, with its real start and end
  time, source and type (music/jingle/ad), exportable to CSV. It's needed to prove ads ran
  and for music royalty reporting. A row is written when the item starts and completed when
  it ends, so a crash leaves a row that's clearly marked unfinished.

### 6.12 UI (desktop, egui)

**The playlist view has two panes side by side (user):**
- **Left, Main:** the main (music) playlist, a long list of songs that can run to thousands
  of rows.
- **Right, everything inserted:** the other named playlists (Jingles, Ads, Shows, News, …),
  as many as you create.
- **Both panes show estimated start times.** They're two views of one on-air timeline, so
  their times always agree.

```
┌ Decks ─────────────────────────────────────────────────────────────────────┐
│ [A ▶ 2:41/3:41 ▓▓▓▓▓░░] [B ⏸ ready] [C ⏸ ready] [D — empty]   Mode: AUTO   │
├ Main: Music ───────────────────────┬ [Upcoming] Jingles Ads Shows News [+] ┤
│ 14:52:10  Artist – Song      3:41  │ 14:55:48  ● Jingles  Station ID  0:06  │
│ 14:55:54  Artist – Song      4:02  │ ⏱15:00:00 ● News     stream      5:00 ✓│
│ ── 15:00:00 News (5:00) ────────── │ 15:20:00  ● Ads      3 items     2:30  │
│ 15:05:00  Artist – Song      3:12  │ ⏱16:00:00 ● Shows    Jazz Hour  58:40  │
│ 15:08:12  Artist – Song      3:55  │            fit: −0.4 s ✓ (locked)       │
│ … (thousands of rows, virtualised) │ [+ Add item]                           │
├ Cartwall (page 1/3) ───────────────┴───────────────────────────────────────┤
│ [ID 1] [ID 2] [Sweep] [Horn] [Laugh] [Bed] …            [■ stop all carts] │
├ Meters / outputs: Local ● Icecast /live ● 128k ● · Alarms · Script log     ┤
└────────────────────────────────────────────────────────────────────────────┘
```

**Details (my calls):**
- **Right pane tabs:**
  - **Upcoming** (the default) merges every inserted playlist into one list sorted by time.
    Each row is coloured by the playlist it came from and shows its start time and fit
    status.
  - Each playlist also has its own tab, which shows its items, pick order and rules, with the
    next planned time of every item.
  - **[+]** creates a new named playlist.
- **"+ Add item"** in a playlist tab adds an item (a file, live input, stream or show). In
  *Upcoming*, **Add one-off** adds a one-off timed item: source, time, `soft`/`hard`,
  tolerance and repeat (see "One-off scheduled items" in §6.6). You can also drag items from
  the library onto a time.
- **Rule editor:** each playlist tab has a rules section, built from the triggers,
  placements and constraints in §6.6, using drop-downs. No code is needed.
- **Main pane:** shows a thin divider row wherever an inserted item cuts into the music, with
  its time and length, coloured by its playlist. The list only draws the visible rows.
- **Selection is linked:** selecting a row in one pane highlights its place in the other.
- **Editing:** main rows are dragged to reorder, or dragged in from the library. Live / stream
  items have an "end now" button.

**Tool windows (user, 2026-11-21).** The cartwall, mixer, clock and history are separate
windows, each its own OS window (egui viewport) that can go on a second screen. They're opened
from a Windows menu in the deck strip, and the main window keeps the decks and the two playlist
panes.
- **Cartwall:** pages of carts, and stop-all.
- **Mixer:** a strip each for players A–D, the cart bus, live input and Out, with faders,
  stereo VU meters with peak hold and numeric dB, and mute/PFL.
- **Clock:** a large clock that scales with the window, plus countdowns to the next fixed
  event and to the end of the current item.
- **History:** the as-played log (aired at, playlist, title, length, origin), newest first,
  filterable by text and playlist.
- **Library:** a folder/category tree plus a searchable table. Items are dragged onto the Main
  pane, onto a playlist or onto a cart; files dropped in from the OS file manager are imported.

**Settings live with their part (user, 2026-11-21). There is no general settings dialog.**
Every window and pane has its own ⚙ for that part only:
- **Mixer:** audio devices, sample rate, routing and meters. The Out strip's ⚙ holds the
  Icecast mount.
- **Cartwall:** grid, pages and ducking.
- **Clock:** format and countdowns.
- **History:** columns and range.
- **Library:** watched folders and categories.
- **Playlists:** pick order and rules.

The ⚙ on each playlist opens its rules editor, with Visual and Script tabs (§6.6).

**Deck controls and metadata (user, 2026-11-21).**
- **Deck controls (revised 2026-11-21):** each deck card has one button: *Play this deck*
  fades the deck in (its ⚙ fade-in, 0.5 s by default) while the item on air fades out
  (`deck.play` with `fade_in`); on the deck on air it is Pause / Resume. There is no per-deck
  Next or Skip. The header has the one *Play next* (⌘⇧N), beside the mode control: it plays
  the NEXT 1 deck the same way, and in Assist mode it is the Next. *Fade out* (⌘⇧F) sits
  beside it. These arm and need a second press only when they would cut the item on air
  before its end. The plan goes on from the deck played; the items that waited before it
  leave their decks and return to the rotation for a later turn, so nothing plays twice.
- **Metadata editing:** any item's file tags (title, artist, album, year, genre, comment)
  can be edited and written back to the file, via lofty.
- **Exact control over the stream title:** every item also has a *publish mode* that
  decides exactly what Icecast receives:
  - From tags: "Artist – Title".
  - Custom text: a template like `{artist} – {title}`.
  - Keep previous: the default for jingles and ads.
  - Hide: sends the station name.
- **Where it's set:** the metadata window shows a live preview of the exact string.
- **Model:** `Item.meta: ItemMeta { title, artist, album, year, genre, comment, publish, custom }`.

### 6.13 Testing

- The `timing` and `scheduler` crates are pure and deterministic: they're tested with a fake
  clock and fixture lengths, including property tests (proptest, as in the template) for the
  fitting algorithm.
- `testkit` renders a playlist offline, faster than real time, to a WAV plus the as-played log,
  so tests can check that the real start times match the estimates to within a few
  milliseconds.
- Soak test: a 24-hour accelerated offline run with rules and scripts. A fake device clock
  skewed by ±100 ppm checks that drift is absorbed and no fixed event is missed. There's also
  a real-hardware soak of at least 24 h before release.
- UI tests with `egui_kittest`, as in the template.

### 6.14 Dead air, alarms and control safety (added after review, my calls)

- **Silence detector on Program:** after N seconds of silence (default 15), it skips to the
  next item. If that fails too, it switches to an emergency fallback playlist (a local folder
  that's always available).
- **Off-site alarms:** dead air, a fixed event missed, the stream dropping and a device being
  lost are sent to a webhook (and email through it), as well as shown on screen.
- **Ending a live item on silence** is opt-in per item, with a long threshold (default 60 s),
  because quiet speech would trigger it.
- **Control socket permissions:** only the station's OS user can open it. Commands come in two
  tiers, *read-only* and *on-air*; MCP gets read-only by default.
- **Library:** moved files can be relinked by content hash. The playlist view only draws the
  rows on screen, so a 24-hour playlist stays fast.

### 6.15 Operating modes and hand edits (my calls, open question not yet answered)

- **Auto:** the playlist advances by itself.
- **Assist:** each next item waits for the operator to press Next. Fixed `hard` events still
  fire on time.
- **Manual:** nothing starts automatically except `hard` fixed events.
- **Stop after current** works in every mode.
- The desktop UI always runs Auto; Assist/Manual stay in the engine for the CLI/MCP.
- **Hand edits are locked:** any item an operator adds or moves by hand is locked, and the
  fitting step (§6.3) never moves or drops it: not its order and not its start time. A `soft`
  event waits for the locked items in its way (§6.3); only a `hard` event moves one. The lock
  shows as a pin icon and can be removed.

## 7. Left to the builder

- Exact crate choices beyond those named (resampling, ring buffer, SQLite binding, Icecast
  HTTP client).
- The exact fitting search algorithm and its limits, as long as it follows the order in §6.3
  and stays fast enough to re-run every second.
- Default values marked as my call (tolerances, lock-in time, pre-roll, grid size).
- Command naming and how the CLI / MCP tools map onto them.
- Build order and packaging, following the template's CI and release setup.
