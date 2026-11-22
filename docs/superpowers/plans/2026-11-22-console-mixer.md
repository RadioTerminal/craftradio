# Console Mixer (SND monitor bus and output meters) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace PFL with a per-strip SND monitor send, make the Program sound card optional (None), and give Icecast, WebRTC and the Program device their own meters, shown as console strips.

**Architecture:** The mixer's PFL bus becomes the SND (monitor) bus, plus a new Program SND bit that adds the post-dynamics Program to it; the cue output becomes the Monitor output. The engine drops `Route`, migrates old rows, and stores `snd`. A small lock-free `LevelMeter` in `radio-stream` is fed by each output's consumer (Icecast sender, WebRTC encoder, Program device callback); `ui-engine` reads them and gates them on each output's status so a down output never shows levels.

**Tech Stack:** Rust (workspace, mise), egui 0.37 (storytold fork), rtrb, serde, rusqlite.

**Spec:** `docs/superpowers/specs/2026-11-22-console-mixer-design.md`

## Global Constraints

- Run cargo as `mise exec -- cargo …`. Per task: `cargo fmt --all`, `cargo test -p <crates>`, `cargo clippy -p <crates> --all-targets -- -D warnings`.
- Layers: a crate depends only on lower layers. `radio-stream` must not gain a dependency on `radio-audio`.
- Panels change engine state only through commands; every setting lives in Preferences.
- No placeholder shown as success: an output that is off, down or has no device shows its state in words and empty bars.
- Colours, sizes, gaps and fonts only from theme tokens in `crates/ui-egui/src/theme.rs`.
- State never hangs on colour alone: toggles change word and fill ("MUTED", "SND ON").
- Render thread stays allocation-free (`assert_no_alloc` test in `crates/audio/tests/it/mixer.rs` must keep passing).
- Integration tests live in each crate's `tests/it/` binary. Never weaken or delete a test to pass; rename/update tests whose subject was renamed (PFL → SND).
- Existing stations keep their air feed: a missing or empty `audio.out` still means System default.
- Commit messages: plain English, end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- Program SND while Program is muted: the monitor must be silent from Program (post-mute). Test in Task 2.
- Several SNDs on at full level: the monitor must not exceed ±1.0. Test in Task 2.
- An old station row `{"pfl":true,"route":"CueOnly"}` loads as mute + SND, and the station still opens. Test in Task 3.
- A script or MCP client sending the old `pfl` field still works; sending `route` gets a clear error, not silent success. Test in Task 3.
- Program device under fallback (card lost → paced): its strip shows no level even though `ProgramSource` keeps rendering. Test in Task 5.

---

### Task 1: Rename PFL to SND in audio and playout (no behaviour change)

**Files:**
- Modify: `crates/audio/src/mixer.rs` (`Shared.pfl`, `frame`, `strip`, `render`, `render_offline_buses`, `MixerHandle::set_pfl`)
- Modify: `crates/playout/src/output.rs`, `crates/playout/src/cue.rs` (rename module to `monitor.rs`, type `Cue` → `Monitor`), `crates/playout/src/core.rs`, `crates/playout/src/ops.rs`
- Test: `crates/audio/tests/it/mixer.rs`, `crates/playout/src/cue.rs` unit test

**Interfaces:**
- Produces: `MixerHandle::set_snd(&self, strip: Strip, on: bool)`; `Mixer::render(&mut self, program: &mut [f32], monitor: &mut [f32])`; `radio_audio::render_offline_buses(mixer, frames) -> (Vec<f32> /*program*/, Vec<f32> /*monitor*/)`; `playout::monitor::Monitor` (was `cue::Cue`); `PlayoutView.monitor_open: bool` (was `cue_open`).

- [ ] **Step 1:** Rename `pfl_is_pre_fader_and_never_added_to_program` → `snd_is_pre_fader_and_never_added_to_program`, `captured_deck_uses_live_strip` calls, and the cue unit test (`monitor_device_source_reads_only_the_snd_ring`) to the new names. Run `mise exec -- cargo test -p radio-audio` → FAIL (no `set_snd`).
- [ ] **Step 2:** Rename in code as listed under Interfaces; doc comments say "SND (monitor)" not "PFL". `set_snd` still ignores `Strip::Out` in this task.
- [ ] **Step 3:** `mise exec -- cargo test -p radio-audio -p radio-playout` → PASS; clippy clean.
- [ ] **Step 4:** Commit "Rename the PFL bus to the SND monitor bus".

