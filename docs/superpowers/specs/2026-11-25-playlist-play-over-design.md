# Playlist items that play over the music — design

**Why.** Station IDs and jingles sound best laid over the music, ducking it the way a cart
does, not as a slot of their own between two songs. Today every playlist item plays on a
deck, one after another. Carts play on their own bus, but their ducking is kept and shown
and never sent to the mixer (roadmap M6).

**Goal.** A playlist setting says how its items play: *In sequence* (today) or *Over the
music*, with a duck depth. An over item lands where the tracks allow it, takes no airtime,
and ducks the music while it plays. Cart ducking goes live in the same change, since it is
the same mixer path.

**Decided by the operator (2026-11-25).**
- The setting lives on the playlist.
- The tracks decide where an over item plays (see Placement).
- Approach 1 below: the scheduler plans the overlap.
- The library detects intros and outros in this change, and the operator can set or fix
  both markers by hand.

**Non-goals.** Over items from a live input. Talk-over (a voice input ducking
Program).

**Anti-goals.** An over item never talks over vocals: if no intro or outro fits it, it plays
in sequence. Upcoming never shows a jingle where it won't play, and never shows one as
planned over a song when playout will not play it (rule 7).

## Approaches

1. **The scheduler plans the overlap (picked).** A plan entry can lie over another entry and
   takes no airtime. Placement is decided while planning, so Upcoming, the CLI and MCP show
   where each ID lands, and the times after it stay true. Playout fires it at its time.
2. **Playout decides at the handover.** Less scheduler work, but the plan still counts the
   jingle as airtime and shows it as its own slot: the plan would not be what airs.
3. **A rule fires a cart.** Reuses the cart bus, but carts are fixed slots on pages; the
   playlist, its rules and its constraints would not pick the item.

## Placement

For each entry of an *Over the music* playlist, on the planned neighbours:

1. **Over the next song's intro**, if the next entry has `intro_end` and
   `intro_end − cue_in ≥` the jingle's length. Starts at the next entry's start.
2. **Else over the current song's outro**, if the entry before it has `outro_start` and
   `segue − outro_start ≥` the jingle's length. Starts so it ends at that entry's segue.
3. **Else in sequence**, as today.

Hosts are only deck items of a file (not streams, live inputs, another over item, or an
over-playlist entry). The jingle's length is its planned length (cue-in to cue-out).

My calls, from the builder check:
- **One over item per handover.** The outro of song A and the intro of song B are one
  handover; it takes at most one over item, so the one reserved slot is enough. A second
  over item looks for another handover, then falls to in sequence.
- **Outro window ends at the host's segue**, the moment the next song starts, so an outro
  jingle never runs into the next song.
- **A trimmed host** (`trim > 0`, cut before a fixed event) takes no outro jingle: its end
  is not its outro.
- **Fixed and locked entries** of an Over playlist play in sequence, as today. Their time is
  what matters; laying them over music is a later step.
- **Rules and constraints.** If pulling an item out of the flow would make two neighbours
  break a `NotAdjacent…` constraint, the item stays in sequence. Rules keep counting an over
  item as played, as today.

## Design

- **Markers.** `Timing` gains `outro_start: Option<Duration>` (`#[serde(default)]`), beside
  the existing `intro_end`. Both count from the file start, like `cue_in`.
- **Detection (library).** `analyse` already measures energy in 100 ms windows. Smooth them
  over 1 s; the main level is the track's loudness less 6 dB (my call). `intro_end` is the
  first point after `cue_in` where the smoothed level reaches the main level; `outro_start`
  is the last point before `segue` where it is still at it. A marker within 1 s of its
  edge, or a track with no point at the main level, gets `None`. Tests use generated
  audio (a quiet lead-in, then a loud body, then a quiet ending) and assert that the markers
  fall inside the quiet parts' boundaries within a tolerance. Tracks analysed before this
  change get markers on their next analysis (the builder checks what re-analysis exists
  and says in the plan how existing libraries pick them up).
