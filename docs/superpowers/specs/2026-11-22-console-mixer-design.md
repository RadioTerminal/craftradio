# Console mixer: SND monitor bus and output meters

Date: 2026-11-22. Status: agreed with the operator in conversation, pending review of this doc.

## Intent

The operator wants the mixer to read like a broadcast console (reference: a strip-per-source
console with stereo L/R meters, a fader, ON/M and SND/REC). They do **not** want the
processing slots, Pan/Gain knobs or REC. What they want:

1. **A stereo view per strip**: L and R meters beside the fader, with a dB scale.
2. **Mute and SND per strip.** SND is a monitor send: the local sound card plays *nothing*
   unless a strip's SND is on. SND **replaces PFL**.
3. **A meter per output**: Icecast and WebRTC (and the Program sound card) each get their own
   meter showing what that output really sends.

Why: the station should be silent locally unless someone chooses to listen, and the operator
should see at a glance that each output is really carrying audio.

## Goals

- Every strip (A, B, C, D, Cart, Live, Program) shows L/R bars beside its fader, a dB scale,
  a **Mute** toggle and an **SND** toggle.
- SND on a source strip (A–D, Cart, Live) sends that strip to the Monitor bus **pre-fader and
  pre-mute** (same tap point as today's PFL), so a faded-down or muted deck can be cued.
- SND on **Program** sends the on-air mix (post-dynamics, what Icecast/WebRTC get) to the
  Monitor bus.
- The Monitor bus plays on the **Monitor device** (today's cue device, renamed). With no SND
  on anywhere, the Monitor device is silent.
- The **Program device** (today's "Out" device) stays as an optional air feed for hardware
  (FM transmitter, studio chain). Its picker offers **None**, **System default** and each
  device. With None a system-clock timer drives the mixer, as `--null-output` does today
  (`--headless` alone still opens the device). A change takes effect at the next start, as
  today ("Opens at the next start"); live switching is out of scope.
- Output meter strips for **Icecast**, **WebRTC** and the **Program device**, after the
  Program strip. Each measures what that output actually consumes, not a copy of the Program
  meter. When an output is off, disconnected or not configured, its strip says so in words
  (e.g. "Not connected", "Off", "No device") and the bars are empty, never a green state
  (AGENTS.md rule 7).
- All of it is engine commands, so the CLI and MCP can do everything the panel does
  (rule 6). Settings live in Preferences (rule 9).

## Non-goals

- Per-strip DSP (EQ, COMP, AGC, DSP 1–4), Pan, input Gain knobs, REC. Not shown at all.
- Multiple monitor buses, talkback, or mix-minus.
- Changing Program processing (compressor/limiter/loudness stay as they are).

## Anti-goals

- A control that looks active but does nothing (e.g. an output meter that mirrors Program
  while Icecast is down).
- SND leaking onto air: SND must never change what Program, Icecast, WebRTC or the Program
  device carry.
- An existing station silently losing its air feed on upgrade.

## Approaches considered

- **A. One sound card as monitor only.** The single device plays the SND bus; air goes only
  to Icecast/WebRTC. Simplest, but a station feeding hardware from the sound card would have
  to use Program SND, and any other SND would leak onto that feed.
- **B. Program device + Monitor device (chosen).** Keep the Out device as an optional Program
  feed (may be None) and turn the cue device into the Monitor device playing the SND bus.
  Keeps a hardware air feed clean, and maps onto the existing PFL bus and cue output almost
  one to one. Costs one extra device setting.

The operator picked **B**.

## Design

### Audio (`crates/audio`)

- The PFL bus becomes the **SND bus** (rename `set_pfl` → `set_snd`, the `pfl` mask → `snd`,
  `render(program, pfl)` → `render(program, monitor)`). Source strips tap it pre-fader and
  pre-mute, as now.
- **Program SND** is new: when on, the post-dynamics Program frame is added to the monitor
  buffer (after the limiter **and after the Program mute**, so it is exactly what listeners
  hear, including the lookahead latency; a muted Program monitors as silence). Today `Strip::Out` has no PFL; it gains an SND bit.
- Monitor sums can clip when several SNDs are on; apply the same soft `limit` used for
  Program without dynamics so the monitor never wraps. (Builder's call on exact method.)
- Meters are already per channel (`[f32; 2]`); no change needed for stereo.

### Output meters

- Add a small lock-free meter on the **consumer side** of each output:
  - **Icecast**: in `radio_stream` output `send`, meter the samples actually popped from the
    tap and handed to the encoder. While not connected, no samples are taken, so the meter
    falls to silence.
  - **WebRTC**: meter the samples the media loop encodes for listeners. With zero listeners
    the bars are empty and the strip says "On — 0 listeners"; with the server off, "Off".
    The listener count shows when on.
  - **Program device**: meter what is written to a real sound card. `ProgramSource::render`
    also runs under the null output and the fallback, so the meter is fed only while a card
    is open (tied to the device state); under None or fallback the bars stay empty and the
    strip says "No device" / "Fallback: <reason>".
- Each exposes level + held peak per channel, read by the UI like the strip meters
  (`MeterReader`-style snapshot). Status text comes from the existing `StreamStatus`,
  `WebRtcStatus` and device state.
- Output strips are **meter only**: no fader, no Mute, no SND. Their ⚙ opens the matching
  Preferences page (Icecast, WebRTC, Devices).

### Engine (`crates/engine`)

- `StripState.pfl` → `snd`. `mixer.set_strip` takes `snd: Option<bool>`; the strip list
  stays `A B C D Cart Live Out` (`Out` = Program; UI label "Program").
- **Remove `Route`** (`Out` / `OutAndCue` / `CueOnly`) and the Routing page: Mute + SND
  covers "cue only".
- `mixer.set_strip { snd: true }` without a Monitor device is refused with a clear message,
  as PFL is today.
- Device settings: `audio.out` → Program device, with an explicit **None** value meaning
  "no sound card, system clock". `audio.pfl` → `audio.monitor`.
- **Migration** (on load of stored state/settings):
  - `pfl: true` → `snd: true`.
  - `route: CueOnly` → `mute: true, snd: true`; `route: OutAndCue` → `snd: true`;
    `route: Out` → no change.
  - `audio.pfl` → `audio.monitor`.
  - `audio.out` is kept as is, and a missing or empty `audio.out` still means System
    default, so an existing station keeps its air feed. None is stored as an explicit value
    (e.g. `"none"`). Only a **new** station gets None: station creation writes it.
- Old command fields (`pfl`, `route`) in `mixer.set_strip`: builder's call whether to accept
  them as aliases for one release or refuse them with a message naming `snd`.

### UI (`crates/ui-egui`, `crates/ui-engine`)

- Strip layout, top to bottom: name (+ ⚙ on Program), **L/R bars with dB scale** beside the
  vertical fader, level / peak readout, then a row of **Mute** and **SND** toggles. Toggles
  show state in word and fill ("MUTED", "SND ON"), never colour alone (rule 10). SND is
  disabled with a hover reason when no Monitor device is present.
- Scale marks follow the reference's console feel (e.g. 0, −3, −6, −9, −12, −18, −20, −30,
  −40); exact set is the builder's call, from theme tokens.
- After Program, a visual separator, then the output strips **Program device**, **Icecast**,
  **WebRTC**.
- Muting Program still asks first (as Mute Out does today).
- Preferences › Devices: "Program device" (with None) and "Monitor device". The Routing page
  is removed; its search words move to Devices.
- The preview/mock simulates the new meters and states and keeps its "preview — simulated
  levels, no audio" note.

### CLI / MCP

- `mixer.set_strip` (with `snd`) and the device settings keep working from the CLI and MCP.
  Meter levels are not readable through commands today (strip meters neither), so output
  meters are GUI-only, like strip meters; adding meter reads to the CLI is out of scope.

## Tests (behaviour, per AGENTS.md)

- Audio: SND on a source strip puts it on the monitor buffer and leaves Program unchanged;
  SND with fader down / mute still reaches the monitor; nothing on the monitor with no SND;
  Program SND reaches the monitor and is the post-dynamics signal.
- Engine: migration of `pfl` and each `Route` value; `snd: true` refused without a Monitor
  device; new station defaults Program device to None, existing `audio.out` kept.
- Output meters: a disconnected Icecast meter falls to silence while Program has audio; a
  connected one follows it. Same for the Program device with None.
- UI: an output strip of a down output shows its state text and no level.

## Left to the builder

- Ring/meter plumbing details, naming of the new types, exact scale marks and token names.
- Whether to alias old `pfl`/`route` command fields for a release.
- Splitting into commits: rename PFL→SND (no behaviour change) first, then Program SND,
  then Route removal + migration, then output meters, then the UI.
