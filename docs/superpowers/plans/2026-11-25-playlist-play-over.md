# Playlist items that play over the music — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A playlist can play its items over the music, ducked, placed by the tracks' intro and
outro markers; cart ducking goes live through the same mixer path.

**Architecture:** The library detects `intro_end` / `outro_start`. The scheduler, after fitting,
turns entries of an *Over* playlist into over entries that sit on a host song and take no
airtime, then fits again. Playout fires them on a reserved cart slot relative to the host's
real start; the mixer ducks decks and live input by the deepest duck among the cart voices
that are playing.

**Tech Stack:** Rust (workspace), serde, jiff, egui. Cargo via `mise exec -- cargo …`.

**Spec:** `docs/superpowers/specs/2026-11-25-playlist-play-over-design.md` (read it first).

## Global Constraints

- Layers: `model` → `library`/`scheduler` → `audio` → `playout`/`engine` → `ui-engine` → `ui-egui`. A crate depends only on lower layers.
- Panels change engine state only through commands; every new setting is a registry command (CLI and MCP get it free).
- No placeholder shown as success (rule 7): a skipped over item is logged as skipped, never shown as aired.
- Sizes, colours, gaps and fonts come from `crates/ui-egui/src/theme.rs` tokens.
- Tests in `crates/<crate>/tests/it/<module>.rs`, registered in `tests/it/main.rs`. Assert relations, not chosen constants.
- Fixture titles and artists are invented. Times use `jiff`.
- Spec values: default duck −12 dB; a playlist's `duck_db` is whole dB in −40…0; duck attack 150 ms, release 400 ms; main level = track loudness − 6 dB, smoothed over 1 s; a marker within 1 s of its edge is `None`.
- Checks per task, for the crates touched: `mise exec -- cargo fmt --all`, `mise exec -- cargo test -p <crates>`, `mise exec -- cargo clippy -p <crates> --all-targets -- -D warnings`.

## Review Focus

1. **A station file saved before this change** (no `plays`, no `outro_start`, no `markers`) loads unchanged and plans exactly as before. → Task 1 round-trip test.
2. **A full cartwall (every slot used)** while an over item fires: the over item still plays on its reserved slot and no cart is silently lost. The wall offers one fewer slot. → Task 5 test.
3. **The host is cut, skipped or paused before the fire point**: the over item never fires over the next song. → Task 7 test.
4. **An Over playlist whose items are all longer than any intro/outro**: every item plays in sequence; the plan equals the plan with `InSequence`. → Task 6 test.
5. **A hand-set marker followed by a file re-analysis** keeps the hand value. → Task 3 test.

---

### Task 1: Model — markers and play mode

**Files:**
- Modify: `crates/model/src/item.rs` (`Timing`), `crates/model/src/playlist.rs` (`Playlist`), `crates/model/src/lib.rs` (re-exports)
- Fix every `Timing { .. }` / `Playlist { .. }` literal the compiler reports (`crates/library/src/lib.rs:322`, `crates/testkit`, tests)
- Test: `crates/model/tests/it/` (new module `play_over.rs`, or the existing serde module if there is one)

**Interfaces — Produces:**
- `Timing.outro_start: Option<Duration>` `#[serde(default)]`
- `Timing.markers: MarkerSource` `#[serde(default)]`, `enum MarkerSource { #[default] NotAnalysed, Detected, ByHand }` (derives like `OnInterrupt`)
- `Playlist.plays: PlayMode` `#[serde(default)]`, `enum PlayMode { #[default] InSequence, Over { duck_db: i16 } }` (derive `Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema`). Whole dB as `i16`, because `Playlist` derives `Eq` and an `f32` would break it; the mixer takes `f32::from(duck_db)`.
- `pub const DEFAULT_DUCK_DB: i16 = -12;` and `pub const MAX_DUCK_DB: i16 = -40;` in `playlist.rs`

- [ ] **Step 1:** Write `old_station_json_loads_with_defaults`: deserialize a station JSON written without the new fields (take one from an existing fixture); assert `plays == PlayMode::InSequence`, `outro_start == None`, `markers == MarkerSource::NotAnalysed`. And `play_mode_round_trips`: `Over { duck_db: -9 }` → JSON → back is equal.
- [ ] **Step 2:** Run `mise exec -- cargo test -p radio-model` — FAIL (no such fields).
- [ ] **Step 3:** Add the fields and types; fix literals with `outro_start: None, markers: MarkerSource::NotAnalysed` / `plays: PlayMode::InSequence`.
- [ ] **Step 4:** `mise exec -- cargo test --workspace --no-run` builds; model tests PASS.
- [ ] **Step 5:** Commit: "Playlists say how their items play, and cue points gain an outro start".

