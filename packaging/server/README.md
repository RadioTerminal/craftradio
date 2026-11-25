# Running craftradio-server on a host

`craftradio-server` takes the radio app's Icecast push for each station and serves it to
listeners at `/<slug>/stream`, with Icecast-shaped stats at `/status-json.xsl`. It can
also serve each station in other formats and bitrates (see "Outputs"). All names, hosts
and addresses below are invented; use your own.

Files here:

- `craftradio-server.example.toml`: a config with two stations, two outputs and a comment
  for every key.
- `craftradio-server.service`: a systemd unit.
- `Caddyfile`: Caddy in front of the server, for one host.

## Install

1. Build the binary and copy it to the host:

   ```sh
   mise exec -- cargo build --release -p craftradio-server
   install -m 0755 target/release/craftradio-server /usr/local/bin/craftradio-server
   ```

2. Make a user for it, and its config directory:

   ```sh
   useradd --system --user-group --no-create-home --shell /usr/sbin/nologin craftradio
   install -d -o root -g craftradio -m 0750 /etc/craftradio-server
   install -o root -g craftradio -m 0640 craftradio-server.example.toml \
     /etc/craftradio-server/config.toml
   ```

3. Edit `/etc/craftradio-server/config.toml` (see below for keys), then start it:

   ```sh
   install -m 0644 craftradio-server.service /etc/systemd/system/
   systemctl daemon-reload
   systemctl enable --now craftradio-server
   journalctl -u craftradio-server
   ```

   The log says `listening on <address>, <n> station(s)`. A config it can't use stops it
   with a message naming the file.

## Container

Each `server-vX.Y.Z` tag publishes `ghcr.io/radioterminal/craftradio-server:X.Y.Z` (and
`X.Y`, and `latest` for a version without a suffix) for linux/amd64 and linux/arm64. It
runs as an unprivileged user and reads `/etc/craftradio-server/config.toml`; in that
config, `listen` must be `"0.0.0.0:8080"` (or another port you publish), not loopback.

```sh
docker run --rm ghcr.io/radioterminal/craftradio-server key
docker run -d --name craftradio-server --restart unless-stopped -p 8080:8080 \
  -v /etc/craftradio-server/config.toml:/etc/craftradio-server/config.toml:ro \
  ghcr.io/radioterminal/craftradio-server
```

Behind a proxy on the same host, publish the port on loopback only (`-p 127.0.0.1:8080:8080`)
and set `trusted_proxy` to the address the proxy's requests come from as the container
sees them (the Docker bridge's gateway, often `172.17.0.1`), not `127.0.0.1`. For the
server's own TLS, mount the certificate and key read-only and point `tls_cert`/`tls_key`
at them inside the container. `packaging/server/Dockerfile` builds the same image locally.

To let the app change outputs (see "Change outputs from the app"), set
`state_dir = "/var/lib/craftradio-server"` in the config and add
`-v craftradio-state:/var/lib/craftradio-server` to `docker run`. The image makes that
directory writable by its user, and a named volume keeps it across container updates.

## Make a station key

```sh
craftradio-server key
```

It prints the key once and a `key_hash = "sha256:…"` line. Put the `key_hash` line in that
station's `[[station]]` block and keep the key for the app; the config never holds the key
itself. Each station needs a key of its own; a `key_hash` that is not `sha256:` and 64 hex
digits stops the server at start. Restart the server after changing the config.

A key holder can make the server spend CPU (for example by pushing crafted Ogg pages it
must check), so stations that share a small host trust each other's operators that far.

## Route 1: behind Caddy (or nginx)

**Not checked.** Route 2, the server's own TLS (`tls_cert`/`tls_key`), is the route that
has been checked with the app; this one has not (see "Checked" below). The risk is
specific: the app sends an HTTP/1.0 `PUT` with an endless `Content-Length`, and the
server answers `200` at once while it goes on reading the body. The proxy must pass that
upload full-duplex and unbuffered: hand the `200` back to the app before the body ends,
and pass the body on as it comes. A proxy that waits for the whole body never delivers
any audio.

The server listens on loopback and the proxy does TLS:

```toml
listen = "127.0.0.1:8080"
public_url = "https://radio.example.org"
trusted_proxy = "127.0.0.1"
```

Use `Caddyfile` from this folder with your host name. It is meant to pass the stream on
as it comes (`flush_interval -1`) and to leave request bodies unbuffered, which the app's
endless source push needs; neither has been checked.

**nginx:** in the `location` that proxies to the server, set

```nginx
proxy_request_buffering off;
proxy_buffering off;
proxy_http_version 1.1;
```

Without `proxy_request_buffering off` nginx waits for the end of the source push, which
never comes, so no audio reaches the server.

The Caddyfile and the nginx lines have not been checked against a running proxy (see
"Checked" below).

Behind a proxy:

- **Set `trusted_proxy`** to the proxy's address. Without it every client seems to come
  from the proxy, so they all share one lockout address: five wrong keys from anyone
  within 60 s lock every station out for five minutes.
- **The proxy must limit slow clients itself.** The server limits how many connections one
  address may have that are still sending their request head, but not for the trusted
  proxy, which every client shares. Set the proxy's own client timeouts and connection
  limits.

## Route 2: the server's own TLS (no proxy)

The server can serve TLS itself from PEM files:

```toml
listen = "0.0.0.0:8443"
public_url = "https://radio.example.org:8443"
tls_cert = "/etc/craftradio-server/fullchain.pem"
tls_key = "/etc/craftradio-server/privkey.pem"
```

Leave `trusted_proxy` out. Then:

- The server's user must be able to read both files (for example group `craftradio`,
  mode `0640`).
