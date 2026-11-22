# M1 Scheduling Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pure, deterministic library that turns a station definition (the main playlist,
any number of named playlists with rules, and fixed-time events) into an on-air timeline
with an estimated start time for every item, and fits fixed events (reorder → fill → trim →
alarm). A CLI prints the timeline.

**Architecture:** A Cargo workspace laid out like craft-anything (`crates/*`, `apps/*`). It has
three library crates. `model` holds plain serde data. `timing` holds the time arithmetic
and local-time resolution. `scheduler` merges the playlists into a timeline, enforces the
constraints and fits fixed events. On top of them sits one app, `radio-cli`, which reads a
TOML station file. There's no audio, no I/O beyond reading that file, and no wall clock:
"now" is always passed in.

**Tech Stack:** Rust (edition 2024), serde, toml, jiff (time zones / DST), thiserror,
rand_chacha (seeded shuffles), clap, proptest.

**Spec:** `docs/2026-11-21-radio-station-manager-design.md` (§6.2, §6.3, §6.6, §6.15).
Roadmap: `docs/superpowers/plans/2026-11-21-roadmap.md`.

**Status (2026-11-22):** done. Tests live in one binary per crate (`tests/it/*.rs`), not the
files named below. Some listed tests exist under other names. Task 6's 10 ms timing test was
not written; `tests/it/performance.rs` holds the scheduler's benchmarks.

## Global Constraints

- Edition 2024, `rust-version = "1.97"`, license `MIT OR Apache-2.0`, matching the template.
- Workspace `members = ["crates/*", "apps/*"]`; dependency versions go in `[workspace.dependencies]`.
- Library crates are named `radio-<name>`, live in `crates/<name>`, and are imported as `radio_<name>`.
- Library crates don't read the system clock and don't touch the filesystem; "now" is always a parameter.
- Everything is deterministic: the same input and seed give the same timeline (no `thread_rng`, no `HashMap` iteration order in the output).
- Wall-clock time is `jiff::Timestamp`. Lengths are `std::time::Duration`. Signed errors are `jiff::SignedDuration`.
- `fixed_at` is a local `jiff::civil::DateTime` in the station's time zone. A time that doesn't exist (DST gap) moves to the next valid time; a time that occurs twice (DST fold) takes the first one. This is jiff's `compatible()` disambiguation.
- Defaults from the spec: tolerance ±1 s; lock time 10 min; re-plan threshold 0.5 s; fit search resolution 100 ms.
- No playlist type is special: ads, jingles, shows and news are all `Playlist`s. Behaviour comes only from the flags and rules.
- CI runs `cargo test --workspace` on Windows, Linux and macOS.
- **Minimal tests (user directive, 2026-11-21):** each task writes only the 1–3 tests for its
  core behaviour, plus its Review Focus test if it owns one. Skip the remaining listed tests,
  the proptest step (Task 8 step 5) and the performance test (Task 6). The goal is to reach
  the UI (Task 10) fast.

## Review Focus

1. **A rule fires but its playlist has nothing eligible** (empty, every item past its valid
   dates or caps, or every item blocked by constraints). Expected: that insertion is skipped
   with a `Warning::NoEligibleItem`. No panic, and the timeline continues. Tested in Task 5.
2. **Not enough material before a fixed event** (the gap is bigger than everything movable
   plus the fill pool). Expected: a `FitResult` with `within_tolerance = false`, and for
   `hard` events a `Warning::DeadAir` with the length of the gap. Tested in Task 7.
3. **A fixed event on a DST change day** (02:30 on spring-forward, 02:30 on fall-back).
   Expected: it resolves to 03:30 and to the first 02:30. Tested in Task 3.
4. **Two fixed events closer together than the item between them, or at the same time.**
   Expected: the higher `priority` playlist keeps its time. The other one starts as soon as
   possible after it and gets `Warning::FixedConflict`. Tested in Task 7.
5. **Invalid item timing** (`segue < cue_in`, `cue_out < segue`, a live / stream item with no
   `planned_len`). Expected: `Station::validate` returns an error naming the item; the planner
   never sees it. Tested in Task 1.

---

## File Structure

