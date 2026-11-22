# The control channel

A running station (`craftradio`, with or without `--headless`) listens for the CLI and the MCP
server on a loopback TCP port. It is craft-control's channel from storytold/craft-libs.

## Finding it

- The port is `--control PORT`, else `CRAFTRADIO_CONTROL_PORT`, else 17470 or the next free one
  of 16. `--no-control` starts no channel; a channel that cannot start is reported at launch, and
  the station runs without it.
- Its files are in `<data folder>/radio-station-manager/control/` (`CRAFTRADIO_CONTROL_DIR`
  moves them): `current.port` names the port of the station started last, and `<port>.token`
  holds that launch's token. Both are readable only by the user who runs the station (`0600`
  in a `0700` folder; one ACL entry on Windows), and both are removed when the station stops.
- The folder belongs to the user, not to a `--data-dir`: two stations of one user share it, so
  pass `--port` to reach one that was not started last. When the station started last stops
  first, its discovery file goes with it, and a client finds the other by its token file
  (with several left, it asks for `--port`).

## Talking to it

One JSON request per line, one JSON reply per line. The first request of a connection carries
the token; a connection without it is closed.

```json
{"id": 1, "method": "station.run", "params": {"id": "playlist.create", "args": {"name": "Night"}}, "token": "…"}
{"id": 1, "ok": true, "result": {"audio_required": false, "applied_to_audio": false, "data": …}}
```

| Method | Params | Result |
|---|---|---|
| `station.commands` | none | every engine command: `id`, `summary`, `tier`, `args_schema` |
| `station.run` | one engine command, `{"id": …, "args": {…}}` | its outcome: `data`, `note`, `audio_required`, `applied_to_audio` |
| `ui.screenshot` | none | the main window as PNG: `png` (base64), `mime`, `width`, `height`; an error when the station runs `--headless` and has no window |
| `control.capabilities` | none | the methods above |

A failure is `{"ok": false, "error": "…"}`. A command runs on the playout as a panel's would, so
the window shows its change. `craftradio-cli commands` lists the ids.

## MCP

`craftradio-cli mcp` serves MCP on stdio for an AI client: every engine command is a tool named
by its id with `_` for `.` (`schedule.add` is `schedule_add`), with the command's argument
schema. Read-only commands are marked read-only; the others are marked destructive and say they
change the running station. A call runs on the station started last, or on `--port PORT`; the token comes from
`CRAFTRADIO_CONTROL_TOKEN` or the token file, never from the command line. With no station
running, a call is an error result. One more tool, `screenshot`, returns the app's main window as
an image, so a client can see what the operator sees; a `--headless` station has no window, and
the call says so. For Claude Code:

```sh
claude mcp add craftradio -- craftradio-cli mcp
```