- The files are read only at start: restart the server after renewing the certificate.
- To listen on 443 as the unprivileged user, uncomment
  `AmbientCapabilities=CAP_NET_BIND_SERVICE` in the unit.
- **Open files:** each connection holds one file descriptor, and the default limit is
  often 1024 (on macOS 256). The unit sets `LimitNOFILE=4096`; raise it above the sum of
  every station's `max_stream_listeners` plus some room, then
  `systemctl daemon-reload && systemctl restart craftradio-server`.

## `public_url`

`public_url` must be the bare origin (`https://radio.example.org`, with a port if it is
not the default), with no path after it. The listener page fetches `/status-json.xsl` from
the origin, so the server can't live under a path prefix such as
`https://example.org/radio`. The server refuses to start with any other `public_url`,
because the pairing document at `/api/station` (which a station's key fetches) derives the
app's Icecast host, port and TLS from it.

## Pair the app with the server

Pairing is the main route: the app asks the server at `GET /api/station` with the station
key, and the server names the push (host, port, TLS, mount, user) and the stream listeners
open. Give the station's operator two things: the server's address (`public_url`, for
example `https://radio.example.org`) and the key from `craftradio-server key`.

- **In the app:** Preferences › Server, enter the address and the key, press Connect.
- **Without the window** (close the app first; a running app keeps its own copy of the
  station):

  ```sh
  craftradio-cli pair https://radio.example.org --data-dir /srv/morning < morning-key.txt
  ```

  The key is one line on standard input, or `CRAFTRADIO_SERVER_KEY`; it is never an
  argument and never printed. A wrong key exits non-zero with `key refused`; a server that
  does not answer, with `unreachable: …`.

The address needs `https://`; `http://` is only for a server on the same computer. The key
goes to the OS keychain, as the push password of the mount the server named, and only ever
to the address it was paired through: if that address changes, type the key again. The app
asks the server again at each start, before it pushes. The Server page and the headless app
(as `radio: …`) say one of:

- `Not paired`
- `Pairing with <address>…` (or `Paired — asking <address>…` at start)
- `Paired with <address>`, the only state that means it works
- `Paired — server unreachable: <reason>`: a down server leaves the last paired settings
- `Paired — key refused`: a key rotated on the server; nothing retries it, so it does not
  walk the lockout. Pair again with the new key.
- `Not paired — the server refused the key`, `… — server unreachable: <reason>` or
  `… — <reason>` when a first pairing did not go through

`craftradio-cli unpair --data-dir <dir>` (or Disconnect on the Server page) puts the
settings back and removes the key.

## Set the app's Icecast output by hand

The fallback, for a source that cannot pair (another Icecast client, or an app older than
pairing). Do not do both: a paired app overwrites these settings at each start. In the
app's Icecast output settings for the station:

| Field | Value |
|---|---|
| Host | your host, for example `radio.example.org` |
| Port | `443` behind a proxy; the `listen` port with the server's own TLS |
| TLS | on (off only for a server on loopback with no TLS) |
| Mount | `/<slug>/stream`, for example `/morning/stream` |
| User | anything, for example `source` |
| Password | the station key from `craftradio-server key` |
| Format | Opus or MP3 |

A wrong key shows in the app as refused with HTTP 401, never as connected. Listeners
open `https://radio.example.org/morning/stream`.

## Outputs

An output is the station's broadcast in another format or bitrate, which the server
re-encodes from what the app pushes. `/<slug>/stream` stays exactly what the app sends.
Add one `[[station.output]]` block per output under its station:

```toml
[[station.output]]
mount = "stream.mp3"   # served at /morning/stream.mp3
format = "mp3"         # "mp3" or "opus"
kbps = 192
```

- **Mount:** 1–32 of `a-z`, `0-9`, `-`, `.` and `_`, not starting with `-` or `.`, and
  not `stream`, `live`, `signal` or `feed`. Two outputs of a station can't share one.
- **Bitrate:** MP3 takes 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256 or 320;
  Opus takes 6 to 510. Anything else stops the server at start with a message. A bitrate
  above the app's is allowed but sounds no better.
- **The source must be 48 kHz stereo**, which the app's Icecast output always is. For any
  other source the outputs are off: a listener gets 503 with the reason, and
  `/<slug>/stream` still plays.
- **Cost:** while someone listens to an output, the server decodes the station's stream
  once and encodes once per output in use, however many listen to it. An output nobody
  listens to uses no CPU for encoding, and while no output has a listener nothing is
  decoded either. What remains is a short decode of the stream each time the app
  connects, to check its format, and a look for listeners 20 times a second. A listener
  of an idle output waits a moment for it to start.
- **Listener limit:** `max_stream_listeners` counts `/<slug>/stream` and all the station's
  outputs together.
- **Titles:** every output carries the titles the app sends. MP3 players that ask for
  titles (ICY) get them in the stream, on MP3 outputs and on `/<slug>/stream` when the app
  pushes MP3.
- The stats list each output while it plays, with its format and bitrate, after the
  station's `/<slug>/stream`.
- To let the station's operator change outputs from the app instead of editing this file,
  see "Change outputs from the app".

## Change outputs from the app

Set `state_dir` in the config to a directory the server can write:

```toml
state_dir = "/var/lib/craftradio-server"
```

With it, a paired app can list the station's outputs and save a new list (Preferences ›
Server › Outputs, or `craftradio-cli outputs`). The server saves them as
`<state_dir>/<slug>.outputs.json` and applies them at once.

