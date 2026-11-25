# Deck scrubbing — design

**Why.** The operator cannot move the position of a deck. The progress bar only shows it, and
"Cue to start" in the deck menu is a stub that says "Not in this build yet".

**Goal.** Drag or click the progress bar of a deck to move its position, on any deck with a
file: waiting, paused or on air. "Cue to start" moves it to the start. The CLI, MCP and
scripts can do the same.

**Decided by the operator (2026-11-25).** A deck on air can be scrubbed, and without the
arm-then-confirm guard that a cut has.

**Not in this change.** Hearing the audio while dragging. Stop, Eject and Reload (they stay
stubs that say so). Seeking a stream or a live input: they cannot seek, the bar does not
react, and the command fails.

## Approaches

1. **Swap the source of the deck (picked).** A worker opens a second decoder at the new
   position. The mixer keeps playing the old audio until the new ring is full, then swaps
   the two in one frame and reports the frame. No gap on air, no file I/O on the engine
   thread, nothing new on the render thread but a pointer swap.
2. **Unload and load again at an offset.** Reuses the resume path, but the mixer refuses a
   load on a deck on air, and the deck would be silent while the file opens.
3. **Seek the running decoder.** Needs a channel into the feeder thread and a way to drop
   the stale audio of the ring from the render side; more moving parts for the same result.

## Design

- **Audio.** `MixerHandle::seek_decoder(deck, decoder, item_tag, id)` sends a source that
  waits beside the one on the deck. When its ring is ready the mixer swaps it in, keeps
  play / pause, item gain and fader, drops any fade-in on a started deck, and emits
  `AudioEvent::Seeked { deck, item_tag, id, at_sample }`. A newer seek replaces a waiting
  one. A seek for another item tag, or for a deck fed from outside, is refused.
- **Playout.** `AudioIntent::Seek { deck, expect_item, to }`, `to` counted from the cue-in
  and limited to the planned length. Refused for anything but a file, while the file still
  opens, and once the end fade is sent. On `Seeked` playout sets the offset and, for a
  started item, the start, as a pause does today: the planned end and the items after it
  move, the log row stays. The deck view gains `position`, `len` and `seekable`; the
  position stands still while paused.
- **Engine.** Command `deck.seek { deck, position, expect_item? }`.
- **UI.** The progress bar of a seekable deck takes click, drag and focus. A drag shows the
  new position and sends one seek on release. Left / Right move 5 s. "Cue to start" sends a
  seek to zero and is off for a deck that cannot seek. The preview source (the mock) moves
  its own position.