### Task 2: Library — detect intro end and outro start

**Files:**
- Modify: `crates/library/src/lib.rs` (`analyse`, around lines 270–330)
- Test: `crates/library/tests/it/markers.rs` (create `tests/it/main.rs` if the crate has none)

**Interfaces — Consumes:** Task 1 fields. **Produces:** `analyse` fills `timing.intro_end`, `timing.outro_start`, `timing.markers = Detected`; a pure helper `fn markers(windows: &[(u64, f64)], rate: u32, loudness_db: f64, cue_in: Duration, segue: Duration) -> (Option<Duration>, Option<Duration>)` in the same file (or `src/markers.rs`) so tests can call it through `analyse` only.

- [ ] **Step 1:** Tests, generating WAV files in a temp dir with the existing test helper (or `hound` if already a dev-dependency; else write a minimal PCM WAV writer in the test module):
  - `quiet_intro_and_outro_are_found`: 4 s at −30 dBFS sine, 20 s at −10 dBFS, 4 s at −30 dBFS. Assert `intro_end` is within 4 s ± 1.0 s and `outro_start` within 24 s ± 1.0 s, and `intro_end < outro_start < segue`.
  - `loud_from_the_start_has_no_intro`: 20 s at −10 dBFS then 4 s quiet → `intro_end == None`, `outro_start.is_some()`.
  - `markers_are_marked_detected`: `markers == MarkerSource::Detected`.
- [ ] **Step 2:** Run `mise exec -- cargo test -p radio-library markers` — FAIL.
- [ ] **Step 3:** Implement: smooth `windows` energy (100 ms) with a 10-window moving average; threshold = `10^((loudness − 6)/10)` in the same energy units the segue code uses (`(loudness - 20.0) / 10.0` is the existing pattern); intro end = first smoothed point ≥ threshold after `cue_in`; outro start = last smoothed point ≥ threshold before `segue`; `None` when within 1 s of its edge (cue_in / segue) or never reached.
- [ ] **Step 4:** PASS; existing library tests still PASS.
- [ ] **Step 5:** Commit: "The library finds where a song's intro ends and its outro starts".

### Task 3: Engine — `item.set_markers` and `playlist.set_plays`

**Files:**
- Modify: `crates/engine/src/command.rs` (enum, `name()`, the registry table near line 708), `crates/engine/src/lib.rs` (apply, beside `PlaylistSetOnInterrupt` and `CartSet` ~1480)
- Test: `crates/engine/tests/it/play_over.rs`

**Interfaces — Produces:**
- `Command::ItemSetMarkers { item: ItemId, intro_end: Option<Duration>, outro_start: Option<Duration>, source: MarkerSource }`, id `"item.set_markers"`. Refused (error text, no state change) when: item not a file / has no timing; a marker outside `cue_in..=cue_out`; `outro_start > segue`; `intro_end ≥ outro_start` when both set; `source == NotAnalysed`. When `source == Detected` and the item's `markers == ByHand`, it succeeds and changes nothing.
- `Command::PlaylistSetPlays { playlist: PlaylistId, plays: PlayMode }`, id `"playlist.set_plays"`; refuses `Over { duck_db }` outside `MAX_DUCK_DB..=0`.
- Both are station-data changes (they replan), unlike the cart duck note at `lib.rs:675`.

- [ ] **Step 1:** Tests: `hand_markers_survive_detection` (set ByHand, then Detected with other values → ByHand values kept); `marker_after_segue_is_refused`; `positive_duck_is_refused`; `set_plays_changes_the_playlist`. Dispatch through the registry by id with JSON args, as existing engine tests do, so CLI/MCP use is covered.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS.
- [ ] **Step 5:** Commit: "Commands to set a song's intro and outro markers and how a playlist plays".

### Task 4: Playout — analyse markers of existing items

**Files:**
- Modify: `crates/playout/src/deadair.rs` (~400–420, the analysis loop), `crates/playout/src/core.rs` (`Done::Analysed` handling)
- Test: `crates/playout/tests/it/markers.rs`

**Interfaces — Consumes:** Task 2 `analyse`, Task 3 `item.set_markers`.
- An item with `timing.markers == NotAnalysed` is analysed even when it has timing (today the loop skips any item with timing). On the result, playout dispatches `item.set_markers { source: Detected }` with only the two markers; it never overwrites cue points it didn't set. This is how libraries from before this change pick the markers up.