```
Cargo.toml                         workspace
crates/model/src/lib.rs            re-exports
crates/model/src/item.rs           Item, Source, Timing, ItemId
crates/model/src/playlist.rs       Playlist, PlaylistId, PickOrder
crates/model/src/rule.rs           Rule, Trigger, Placement, FixMode, Repeat, Constraint
crates/model/src/station.rs        Station, validate(), ModelError
crates/model/tests/model.rs
crates/timing/src/lib.rs           advance(), item_len(), resolve_local(), next_occurrences()
crates/timing/tests/timing.rs
crates/scheduler/src/lib.rs        plan() entry point, PlanInput, Timeline, Entry, Origin, Warning
crates/scheduler/src/history.rs    PlayHistory (what has played, for caps / separation)
crates/scheduler/src/pick.rs       picking the next item from a playlist
crates/scheduler/src/constraints.rs constraint checks
crates/scheduler/src/merge.rs      main playlist plus rule insertions → raw timeline
crates/scheduler/src/fit.rs        fixed-event fitting
crates/scheduler/src/replan.rs     hysteresis, lock window, frozen entries
crates/scheduler/tests/*.rs
crates/testkit/src/lib.rs          fixture builders
apps/radio-cli/src/main.rs         `radio-cli plan`
apps/radio-cli/tests/cli.rs
examples/station.toml
```

---

### Task 1: Workspace and data model

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `crates/model/**`, `crates/testkit/**`
- Test: `crates/model/tests/model.rs`

**Interfaces:**
- Produces (in `radio_model`):
  ```rust
  pub struct ItemId(pub u64);  pub struct PlaylistId(pub u64);   // Copy, Eq, Ord, Hash, serde
  pub enum Source { File { path: String }, LiveInput { device: String, channels: [u16; 2] }, Stream { url: String } }
  pub struct Timing { pub cue_in: Duration, pub segue: Duration, pub cue_out: Duration,
                      pub intro_end: Option<Duration>, pub fade_in: Duration, pub fade_out: Duration }
  pub struct Item { pub id: ItemId, pub title: String, pub artist: Option<String>, pub source: Source,
                    pub timing: Option<Timing>, pub planned_len: Option<Duration>,
                    pub movable: bool, pub trimmable: bool, pub locked: bool,
                    pub valid_from: Option<civil::Date>, pub valid_to: Option<civil::Date>,
                    pub max_per_day: Option<u32> }
  pub enum PickOrder { InOrder, Shuffle { seed: u64 }, Weighted { weights: Vec<(ItemId, u32)> }, LeastRecentlyPlayed }
  pub struct Playlist { pub id: PlaylistId, pub name: String, pub color: [u8; 3], pub enabled: bool,
                        pub is_main: bool, pub is_fill_pool: bool, pub priority: i32,
                        pub pick: PickOrder, pub items: Vec<Item>, pub rules: Vec<Rule>,
                        pub constraints: Vec<Constraint> }
  pub enum Trigger { EveryNTracks(u32), EveryNMinutes(u32), AtMinutes(Vec<u8>),
                     At { at: civil::DateTime, repeat: Repeat }, ScriptEvent(String) }
  pub enum Repeat { Once, Daily, Weekdays, Weekly }
  pub enum FixMode { Soft, Hard }
  pub enum Placement { BetweenTracks, Fixed { mode: FixMode, tolerance: Duration } }
  pub struct Rule { pub trigger: Trigger, pub placement: Placement, pub take: u8, pub constraints: Vec<Constraint> }
  pub enum Constraint { NotAdjacentToSelf, NotAdjacentTo(PlaylistId), MinGapSameItem(Duration),
                        ArtistSeparation(Duration), TitleSeparation(Duration),
                        Daypart { from: civil::Time, to: civil::Time }, MaxPerHour(u32), MaxPerDay(u32) }
  pub struct Station { pub tz: String /* IANA name */, pub playlists: Vec<Playlist> }
  impl Station { pub fn validate(&self) -> Result<(), ModelError>; pub fn main(&self) -> &Playlist; }
  pub enum ModelError { NoMainPlaylist, MultipleMainPlaylists, BadTiming { item: ItemId, reason: &'static str },
                        MissingPlannedLen(ItemId), UnknownTimeZone(String), DuplicateId(u64) }
  ```
