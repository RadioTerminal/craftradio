# Off air: one switch that stops everything the station sends

**Status (2026-11-24):** built; the user-facing description is in the README, "Going off air".
Operator changes while building: the switch is confirmed by pressing twice, both ways; and the
On air button was removed from the Mixer, so the station switches from Playout › On Air and
its shortcut, and the main window's status bar shows the state.

## Intent

The operator wants one switch that takes the station off air: nothing leaves the
computer, neither the Icecast push nor WebRTC. Today Icecast is read once at start and
stays connected until the app quits; only WebRTC can be turned off while the app runs.

## Approaches

- **A. A saved On air / Off air setting (picked by the operator).** Playout follows it every
  tick, as it follows WebRTC's `enabled` today. It survives a restart.
- B. The same switch, not saved: every start is on air. Rejected: a crash or restart puts the
  station back on air without anyone choosing it.
- C. Off air also stops the decks. Rejected: it loses local preview while off air.

## Design

- A setting `on_air` (bool, default `true`) in the station's settings, changed with the
  existing `settings.set` command, so the panels, the CLI, MCP and scripts all reach it
  (rule 6). Call: no new command; a `station.on_air`/`station.off_air` pair can come later
  if the CLI needs a shorter name.
- Playout: while `on_air` is false, the Icecast connection is closed (its handle dropped)
  and the WebRTC server is stopped, whatever WebRTC's own `enabled` says. When it turns true
  again, Icecast reconnects with the config the app started with, and WebRTC follows its
  `enabled` again. Decks, the cartwall, events and the sound card go on as before.
- What shows (rule 7): the Icecast and WebRTC strips say "Off air", not "Not connected" or a
  fault, and are never shown live. The status bar shows "Off air" in words, with a colour
  that is not the success colour. The headless app and the station view the CLI prints say
  "off air" too.
- The switch: a large On air / Off air button in the output strips' area, an "On Air" item
  with a check mark in the Playout menu, and a shortcut.
- **Confirmed by pressing twice, both ways (operator, 2026-11-24)**, exactly as a cut of the
  item on air is today (`decks.rs`: the armed button, `size::ARM_S`, the ring that runs down
  the time left). The first press arms the button and says what the second does ("Press
  again to go off air" / "Press again to go on air"); a second press within the arm time
  switches; otherwise it disarms. The menu item and the shortcut arm the same way. The deck
  preference "Confirm a cut of the item on air" does not turn this off: going off air is
  never one press. A command (`settings.set`) from the CLI, MCP or a script switches at once.
- **Animated (operator, 2026-11-24).** On air: the button's dot pulses slowly in the on-air
  colour, as a live light does. Off air: still, in a colour that is not the success colour,
  with "Off air" in words. Switching crossfades between the two (egui's `animate_bool`), and
  while Icecast is reconnecting after going on air the button shows "Going on air…" with a
  running ring until Icecast says connected. The pulse redraws at the arm repaint interval
  only while on air and only for that button. Colour is never the only signal (rule 10):
  the words always change.
- Starting off air: when the saved setting is false at start, Icecast is not connected at
  all until it turns true.

## Tests

- Off air closes the Icecast connection (a test server sees the source leave) and stops
  WebRTC; on air reconnects both.
- A station saved off air does not connect at start.
- The output strips' text while off air is "Off air" and not live.
- One press of the On air button changes nothing; two presses within the arm time switch;
  two presses further apart than it don't.

## Left to the builder

The shortcut key, where exactly the toggle sits in the strips, and the colour token.