- [ ] **Step 1:** Test `old_items_get_markers`: a station whose file item has timing and `NotAnalysed`, with a stub `cfg.analyse` returning markers; after ticks, the engine state has those markers, `Detected`, and the original `cue_in`/`segue` unchanged.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS.
- [ ] **Step 5:** Commit: "Songs analysed before markers existed get them on their next analysis".

### Task 5: Mixer and carts — ducking goes live

**Files:**
- Modify: `crates/audio/src/command.rs` (`FireCart`), `crates/audio/src/mixer.rs` (`Voice`, `FireCart` handling ~851, the cart loop ~1083, `Duck`), `crates/playout/src/ops.rs` (`op_cart`, `Carts::new`, `sync_carts`), `crates/playout/src/core.rs:551`
- Remove the "kept, not played yet" note: `crates/ui-egui/src/cartwall.rs:21,471` and its callers
- Test: `crates/audio/tests/it/mixer.rs`, `crates/playout/tests/it/` (existing cart tests if any; else `carts.rs`)

**Interfaces — Produces:**
- `CommandKind::FireCart { slot: usize, duck_db: f32 }` (0 = no duck). `Voice` gains `duck_db: f32`.
- Mixer: each frame, `target = min(duck_db of active voices, 0.0)`; when it changes, ramp to it — down over `DUCK_ATTACK = 150 ms`, up over `DUCK_RELEASE = 400 ms` (constants in `mixer.rs`). The explicit `Duck`/`Unduck` commands stay and combine by taking the deeper of the two.
- Playout: `pub(crate) fn over_slot(cart_slots: usize) -> usize { cart_slots - 1 }`; the wall uses `cart_slots - 1` slots (`Carts::new`). `op_cart` sends the cart's `duck_db` from engine state.

- [ ] **Step 1:** Mixer tests: `ducking_cart_lowers_decks_not_carts` (deck tone and cart tone; while the cart plays, deck level lower by ~|duck|, cart level unchanged); `duck_lifts_after_the_last_ducking_cart` (two overlapping carts −6 and −18: depth follows −18 while both play, −6 after the deep one ends, 0 after both); `zero_duck_cart_leaves_decks`. Playout test `full_wall_keeps_the_over_slot`: assign carts to every wall slot; the over slot index is not used by any of them.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS (`-p radio-audio -p radio-playout -p radio-ui-egui`).
- [ ] **Step 5:** Commit: "Carts duck the music by their set depth".

### Task 6: Scheduler — place over entries

**Files:**
- Create: `crates/scheduler/src/over.rs`
- Modify: `crates/scheduler/src/merge.rs` (`Timeline` gains `over`), `crates/scheduler/src/replan.rs` (`plan`), `crates/scheduler/src/lib.rs` (exports), the `Timeline { .. }` literals (`replan.rs:133`, `merge.rs:528,736`, `playout/src/core.rs:540`, scheduler tests)
- Test: `crates/scheduler/tests/it/over.rs`

**Interfaces — Produces:**
```rust
pub enum Side { Intro, Outro }
pub struct Over { pub host: ItemId, pub host_start: Timestamp, pub side: Side,
                  pub offset: Duration /* from host start */, pub duck_db: i16 }
pub struct OverEntry { pub entry: Entry, pub over: Over } // entry.start = host_start + offset
// Timeline: #[serde(default)] pub over: Vec<OverEntry>
pub fn place_over(tl: &mut Timeline, station: &Station) -> Vec<OverEntry> // pulls placed ones out of tl.entries
```
- `plan()` becomes: today's body → `place_over` → `fit_all` again on the remaining entries → re-validate each placed entry against the final neighbours (host still there, same side still fits, host `trim == 0` for Outro, no new `NotAdjacent…` break) → invalid ones go back into `tl.entries` in sequence after their host (Outro) or before it (Intro) → final `fit_all` → recompute each `OverEntry.entry.start` from its host's final start. At most three `fit_all` calls; the last places nothing.
- Placement per spec §Placement and "My calls": candidates are non-fixed, non-locked entries of a playlist with `PlayMode::Over`; hosts are `Source::File` entries of `InSequence` playlists; intro fits if `intro_end − cue_in ≥ len`; outro fits if `segue − outro_start ≥ len`, offset `= (segue − cue_in) − len`; one over item per handover (outro of A + intro of B); intro tried first.