- Produces (in `radio_testkit`): `track(id: u64, title: &str, artist: &str, secs: u32) -> Item`
  (cue_in 0, segue = secs − 2, cue_out = secs, movable, trimmable); `jingle(id, secs) -> Item`
  (segue = cue_out = secs, not movable, not trimmable); `live(id, planned_secs) -> Item`;
  `playlist(id, name, items) -> Playlist`; `station(tz, playlists) -> Station`.
  Serde defaults: `enabled = true`, `movable = true`, `trimmable = true`, `take = 1`.

- [x] **Step 1: Create the workspace.** Root `Cargo.toml` per Global Constraints, with
  workspace dependencies `serde = { version = "1", features = ["derive"] }`, `toml = "0.9"`,
  `jiff = { version = "0.2", features = ["serde"] }`, `thiserror = "2"`, `rand_chacha = "0.9"`,
  `clap = { version = "4", features = ["derive"] }`, `proptest = "1"`. Run `git init`.
- [x] **Step 2: Write the failing tests** in `crates/model/tests/model.rs`:
  ```rust
  #[test] fn station_roundtrips_through_toml()   // build with testkit, to_string → from_str, assert_eq
  #[test] fn validate_rejects_no_main()           // Err(ModelError::NoMainPlaylist)
  #[test] fn validate_rejects_two_mains()         // Err(MultipleMainPlaylists)
  #[test] fn validate_rejects_segue_before_cue_in() // Err(BadTiming { item: ItemId(7), .. })
  #[test] fn validate_rejects_cue_out_before_segue()
  #[test] fn validate_requires_planned_len_for_live_and_stream() // Err(MissingPlannedLen(ItemId(9)))
  #[test] fn validate_rejects_unknown_tz()        // tz "Mars/Base" → UnknownTimeZone
  #[test] fn validate_rejects_duplicate_item_ids()
  ```
- [x] **Step 3: Run** `cargo test -p radio-model`. Expected: compile failure, because the types don't exist yet.
- [x] **Step 4: Implement** the types above in their files, and `Station::validate`. It
  checks the time zone with `jiff::tz::TimeZone::get`. A `File` item may have
  `timing: None` only if it has `planned_len` (cue points not analysed yet). Also implement
  the testkit builders.
- [x] **Step 5: Run** `cargo test -p radio-model`. Expected: all pass.
- [x] **Step 6: Commit** `feat(model): station data model and validation`.

### Task 2: Item lengths and timeline arithmetic

**Files:** Create `crates/timing/**`. Test: `crates/timing/tests/timing.rs`.

**Interfaces:**
- Consumes: `radio_model::Item`, `Timing`.
- Produces (in `radio_timing`):
  ```rust
  /// How long the item sounds from cue_in to cue_out (or planned_len).
  pub fn item_len(item: &Item) -> Duration;
  /// Time from `prev` starting to `next` starting: prev.segue − prev.cue_in, or planned_len
  /// when there's no timing. When `next` is None, or next is a Fixed Hard start,
  /// use cue_out − cue_in (play to the end).
  pub fn advance(prev: &Item, next: Option<&Item>) -> Duration;
  /// Start time of each item in `seq` when the first starts at `t0`.
  pub fn start_times(t0: Timestamp, seq: &[&Item]) -> Vec<Timestamp>;
  ```
- [x] **Step 1: Failing tests:**
  ```rust
  #[test] fn advance_uses_segue() { let a = track(1,"a","x",200); let b = track(2,"b","y",180);
      assert_eq!(advance(&a, Some(&b)), Duration::from_secs(198)); }
  #[test] fn advance_last_item_plays_to_cue_out() { assert_eq!(advance(&track(1,"a","x",200), None), Duration::from_secs(200)); }
  #[test] fn advance_live_uses_planned_len() { assert_eq!(advance(&live(3, 3600), None), Duration::from_secs(3600)); }
  #[test] fn start_times_accumulate() { // t0 = 2026-11-21T14:00:00Z; tracks 200,180,jingle 6
      // expect [14:00:00, 14:03:18, 14:06:16] }
  ```
