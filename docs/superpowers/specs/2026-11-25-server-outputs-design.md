# Managing a station's server outputs from the app

**Status (2026-11-25):** built.

## Intent

The operator wants to add, change and remove a station's re-encoded outputs (phase 1b's
`[[station.output]]`) from the app, in Preferences › Server once paired, instead of editing
the server's `config.toml` and restarting it. In Docker that file is mounted read-only.

## Approaches

- **A. The server keeps app-set outputs in a state directory (picked).** A new config key
  `state_dir`; the server writes `<state_dir>/<slug>.outputs.json` and applies it live.
- B. The server rewrites `config.toml`. Rejected: the file is read-only in a container, and
  rewriting it loses the operator's comments.
- C. Outputs only in memory. Rejected: a restart would silently bring the old ones back
  (rule 7).

## Design

- **Server config:** optional `state_dir` (a directory the server's user can write). Without
  it the station's outputs come from the config only and can't be changed from the app.
- **Saved outputs:** `<state_dir>/<slug>.outputs.json`, a JSON array of
  `{"mount", "format", "kbps"}`. At start a saved file replaces that station's config outputs;
  a file that does not parse or fails the output checks stops the server with a message
  naming the file, as a bad config does. Written to a temporary file in the same directory
  and renamed, so a crash never leaves half a file.
- **API:** `PUT /api/station/outputs`, keyed exactly as `GET /api/station` (Basic password =
  station key, the same lockout). Body: the JSON array, `Content-Type: application/json`,
  at most 16 KiB. Answers:
  - `200` with the new pairing document;
  - `400` with the reason in words (same checks as the config, plus at most 8 outputs);
  - `403` "this server keeps no state_dir: outputs are set in its config" when there is none;
  - `411`/`413` for a missing or too large body; `401`/`429` as for pairing.
  A refused change changes nothing, on disk or live.
- **Live:** the change applies at once. The pushed `/<slug>/stream` keeps playing. Every
  current output ends (its listeners are cut off and reconnect), the new set is built, and if
  the station is live the transcoder starts again on it. Stats and the pairing document show
  the new set at once.
- **Pairing document:** gains `outputs_editable: bool` (true when the server has a
  `state_dir`). The version stays 1; an app reading an older server takes a missing field as
  false.
- **App:** Preferences › Server, below the status once it says "Paired with …": an
  **Outputs** list. Each row: format (Opus / MP3), bitrate (kbps), mount, Remove. Below: Add
  and Save. Save sends the whole list; while it is sent the page says `Saving…`, then
  `Saved` or the server's reason. When the server can't store outputs, the list is shown
  read-only with that reason. The settings' `server.outputs` follow the document the server
  returns (`server.pair` again), so the listen links follow too.
- **Without the window:** `craftradio-cli outputs --data-dir <dir>` prints the station's
  outputs; `craftradio-cli outputs set --data-dir <dir> [<mount>=<format>:<kbps> …]` sends
  them (none clears them), with the key from the keychain entry pairing stored.
- The station key is never an argument, a setting, a log line or part of a URL.

## Left to the builder

Widget layout of the rows, the exact words of the hint under the list, the order of checks.