### Task 2: Program SND and a clip-safe monitor

**Files:**
- Modify: `crates/audio/src/mixer.rs` (`set_snd`, both branches of `render`)
- Test: `crates/audio/tests/it/mixer.rs`

**Interfaces:**
- Consumes: Task 1 names.
- Produces: `set_snd(Strip::Out, true)` adds the final Program frame (after dynamics/limit, after the Out fader/mute) to the monitor frame of the same index. Every monitor frame passes through the existing `limit()`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn program_snd_puts_what_listeners_hear_on_the_monitor() {
    // deck A at 0.25 constant, playing, fader 0 dB; set_snd(Strip::Out, true)
    let (program, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    assert_eq!(&monitor[1024..], &program[1024..]); // same samples once settled
}

#[test]
fn program_snd_is_silent_while_program_is_muted() {
    // as above, then SetGain { strip: Strip::Out, db: NEG_INFINITY }
    let (_, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    assert!(monitor.iter().skip(512).all(|x| x.abs() < 0.001));
}

#[test]
fn monitor_never_exceeds_full_scale_with_many_snds() {
    // decks A–D each at 0.9, all set_snd(.., true), plus set_snd(Strip::Out, true)
    let (_, monitor) = radio_audio::render_offline_buses(&mut mixer, 1024);
    assert!(monitor.iter().all(|x| x.abs() <= 1.0));
}

#[test]
fn no_snd_means_a_silent_monitor() {
    // deck A playing, nothing on SND
    assert!(monitor.iter().all(|x| *x == 0.0));
}
```

Also run once with `mixer.set_dynamics(Some(ProgramDynamics::default()))` for the first test (Program SND equals post-dynamics Program, frame for frame).

- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement: `set_snd` accepts `Strip::Out` (bit 6). In both `render` branches, after a Program frame `y` is final, add it to the monitor frame at the same index when the Out bit is set; then apply `limit` to each monitor frame. In the dynamics branch the monitor frames for the block are filled in the first loop; index them by block offset rather than consuming an iterator so the second loop can add `y`.
- [ ] **Step 4:** `mise exec -- cargo test -p radio-audio` → PASS (including the no-alloc test).
- [ ] **Step 5:** Commit "SND on Program plays the on-air mix on the monitor".

### Task 3: Engine — `snd` replaces `pfl` and `route`, with migration

**Files:**
- Modify: `crates/engine/src/lib.rs` (`StripState`, `MixerSetStrip` handling, `AudioIntent::SetStrip`, `State::fresh`, post-load migration), `crates/engine/src/command.rs` (`Route` removed, `MixerSetStrip` fields, description), `crates/engine/src/store.rs` (strip row load)
- Modify: `crates/playout/src/ops.rs` (`cue_device` → `monitor_device` reading `audio.monitor`; `sync_strips` and SetStrip use `snd`), playout tests that build `MixerSetStrip`
- Modify: `docs/control.md` and `docs/2026-11-21-radio-station-manager-design.md` wherever they describe PFL/route
- Test: `crates/engine/tests/it/main.rs`

**Interfaces:**
- Produces:
  - `pub struct StripState { pub gain_db: f32, pub mute: bool, pub snd: bool }`, `Deserialize` via a private `StripRow { gain_db, mute, #[serde(default)] snd, #[serde(default)] pfl: bool, #[serde(default)] route: Option<LegacyRoute> }` with `From<StripRow>`: `pfl` → `snd`; `CueOnly` → `mute = true, snd = true`; `OutAndCue` → `snd = true`; `Out` → nothing.
  - `Command::MixerSetStrip { strip, gain_db: Option<f32>, mute: Option<bool>, #[serde(alias = "pfl")] snd: Option<bool>, #[serde(default, skip_serializing)] #[schemars(skip)] route: Option<serde_json::Value> }`. A present `route` is refused: `"route is gone: use mute and snd"`.
  - `snd: Some(true)` without a non-empty `audio.monitor` is refused: `"Monitor output is not available; choose a monitor device first"`.
  - `pub const MONITOR_KEY: &str = "monitor"`, `pub const OUT_KEY: &str = "out"`, `pub const NO_DEVICE: &str = "none"` in `radio_engine`.
  - On load: settings `audio.pfl` moves to `audio.monitor` (only if `monitor` is absent), and is removed.
  - `State::fresh()` sets `audio.out = "none"`.
  - `AudioIntent::SetStrip { strip, gain_db, mute, snd }`.

- [ ] **Step 1: Failing tests** (replace `unavailable_cue_output_is_rejected_without_showing_pfl_on`)

```rust
#[test] fn snd_without_a_monitor_device_is_refused_and_not_shown_on() { /* err contains "Monitor output is not available"; state A.snd false */ }
#[test] fn old_pfl_field_still_turns_snd_on() { /* settings audio.monitor = "Dev"; dispatch JSON {"type":"mixer.set_strip","strip":"A","pfl":true}; A.snd true */ }
#[test] fn a_route_field_is_refused_with_a_message() { /* err contains "route is gone" */ }
#[test] fn old_strip_rows_load_as_mute_and_snd() {
    // write rows via raw SQL into settings_by_part ('mixer','A','{"gain_db":0,"mute":false,"pfl":true,"route":"Out"}'),
    // ('mixer','B', ...route "CueOnly"), ('mixer','C', ...route "OutAndCue"), reopen
    // A: snd; B: mute && snd; C: snd && !mute
}
#[test] fn old_cue_device_setting_becomes_the_monitor_device() { /* audio.pfl = "Dev" → after reopen audio.monitor == "Dev", no audio.pfl */ }
#[test] fn a_new_station_has_no_program_device() { /* open_in_memory: audio.out == "none" */ }
#[test] fn an_existing_station_without_out_keeps_system_default() { /* db with a saved state but no audio.out row → after reopen, audio.out absent */ }
```

Use the JSON command entry the engine already has for CLI/MCP dispatch for the alias test. For the reopen tests use a temp file path (`Engine::open`), `flush()`, drop, reopen.

- [ ] **Step 2:** Run `mise exec -- cargo test -p radio-engine` → FAIL.
- [ ] **Step 3:** Implement per Interfaces. Migration runs in `Engine::start` after `load_state` and marks the state dirty only if it changed something.
- [ ] **Step 4:** Update playout callers; `mise exec -- cargo test -p radio-engine -p radio-playout` → PASS; clippy clean.
- [ ] **Step 5:** Commit "Mixer strips have SND instead of PFL and routes; old stations migrate".

### Task 4: Program device can be None

**Files:**
- Modify: `apps/radio/src/main.rs:192-201`
- Test: `apps/radio` unit test of a new pure fn, or `crates/ui-engine` if the mapping lives there

**Interfaces:**
- Consumes: `radio_engine::{OUT_KEY, NO_DEVICE}`.
- Produces: `pub fn output_choice(out_setting: Option<&str>, null_flag: bool) -> OutputChoice` in `radio_ui_engine` (next to `AUDIO_PART`): `null_flag` or `Some("none")` → `Null`; `None` or `Some("")` → `Device { name: None }`; else `Device { name: Some(..) }`.

- [ ] **Step 1:** Test `output_choice` for the four cases above → FAIL.
- [ ] **Step 2:** Implement; `main.rs` uses it.
- [ ] **Step 3:** Tests pass; commit "The Program sound card can be None: the station keeps time without one".

### Task 5: Output level meters

**Files:**
- Create: `crates/stream/src/meter.rs`
- Modify: `crates/stream/src/lib.rs` (export), `crates/stream/src/output.rs` (`IcecastClient::start` + `send`), `crates/stream/src/webrtc.rs` (`start_shared`, media loop at the encode site ~line 475), `crates/playout/src/output.rs` (`ProgramSource`), `crates/playout/src/core.rs:340,369,496`, `crates/playout/src/streams.rs:323`, `crates/playout/src/lib.rs` (accessor)
- Test: `crates/stream/src/meter.rs` unit tests, `crates/stream/tests/it/relay.rs` or a new `crates/stream/tests/it/meter.rs`, `crates/playout/tests/it/streams.rs`

**Interfaces:**
- Produces:
  - `pub struct LevelMeter` (two `AtomicU32` holding the max |sample| per channel since the last take): `LevelMeter::default()`, `fn add(&self, interleaved_stereo: &[f32])`, `fn take(&self) -> [f32; 2]` (dBFS, floor −120, resets to silence). No allocation, no lock.
  - `IcecastClient::start(cfg, frames, meter: Arc<LevelMeter>)` meters exactly the samples it hands to the encoder.
  - `WebRtcServer::start_shared(cfg, tap, meter: Arc<LevelMeter>)` meters the samples it encodes, i.e. only when at least one listener is connected.
  - `ProgramSource { device_meter: Arc<LevelMeter>, .. }` meters what it writes to the device buffer.
  - `pub struct OutputMeters { pub device: Arc<LevelMeter>, pub icecast: Arc<LevelMeter>, pub webrtc: Arc<LevelMeter> }` and `Playout::output_meters(&self) -> OutputMeters` (same Arcs for the life of the playout, reused across Icecast/WebRTC restarts).

- [ ] **Step 1: Failing tests**

```rust
#[test] fn take_reports_the_loudest_sample_per_channel_then_falls_to_silence() {
    // add [0.5, 0.25, -1.0, 0.1] → take ≈ [0.0 dB, -12.04 dB]; next take == [-120.0; 2]
}
// playout, offline: deck playing, Icecast pointed at a closed port (never connects)
#[test] fn icecast_meter_stays_silent_while_not_connected() { /* after rendering 1 s, icecast.take() == [-120.0; 2] while Out meter > -40 dB */ }
#[test] fn webrtc_meter_is_silent_with_no_listeners() { /* server on, 0 listeners: take() == [-120.0; 2] */ }
```

If the stream integration tests already run a local Icecast stub (`relay.rs`), add `icecast_meter_follows_program_while_connected` there: take() > −40 dB after audio flows.

- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement per Interfaces; `add` uses `fetch_max` on the `f32` bits of `|x|` (non-negative floats order like their bits).
- [ ] **Step 4:** `mise exec -- cargo test -p radio-stream -p radio-playout` → PASS; clippy clean.
- [ ] **Step 5:** Commit "Icecast, WebRTC and the Program sound card each have a level meter".

### Task 6: View-model and adapter (`ui-egui` view, mock, `ui-engine`)

**Files:**
- Modify: `crates/ui-egui/src/view.rs` (`MeterView`, `MeterFrame`, `StripChange`, remove `Route`, `DeviceRole::Pfl` → `Monitor`, `DeviceRoles.pfl` → `monitor`, `StationView.cue_open` → `monitor_open`, new `OutputMeterView`)
- Modify: `crates/ui-egui/src/mock/{mod,decks,edit}.rs`
- Modify: `crates/ui-engine/src/lib.rs` (`meters`, `strip_label`, `device_roles`, `MeterFrame` read, output meters), `crates/ui-engine/src/edit.rs:1126,1318-1335`, `crates/ui-engine/src/fader.rs:172`
- Test: `crates/ui-engine` unit tests (module `tests` in the file that holds `output_meter_views`)

**Interfaces:**
- Produces:
  - `MeterView { strip: String /*engine id: A..D, Cart, Live, Out*/, label: String /*shown: "Program" for Out*/, level_db, peak_db, gain_db, mute, snd: bool }`.
  - `StripChange::{Gain(f32), Mute(bool), Snd(bool)}`.
  - `pub enum OutputKind { Device, Icecast, WebRtc }`; `pub struct OutputMeterView { pub kind: OutputKind, pub label: String, pub live: bool, pub note: String }`; `StationView.output_meters: Vec<OutputMeterView>` in that order.
  - `MeterFrame { level_db, peak_db, at_sample, output_level_db: Vec<[f32; 2]> }` in `output_meters` order; the UI ballistics treat them like strip levels.
  - `pub(crate) fn output_meter_views(out: &OutputStatus /*playout*/, local: &LocalOutput) -> Vec<OutputMeterView>` in `ui-engine`: `live` only when Device is `LocalOutput::Device`, Icecast is connected, WebRTC is running with ≥1 listener. Notes: "No device" (None), "Fallback: <reason>"/"Lost: <name>", "Not connected", "Off", "On — 0 listeners", "<n> listeners", or the device name.
  - The adapter puts `[-120.0; 2]` in `output_level_db` for any output whose view is not `live` (bars never show under a down output, even if its meter saw samples).

- [ ] **Step 1: Failing tests**

```rust
#[test] fn program_device_under_fallback_is_not_live() { /* LocalOutput::Lost/Null → live false, note starts "No device" or "Lost" */ }
#[test] fn icecast_not_connected_is_not_live_and_says_so() { /* note == "Not connected" */ }
#[test] fn webrtc_with_no_listeners_is_not_live() { /* note == "On — 0 listeners" */ }
#[test] fn a_live_output_passes_its_level_through() { /* Device + level [-6,-6] → frame has [-6,-6] */ }
#[test] fn a_down_output_shows_silence_even_with_a_level() { /* Icecast down + level [-6,-6] → frame has [-120,-120] */ }
```

- [ ] **Step 2:** Run `mise exec -- cargo test -p radio-ui-engine` → FAIL.
- [ ] **Step 3:** Implement; mock simulates output meters (Device "No device" by default, Icecast "Not connected", WebRTC "Off") so the preview never shows a live output it doesn't have.
- [ ] **Step 4:** `mise exec -- cargo test -p radio-ui-egui -p radio-ui-engine` → PASS (fix compile errors in `mixer.rs`/`preferences.rs` minimally here; the drawing changes are Task 7); clippy clean.
- [ ] **Step 5:** Commit "The mixer view-model has SND and output meters".

### Task 7: Console strips and Preferences

**Files:**
- Modify: `crates/ui-egui/src/mixer.rs` (`show`, `strip_ui`, `meter_bars`, `SCALE_DB`, new `output_strip`, `devices_tab`, remove `routing_tab` and `SettingsTab::Routing`, `strip_menu`)
- Modify: `crates/ui-egui/src/preferences.rs` (`Page::Routing` removed; its search words "routing route cue" join Devices; Devices search words say "program device monitor device")
- Modify: `crates/ui-egui/src/theme.rs` only if a new token is needed (e.g. `size::OUTPUT_STRIP_W`, a separator gap)
- Test: `crates/ui-egui/tests/it/` (new `mixer.rs` module in the `it` binary, using the existing test harness the other modules use)

**Interfaces:**
- Consumes: Task 6 view types.
- Behaviour:
  - Source and Program strips: name row (⚙ on Program opens Devices), L/R bars + scale beside the vertical fader, level/peak readout, then `Mute` / `SND` toggles. Words: "Mute"/"MUTED", "SND"/"SND ON". SND disabled with hover "SND needs a present monitor device and an open monitor output" when `!(monitor_open && devices contains device_roles.monitor)`.
  - Scale marks: `SCALE_DB = [0, -3, -6, -9, -12, -18, -20, -30, -40]` (drop labels that would overlap at `size::METER_H`; marks stay).
  - After Program: a separator, then one `output_strip` per `output_meters` entry: name, L/R bars + scale (no fader), the note text under the bars, ⚙ opening `Page::Devices`, `Page::Icecast` or `Page::WebRtc`. Not live → bars empty and note in muted text.
  - Mute on Program still asks first ("Mute Program? Listeners hear silence until you unmute it.").
  - Devices page: "Program device" picker with "None", "System default" and each device (None writes `"none"`, System default writes `""`), help text "Opens at the next start"; "Monitor device" picker (writes `audio.monitor`).
  - Every control's accessible name starts with its visible label ("SND A", "Mute Program", "Program device").

- [ ] **Step 1: Failing tests**

```rust
#[test] fn snd_toggle_sends_a_strip_edit() { /* click "SND A" → Action carrying StripChange::Snd(true) for strip "A" */ }
#[test] fn snd_is_disabled_without_a_monitor_device() { /* devices empty → "SND A" disabled */ }
#[test] fn a_down_output_strip_shows_its_note() { /* Icecast not live, note "Not connected" → label "Not connected" present */ }
#[test] fn routing_page_is_gone() { /* Page::ALL has no "Routing" label */ }
#[test] fn choosing_none_for_the_program_device_writes_none() { /* pick "None" → settings edit audio.out = "none" */ }
```

- [ ] **Step 2:** Run `mise exec -- cargo test -p radio-ui-egui` → FAIL.
- [ ] **Step 3:** Implement per Behaviour, sizes and colours from tokens only.
- [ ] **Step 4:** Tests pass; clippy clean. Run the app (`mise exec -- cargo run -p radio`) with no Icecast configured and check the strips read honestly (Icecast "Not connected", Program device per setting); take a screenshot into `docs/screenshots/` if that folder holds mixer shots.
- [ ] **Step 5:** Commit "The mixer shows console strips with SND and a meter per output".

### Task 8: Whole-workspace check

- [ ] `mise exec -- cargo fmt --all --check`, `mise exec -- cargo test --workspace`, `mise exec -- cargo clippy --workspace --all-targets -- -D warnings` → all pass.
- [ ] `grep -rni "pfl\|cue_open\|Route::" crates apps --include=*.rs` returns only migration code (`StripRow`, `LegacyRoute`, settings migration) and its tests.