- [x] **Step 2: Run** `cargo test -p radio-timing`. Expected: FAIL.
- [x] **Step 3: Implement** `item_len`, `advance`, `start_times` in `crates/timing/src/lib.rs`.
- [x] **Step 4: Run.** Expected: PASS.
- [x] **Step 5: Commit** `feat(timing): item length and start-time arithmetic`.

### Task 3: Local time resolution and trigger occurrences

**Files:** Modify `crates/timing/src/lib.rs`; add `crates/timing/src/occur.rs`. Test: `crates/timing/tests/occur.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub fn resolve_local(tz: &TimeZone, at: civil::DateTime) -> Timestamp; // jiff compatible()
  /// Every instant in [from, to) at which a fixed or clock trigger fires.
  /// AtMinutes(m) fires at :m of every local hour; EveryNMinutes(n) fires at every local
  /// minute where minute-of-day % n == 0. EveryNTracks and ScriptEvent return an empty Vec.
  pub fn occurrences(tz: &TimeZone, trigger: &Trigger, from: Timestamp, to: Timestamp) -> Vec<Timestamp>;
  ```
- [x] **Step 1: Failing tests** (tz `Europe/Ljubljana`):
  ```rust
  #[test] fn dst_gap_moves_forward()  // 2027-03-28T02:30 → 2027-03-28T01:30:00Z (03:30 local)
  #[test] fn dst_fold_takes_first()   // 2026-10-25T02:30 → 2026-10-25T00:30:00Z
  #[test] fn at_minutes_20_50_over_one_hour() // from 14:00 to 15:00 local → [14:20, 14:50]
  #[test] fn daily_repeat_spans_midnight()    // At 00:00 Daily, from 23:00 to 01:00 next day → [00:00]
  #[test] fn weekdays_skips_weekend()         // At 07:00 Weekdays over 2026-11-21 (Sat)…23 → only Mon 23rd
  #[test] fn once_in_past_yields_nothing()
  ```
- [x] **Step 2: Run.** Expected: FAIL.
- [x] **Step 3: Implement**, iterating over local civil days and converting each candidate with `resolve_local`.
- [x] **Step 4: Run.** Expected: PASS.
- [x] **Step 5: Commit** `feat(timing): DST-safe local times and trigger occurrences`.

### Task 4: Play history and picking items

**Files:** Create `crates/scheduler/{Cargo.toml,src/lib.rs,src/history.rs,src/pick.rs}`. Test: `crates/scheduler/tests/pick.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub struct Played { pub item: ItemId, pub playlist: PlaylistId, pub at: Timestamp,
                      pub artist: Option<String>, pub title: String }
  pub struct PlayHistory { /* sorted by at */ }
  impl PlayHistory { pub fn new() -> Self; pub fn push(&mut self, p: Played);
      pub fn last_played(&self, item: ItemId) -> Option<Timestamp>;
      pub fn count_since(&self, pred: impl Fn(&Played) -> bool, since: Timestamp) -> usize;
      pub fn iter_since(&self, since: Timestamp) -> impl Iterator<Item = &Played>; }
  /// Picker state per playlist; `next` returns the first candidate (in pick order) that `ok` accepts.
  pub struct Picker { /* cursor + ChaCha8Rng seeded from PickOrder::Shuffle.seed */ }
  impl Picker { pub fn new(pl: &Playlist) -> Self;
      pub fn next<'a>(&mut self, pl: &'a Playlist, hist: &PlayHistory, ok: impl Fn(&Item) -> bool) -> Option<&'a Item>; }
  ```
- [x] **Step 1: Failing tests:** `in_order_cycles` (3 items → 1,2,3,1);
  `shuffle_same_seed_same_order` (two pickers with seed 42 give identical sequences of 10);
  `shuffle_plays_all_before_repeat`; `least_recently_played_picks_oldest`;
  `weighted_respects_zero_weight_never_picked`; `next_returns_none_when_ok_rejects_all`.
- [x] **Step 2: Run** `cargo test -p radio-scheduler pick`. Expected: FAIL.
- [x] **Step 3: Implement.** Shuffle builds a permutation from `ChaCha8Rng::seed_from_u64(seed)` and reshuffles when it's used up. Weighted uses a deterministic smooth weighted round-robin (no RNG).
- [x] **Step 4: Run.** Expected: PASS.
- [x] **Step 5: Commit** `feat(scheduler): play history and pick orders`.