- [ ] **Step 1:** Tests (use `radio_testkit::{track, jingle, playlist, station}`; set markers on the track items):
  - `intro_wins_when_it_fits`; `outro_when_only_it_fits`; `in_sequence_when_neither_fits`;
  - `over_entry_takes_no_airtime`: the entry after the host starts earlier than in the `InSequence` plan;
  - `hard_event_still_on_target` with an over playlist feeding the hour before it;
  - `one_over_per_handover`: two jingles due at the same handover → one over, one elsewhere/in sequence, never two overlapping windows;
  - `trimmed_host_takes_no_outro`;
  - `fixed_entry_of_over_playlist_stays_in_sequence`;
  - `all_too_long_equals_in_sequence_plan` (Review Focus 4).
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS, and the existing `performance` test still PASSes.
- [ ] **Step 5:** Commit: "The scheduler lays jingles of an over playlist on a song's intro or outro".

### Task 7: Playout — fire over entries

**Files:**
- Create: `crates/playout/src/over.rs`
- Modify: `crates/playout/src/core.rs` (tick, `Done::Cart` for the over slot, replan uses `timeline.over`), `crates/playout/src/ops.rs` (load the over slot like `sync_carts`), `crates/playout/src/view.rs` (expose over entries to the view)
- Test: `crates/playout/tests/it/over.rs`

**Interfaces — Consumes:** Task 5 `FireCart { slot: over_slot(..), duck_db }`, Task 6 `Timeline.over`.
- Keep the next not-yet-fired over entry's file loaded in the over slot.
- When a deck `Slot` for `over.host` is `Playing` with `started` set and not `seeked` / `paused_at`, fire at sample `map.sample_at(started + offset)` (the mapping inverse of `wall_at`; add it if missing). If that moment has passed, the clip isn't loaded, or the host was paused/seeked/cut first → skip.
- Log: fired → an `AsPlayed` row with `origin: "over"` and `rule` naming the side ("over intro" / "over outro"); skipped → a row with status `Interrupted` and no airtime claim, plus a `tracing::warn!`. Never an alarm.

- [ ] **Step 1:** Tests with the fake clock and test output: `fires_over_the_intro` (cart voice starts within one mixer block of the host start + offset, and decks are ducked); `follows_the_late_host` (host starts 2 s late → fire also 2 s later); `skips_when_host_is_cut` (cut before the fire point → never fires, log row is not `Aired`); `skips_when_paused`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS.
- [ ] **Step 5:** Commit: "Playout plays a jingle over its song and logs it".

### Task 8: UI — the Plays setting, the marker fields, Upcoming rows

**Files:**
- Modify: `crates/ui-egui/src/view.rs` (view-model fields), `crates/ui-egui/src/rules_editor.rs` (Plays row), `crates/ui-egui/src/meta_editor.rs` (marker fields), `crates/ui-egui/src/scheduled_pane.rs` / `main_pane.rs` (Upcoming rows), `crates/ui-egui/src/mock/*` (mock data and edits), `crates/ui-engine/src/map.rs` and `edit.rs` (map the new state, dispatch the new commands)
- Test: `crates/ui-egui/tests/it/editing.rs` (or a new `play_over.rs`), `crates/ui-engine/tests/it/edits.rs`

**Interfaces — Consumes:** Task 3 commands, Task 7 view.
- Rules editor: **Plays** combo *In sequence* / *Over the music*; **Duck by** `DragValue` (dB, −40…0, default −12) only for Over. Each control's accessible name starts with its label.
- Item editor: **Intro ends**, **Outro starts** (m:ss.s, empty = none) and a small "auto" / "set by hand" hint; editing sends `item.set_markers { source: ByHand }`.
- Upcoming: an over entry is a thin row under its host: playlist colour swatch + name, "over intro" / "over outro", start time. Not a slot of its own; sizes from theme tokens.

- [ ] **Step 1:** Tests: `plays_over_sends_set_plays` (choose Over in the rules editor → `playlist.set_plays` action with −12); `duck_row_hidden_in_sequence`; `marker_edit_sends_by_hand`; `over_row_shows_under_its_host` (kittest finds a row labelled with the playlist name and "over intro" after the host row). ui-engine: `set_plays_reaches_the_engine`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS (`-p radio-ui-egui -p radio-ui-engine`).
- [ ] **Step 5:** Commit: "Playlists can be set to play over the music, songs show their markers, and Upcoming shows where each jingle lands".