- **Hand markers.** Command `item.set_markers { item, intro_end, outro_start }` (each
  optional; `null` clears); refused when a marker lies outside cue-in…cue-out, or when the
  outro start is after the segue. A marker set by hand is kept when the file is analysed
  again. The item editor gets **Intro ends** and **Outro starts** fields (m:ss.s), with a
  hint that they're detected ("auto") or set by hand.
- **Model.** `Playlist.plays: PlayMode` with `#[serde(default)]`:
  `enum PlayMode { InSequence, Over { duck_db: f32 } }`, default `InSequence`.
  `duck_db` is 0 or lower (0 = play over without ducking).
- **Engine.** Command `playlist.set_plays { playlist, plays }`, like
  `playlist.set_on_interrupt`; refuses a positive or non-finite `duck_db`. CLI and MCP get it
  through the registry.
- **Scheduler.** `Entry` gains `over: Option<Over>`:
  `struct Over { host: ItemId /* plus the host's plan position */, side: Intro | Outro,
  duck_db }`. An over entry has its absolute `start` like any entry but is skipped when
  starts are recomputed and when airtime is counted. Placement runs as a pass after
  `fit_all`, on the final neighbours; an entry turned into an over item leaves the flow, and
  the starts after it are recomputed (they move earlier). Because that frees time before a
  hard fixed event, the pass runs before the final fill step, or `fit_all` runs again on
  the flow without them (builder's call, whichever the code allows; the result must be one
  plan where fixed events still fit).
  Rules keep counting an over item as played for between-tracks rules and separation
  constraints, as today.
- **Audio.** Reserve the last cart slot for playlist over items (the cartwall gets one
  fewer; the config value stays the total). Firing a cart or an over item with a duck below
  0 dB sends `Duck { db, attack, release }` at the same frame and `Unduck` when the last
  ducking cart on the bus ends (a count in the mixer, not one per cart). When ducking carts
  overlap, the deepest duck of those still playing wins (my call). Attack and release
  use fixed defaults (my call: 150 ms attack, 400 ms release), not settings yet.
- **Playout.** Loads the next over item's file into the reserved slot ahead of time (as
  `sync_carts` does), fires it relative to where its host really is, not the wall clock
  (my call): an intro item at the host's aired start frame, an outro item at the host's
  aired start plus the planned offset. Pausing or seeking the host before the fire frame
  skips the item, and writes it to the as-played log like
  any item, marked as over. If the file isn't loaded in time, or the host isn't on air at
  that moment (cut, paused, late), the over item is skipped with an alarm-free warning in
  the log and the next replan drops it; it is never fired over the wrong song.
- **UI.** The playlist's ⚙ rules editor gets a **Plays** row: *In sequence* / *Over the
  music*, and **Duck by** (dB, default −12, shown only for Over). Upcoming draws an over
  item as a thin row tucked under its host (playlist colour and name, "over intro" or
  "over outro", its start), not as a slot. The cartwall's "ducking kept, not played yet"
  note goes away.

## Tests (the behaviours worth guarding)

- Detection: markers land at the quiet/loud boundaries of generated audio; a track that is
  loud from the first second gets no intro marker.
- A hand marker survives re-analysis.
- Placement: intro wins when it fits; outro when only it fits; in sequence when neither;
  two over items next to one host don't overlap.
- An over item takes no airtime: the entries after it start earlier than they would in
  sequence, and a hard fixed event still starts on its target.
- Mixer: a ducking cart lowers decks and not carts; the duck lifts only after the last
  ducking cart ends.
- Playout: an over item fires at its frame over its host; it is skipped (and logged) when
  the host is not on air.

## Left to the builder

Where exactly the placement pass sits in `fit_all`, how `Over` names its host in the plan,
the CLI output format for over entries, and the exact Upcoming row layout within the theme
tokens.