### Task 5: Merging rules into the timeline, with constraints

**Files:** Create `crates/scheduler/src/{constraints.rs,merge.rs}`; extend `lib.rs`. Test: `crates/scheduler/tests/merge.rs`.

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces (in `radio_scheduler`):
  ```rust
  pub enum Origin { Main, Rule { playlist: PlaylistId, rule: usize }, Manual }
  pub struct Entry { pub playlist: PlaylistId, pub item: ItemId, pub start: Timestamp, pub len: Duration,
                     pub origin: Origin, pub locked: bool, pub fixed: Option<Fixed>, pub trim: Duration }
  pub struct Fixed { pub target: Timestamp, pub mode: FixMode, pub tolerance: Duration, pub priority: i32 }
  pub enum Warning { NoEligibleItem { playlist: PlaylistId, at: Timestamp }, DeadAir { at: Timestamp, gap: Duration },
                     FixedConflict { playlist: PlaylistId, wanted: Timestamp, got: Timestamp },
                     OutsideTolerance { playlist: PlaylistId, error: SignedDuration } }
  pub struct Timeline { pub entries: Vec<Entry>, pub fits: Vec<FitResult>, pub warnings: Vec<Warning> }
  pub struct PlanInput<'a> { pub station: &'a Station, pub history: &'a PlayHistory, pub now: Timestamp,
                             pub horizon: Duration, pub pinned: &'a [Entry] /* locked / manual / on-deck */ }
  pub fn merge(input: &PlanInput) -> Timeline;   // no fitting yet; Task 8's `plan` wraps it
  pub fn allowed(c: &Constraint, cand: &Item, pl: PlaylistId, at: Timestamp,
                 prev: Option<&Entry>, hist: &PlayHistory, tz: &TimeZone) -> bool;
  ```
- **Merge algorithm:**
  1. Walk forward from `now`, taking the next item from the main playlist.
  2. Before each main item, check every enabled non-main playlist's `BetweenTracks` rules,
     in descending `priority`:
     - `EveryNTracks(n)` fires when n main items have passed since that rule last fired.
     - `AtMinutes` / `EveryNMinutes` / `At` with `BetweenTracks` fire at the first boundary
       between tracks at or after the occurrence.
     - When a rule fires, insert `take` items.
  3. `Fixed` rules create entries with `fixed: Some(..)` at their occurrence times.
     Task 7 fits them; `merge` just puts them in order by target time.
  4. Every candidate must pass the playlist's constraints, the rule's constraints, item
     validity (valid_from / valid_to in local date) and `max_per_day`. Simulated plays
     inside the horizon count toward the caps.
- [x] **Step 1: Failing tests:**
  - `station_id_every_3_tracks`: 9 tracks give the pattern `T T T J T T T J T T T J`.
  - `ads_at_20_and_50_land_after_track_boundary`: the ad entry's start is ≥ :20, and it's the first boundary.
  - `take_3_inserts_three_ads`.
  - `not_adjacent_to_self_never_two_ads`: with two ad rules due together, they aren't back to back.
  - `artist_separation_skips_same_artist_within_60m`.
  - `expired_item_not_picked`: valid_to yesterday.
  - `max_per_day_cap_respected`: cap 2 over 24 h gives exactly 2.
  - `daypart_rule_only_fires_inside_window`.
  - `empty_playlist_rule_warns_and_continues` (Review Focus 1): gives `Warning::NoEligibleItem`, and the main playlist continues.
  - `priority_orders_simultaneous_rules`.
- [x] **Step 2: Run.** Expected: FAIL.
- [x] **Step 3: Implement** `allowed` in `constraints.rs` and `merge` in `merge.rs`.
- [x] **Step 4: Run.** Expected: PASS.
- [x] **Step 5: Commit** `feat(scheduler): merge named playlists by rules with constraints`.

### Task 6: Fit step 1, choosing which movable items play before a fixed event