- **The saved file overrides the config.** At start, a station with a saved file uses it
  and ignores its `[[station.output]]` blocks; a station without one uses the blocks.
  Changing the blocks later does nothing for a station that has a saved file. Delete the
  file to go back to the config. A saved file the server can't read, or a `state_dir` that
  is not a directory, stops the server at start with a message naming it.
- **Who may change them:** only the holder of the station's key, for that station only.
  The same limits as in the config apply (at most 8 outputs, the mount and bitrate rules
  above); a list that breaks them is refused with the reason and nothing changes.
- **Listeners of a changed output reconnect.** The push and `/<slug>/stream` keep playing.
  Listeners of the outputs are cut off when the list changes, and a player that reconnects
  gets the new list.
- **Without `state_dir`,** the server refuses the change (`this server keeps no state_dir:
  outputs are set in its config`) and the app shows the outputs read-only with
  `This server keeps its outputs in its config file.`
- **Install:** the systemd unit has `StateDirectory=craftradio-server`, which creates
  `/var/lib/craftradio-server` for the server's user. In the container, mount a volume
  there (see "Container").
- **Without the window,** with the app closed:

  ```sh
  craftradio-cli outputs --data-dir /srv/morning
  craftradio-cli outputs set --data-dir /srv/morning stream.mp3=mp3:192 low=opus:64
  craftradio-cli outputs set --data-dir /srv/morning
  ```

  The first prints `<mount> <format> <kbps> <url>` for each output; the second sends the
  whole list; the third, with no outputs, clears the list.

## Not yet available

`media`, `public_ip`, `guest_key`, `max_webrtc_listeners` and an output's `default` are
later phases; the server refuses a config that has them.

## Checked

2026-11-24: **Not checked through a proxy.** Caddy was not installed on the machine the
checks ran on, so neither the Caddyfile nor the nginx lines were run. What was checked,
by `crates/server/tests/it/e2e.rs`: the app's own Icecast output pushing to the server
directly, plain HTTP (Opus and MP3, heard back, titles in the stats, a wrong key reported
as HTTP 401) and over the server's own TLS (MP3, heard back over https by the app's
stream input). Outputs (phase 1b), plain HTTP: the app's Opus push heard as MP3 by the
app's own stream input with the titles it set, on a lower Opus output, and the app's MP3
push heard on an Opus output.