**Files:** Create `crates/scheduler/src/fit.rs`. Test: `crates/scheduler/tests/fit_select.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub const RESOLUTION: Duration = Duration::from_millis(100);
  /// Picks the subset of `cands` (in order) to play in the gap `gap`, keeping the original
  /// order. Returns the indices kept and the leftover gap (positive = early, negative = late).
  pub fn select_before(gap: Duration, cands: &[(Duration, bool /*movable*/)]) -> (Vec<usize>, SignedDuration);
  ```
- **Algorithm** (the signature doesn't determine it):
  - The candidates are the main entries from the previous anchor (now, or the previous fixed
    event) up to the event, plus the next 10 main entries after it.
  - Non-movable or locked entries that were originally before the event must stay; subtract
    them from `gap` first.
  - For the rest, run a 0/1 knapsack DP over lengths quantised to `RESOLUTION`, up to
    `gap + 10 min`. For each reachable sum, store the minimum number of displacements (an
    original-before item dropped plus an after item pulled in), so it prefers the plan that
    moves the fewest songs.
  - Choose the sum that minimises `|gap − sum|`, breaking ties by fewer displacements, then
    by an earlier finish.
  - Dropped items move to just after the event, in their original order.
- [x] **Step 1: Failing tests:**
  - `exact_fit_found`: a gap of 600 s with candidates [200, 180, 220, 190], all movable, gives `[0, 1, 2]` with 0 leftover.
  - `prefers_fewer_moves_on_tie`.
  - `non_movable_always_kept`.
  - `leftover_reported_when_no_exact_fit`.
  - `runs_under_10ms_for_40_candidates_and_1h_gap` (a timing assertion in release mode only, behind `#[cfg(not(debug_assertions))]`).
- [x] **Step 2: Run.** Expected: FAIL.
- [x] **Step 3: Implement** `select_before`.
- [x] **Step 4: Run.** Expected: PASS.
- [x] **Step 5: Commit** `feat(scheduler): fixed-event subset selection`.

### Task 7: Fit steps 2–4, fill, trim, tolerance and conflicts

**Files:** Modify `crates/scheduler/src/fit.rs`. Test: `crates/scheduler/tests/fit.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub enum FitStep { Reordered { moved: usize }, Filled { items: Vec<ItemId> }, Trimmed { by: Duration },
                     EarlyStart { by: Duration }, LateStart { by: Duration } }
  pub struct FitResult { pub playlist: PlaylistId, pub target: Timestamp, pub planned: Timestamp,
                         pub error: SignedDuration /* planned − target */, pub within_tolerance: bool,
                         pub steps: Vec<FitStep> }
  pub fn fit_all(tl: &mut Timeline, station: &Station, hist: &PlayHistory);
  ```
- **Rules, in this order (spec §6.3):**
  1. `select_before` (Task 6).
  2. Fill a positive leftover from the items of the `is_fill_pool` playlists, chosen with the
     same DP and checked with `allowed`.
  3. Trim, as a last resort:
     - If late, and the last entry before the event is `trimmable` and not locked: set its
       `trim` (a fade out early). For a `Hard` event the fade *ends* at the target. A trim is
       capped at 30 s (my call).
     - If still late and the event is `Soft`: `LateStart`, within tolerance.
     - If early and `Soft`: `EarlyStart`, within tolerance.
  4. Set `within_tolerance`. If it's false, push `OutsideTolerance`. For a `Hard` event
     with a positive leftover, also push `DeadAir { gap }`.
  - Items with `trimmable = false` (for example ads) are never trimmed. A `Fixed` entry's
    `priority` comes from its playlist's `priority`.
  - Conflicts: fixed events are processed in order of target time. If two overlap (the
    earlier one ends after the later one's target), the higher `priority` keeps its target;
    the lower one starts right after it and gets `FixedConflict`. On equal priority, the
    earlier target wins.
- [x] **Step 1: Failing tests:**
  - `fill_closes_small_gap`: a 7 s gap and a fill pool of [6 s, 3 s, 4 s] gives `Filled` with 3+4 s and an error of 0.
  - `trim_last_track_when_late_hard`: the fade ends exactly at the target.
  - `ad_is_never_trimmed`: the last item before the event is non-trimmable, so `LateStart` (soft) or `OutsideTolerance` (hard).
  - `soft_late_within_tolerance_ok`: 0.8 s late with tol 1 s gives `within_tolerance`.
  - `dead_air_warned_when_material_short` (Review Focus 2).
  - `conflicting_fixed_events_priority_wins` (Review Focus 4).
  - `same_time_equal_priority_earlier_listed_wins_deterministically`.
  - `locked_entry_never_moved_or_trimmed`.
- [x] **Step 2: Run.** Expected: FAIL.
- [x] **Step 3: Implement** `fit_all`.
- [x] **Step 4: Run.** Expected: PASS.
- [x] **Step 5: Commit** `feat(scheduler): fill, trim, tolerance and fixed-event conflicts`.

### Task 8: `plan()`, the lock window, frozen entries and hysteresis

**Files:** Create `crates/scheduler/src/replan.rs`; finish `lib.rs`. Test: `crates/scheduler/tests/replan.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub struct PlanOptions { pub lock_before: Duration /* 10 min */, pub replan_threshold: Duration /* 0.5 s */ }
  /// merge + fit_all, honouring `pinned`. With `prev`, keeps prev's order for each fixed
  /// window unless the new fit improves |error| by more than replan_threshold, and never
  /// reorders a window whose target is within lock_before of `now` (only fill / trim apply there).
  pub fn plan(input: &PlanInput, opts: &PlanOptions, prev: Option<&Timeline>) -> Timeline;
  ```
- [x] **Step 1: Failing tests:**
  - `pinned_entries_keep_position`: a manual (locked) entry stays where it was put.
  - `on_deck_entries_frozen`: the first 2 pinned entries never move.
  - `small_improvement_does_not_reshuffle`: a 0.3 s better fit keeps prev's order.
  - `large_improvement_reshuffles`.
  - `locked_window_only_fills_and_trims`: 9 min before the event, a removed track leads to fill, not reorder.
  - `same_input_same_output` (determinism).
- [x] **Step 2: Run.** Expected: FAIL.
- [x] **Step 3: Implement** `plan` and `replan.rs`.
- [x] **Step 4: Run.** Expected: PASS.
- [x] **Step 5: Add a property test** `crates/scheduler/tests/prop.rs` with proptest:
  random stations (1–30 tracks of 60–400 s, a fill pool of 3–10 items of 2–15 s, 0–3 fixed
  events). It checks four invariants:
  1. entries sorted by start, with no overlaps beyond the segue;
  2. locked entries unchanged;
  3. every `FitResult` with `within_tolerance` has `|error| <= tolerance`;
  4. no panics.

  Run `cargo test -p radio-scheduler --release prop`. Expected: PASS.
- [x] **Step 6: Commit** `feat(scheduler): plan with lock window, pinning and hysteresis`.

### Task 9: `radio-cli plan`

**Files:** Create `apps/radio-cli/**`, `examples/station.toml`. Test: `apps/radio-cli/tests/cli.rs`.

**Interfaces:**
- Consumes: `Station` (TOML), `plan`, `PlanOptions::default()`.
- Produces: `radio-cli plan <station.toml> --now <RFC3339> [--hours N=24] [--json]`.
  - **Text output:** one line per entry, `HH:MM:SS  <playlist name padded to 10>  <artist – title or title>  <m:ss>`,
    with `⏱` before fixed entries and a line after each fixed entry with `fit: ±S.s s ✓|✗`.
    Times are local to the station's time zone.
  - **Warnings** go to stderr. The exit code is 0, or 2 when any warning is
    `DeadAir` / `OutsideTolerance`.
  - `--json` prints the serialised `Timeline`.
  - The command also exists as an MCP tool in M3; for now it's CLI only.
- [x] **Step 1: Write `examples/station.toml`**, which mirrors the spec's mockup:
  - **Music** (main): 40 tracks, shuffled with seed 1.
  - **Jingles:** a station ID every 3 tracks, `NotAdjacentToSelf`.
  - **Ads:** at :20 and :50, take 3, not trimmable, `MaxPerDay(24)`.
  - **News:** a stream `At 15:00 Daily`, `Fixed Hard`, tolerance 1 s, planned 5 min, priority 10.
  - **Shows:** `Jazz Hour.mp3` `At 16:00 Daily`, `Fixed Soft`, planned 58:40.
  - **Beds:** `is_fill_pool`.
- [x] **Step 2: Failing tests:**
  - `plan_example_prints_news_at_1500`: the output contains `15:00:00  News` and `fit: +0.0 s ✓`, or within ±1.0.
  - `json_roundtrips`.
  - `invalid_station_exits_1_with_item_name`.
  - `dead_air_exits_2`.
- [x] **Step 3: Run** `cargo test -p radio-cli`. Expected: FAIL.
- [x] **Step 4: Implement** `main.rs` with clap. Map `ModelError` to exit code 1 with its message.
- [x] **Step 5: Run** `cargo test --workspace`. Expected: all PASS. Run
  `cargo run -p radio-cli -- plan examples/station.toml --now 2026-11-21T13:00:00Z --hours 4`
  and check by eye that ads come in threes at :20 / :50, the news is at 15:00:00 and the show is at about 16:00.
- [x] **Step 6: Commit** `feat(cli): radio-cli plan prints the on-air timeline`.

### Task 10: UI preview (egui), the two-pane playlist over the real plan

Added at the user's request ("see the UI first"). There's no audio: the decks and the cartwall
are visual only. Times come from `plan()` run against `examples/station.toml`.

**Files:** Create `crates/ui-egui/**` (`radio-ui-egui`) and `apps/radio/**` (the desktop binary).

**Interfaces:**
- Consumes: `Station`, `plan`, `PlanOptions`, `Timeline`, `Entry`, `Origin`, `FitResult`.
- Produces: `radio_ui_egui::RadioApp::new(station: Station, now: Timestamp) -> RadioApp`,
  which implements `eframe::App`. Binary: `radio [station.toml] [--now RFC3339]`. It defaults to
  `examples/station.toml` and to the system clock; this is the only place that reads the clock.

**Layout (spec §6.12 mockup):**
- **Top: deck strip.** Four deck cards, A–D. Each shows the item it would hold (from the first
  four timeline entries), a progress bar driven by the clock, and the mode label `AUTO`.
- **Middle left, Main.** The main playlist entries in an `egui_extras::TableBuilder` that
  only draws visible rows, with the columns start time (`HH:MM:SS`, local), artist – title and
  length. Inserted entries appear as thin divider rows coloured with their playlist's colour.
- **Middle right.** One tab row: `Upcoming`, then one tab per non-main playlist, then `[+]`
  (disabled).
  - *Upcoming* lists every non-main entry by time, with a colour dot, the playlist name, the
    title and the length. Fixed entries show `⏱` and a line `fit: ±S.s s ✓|✗` from their
    `FitResult`.
  - A playlist tab shows its items, its pick order and a read-only summary of its rules.
- **Bottom.** Cartwall: a 4×4 grid of buttons labelled with the station's Jingles items
  (visual only), and a "■ stop all carts" button. Under it, a status line with
  `Local ●  Icecast ●` placeholders and a count of warnings.
- **Linked selection:** clicking a row in either pane highlights the matching entry in the other.
- **Updates:** `plan()` re-runs once a second with the current time and the previous timeline.
  The clock (`HH:MM:SS`) is shown in the top-right.

- [x] **Step 1:** Add the workspace deps `eframe = { version = "0.36", default-features = false, features = ["default_fonts", "glow", "wayland", "x11"] }`, `egui = "0.36"` and `egui_extras = "0.36"` (choose the nearest working versions; note them in the report).
- [x] **Step 2:** One test, `crates/ui-egui/tests/smoke.rs`, using `egui_kittest`: build
  `RadioApp` from the testkit station at a fixed `now`, run one frame, and assert that the
  texts `Upcoming` and `15:00:00` exist.
- [x] **Step 3:** Implement. Keep panels in separate files: `decks.rs`, `main_pane.rs`,
  `scheduled_pane.rs`, `cartwall.rs`, `app.rs`.
- [x] **Step 4:** Run `mise exec -- cargo test -p radio-ui-egui`. Expected: PASS. Then
  `mise exec -- cargo run -p radio -- --now 2026-11-21T13:58:00Z` opens the window. Take a
  screenshot to check it.
- [x] **Step 5: Commit** `feat(ui): two-pane playlist preview over the scheduler`.
