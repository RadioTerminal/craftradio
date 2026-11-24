# Station server: design

**Status (2026-11-24):** design, not built. Awaiting operator approval.

**Related:** `docs/superpowers/plans/2026-11-22-p2p-listener-relay.md` (the signed listener
relay this builds on; its §6 item 2, "the station as its own tracker", was dropped because a
home station has no certificate. A hosted server has one.)

In this document, "decided" marks what the operator said. "Call" marks a choice made while
writing, which the operator can overrule.

## 1. Intent

The operator runs a few stations and today needs a TinyIce (or Icecast) server, two public
WebTorrent trackers and GitHub Pages around them. They want one small server of their own
that does all of it, and a setup that is one step: enter the server address and a key in the
app, and the outputs configure themselves.

They also want what TinyIce calls "Go Live": someone outside broadcasts from a browser page.
Here that audio does not go straight on air. It arrives in the app as an input the operator
puts on air.

## 2. Goals, non-goals, anti-goals

**Goals**

1. One public server binary, written in Rust, serves several stations from a config file.
2. Each station has its own key. Everything that sends audio, reads settings or reads the
   guest feed needs a key (decided: "make sure everything is source protected").
3. In the app, server address plus key fills in the Icecast output and the WebRTC output
   (decided).
4. A guest broadcasts from a browser page on the server; the app plays that as an input,
   with the address filled in already (decided).
5. The server turns the one stream a station pushes into any number of HTTP outputs with
   their own format and bitrate, set in the server config (decided, 2026-11-24: for example
   a 320 kbps Opus source also served as 192 kbps MP3).
6. Listeners get an Icecast-style HTTP stream and WebRTC from the server. The app keeps its
   own WebRTC output and the signed listener relay; the server is the meeting point and also
   an always-on first relay (decided: option C).

**Non-goals**

- Accounts, sign-up, an admin UI, or stations of other people. The config file is the
  admin interface (decided).
- Talkback or a return feed to the guest.
- Video, HLS, RTMP, SRT, AutoDJ, OIDC, passkeys. CraftRadio is the AutoDJ.
- Transcoding of anything but the station's pushed stream into its configured outputs
  (§5.6). The guest feed and the WebRTC packets are never re-encoded.
- TURN. Listeners who can't connect over WebRTC play the HTTP stream.

**Anti-goals**

- The station's Ed25519 signing key on the server. It never leaves the app.
- A key in a playlist item, the station file, a log or MCP output (rule 11).
- A paired state that shows green while the server is unreachable or refuses the key
  (rule 7).
- A guest going on air without the operator putting them there.

## 3. Constraints

- **Rule 4 exception (operator, 2026-11-24):** the server's pages may use a JavaScript
  framework that needs no build step. Call: Alpine.js (MIT), one minified file vendored in
  the repo and embedded in the binary, never loaded from a CDN. The guest page is one HTML
  file with no build step. `AGENTS.md` rule 4 gets this second exception recorded when
  phase 4 lands. `site/index.html` stays as it is, without a framework.
- Rules 6, 7, 9, 11, 12 of `AGENTS.md` apply as written. New crates must pass rule 12.
- The workspace is thread-based with no async runtime (`str0m`, `tungstenite`, `ureq`,
  `rustls` with ring). Call: the server stays that way.
- The Icecast output and the WebRTC output in the app use separate Opus encoders
  (`crates/stream/src/output.rs`, `webrtc.rs`), and the signed blocks cover exact bytes and
  `seq`. The server can't derive relay packets from the Icecast push.
- `StreamInput::open` takes only a URL today (`crates/stream/src/input.rs`).
- The listener page hard-codes the two public trackers (`site/index.html`, `TRACKERS`).

## 4. Approaches

How listeners get WebRTC once there is a server:

| | How | Trade-off |
|---|---|---|
| A. Meeting point | The app serves WebRTC and the signed relay as today. The server replaces the trackers and hosts the HTTP stream. | Smallest. The station's upload and router still decide who connects. |
| B. Hub | The app pushes one stream; the server serves HTTP and WebRTC to everyone. | Works behind any router. The server pays for every listener; the signed relay doesn't fit. |
| C. Both | A, and the server joins the mesh as an always-on relay. | Best reach, largest build. |

**Picked: C (operator, 2026-11-24).** It is built as A first (phases 1–3), then the guest
page (phase 4), then the relay (phase 5), so each phase is usable on its own.

## 5. The server

A new binary, `craftradio-server`. Call: its logic lives in a new crate `crates/server`
(depends on `radio-stream` and `codecs` only, tests in `tests/it/main.rs`), with a thin `apps/radio-server`
around it. It reuses `OggMuxer`, `packets`, the tracker message shapes and the `str0m` code
from `radio-stream`, moving shared pieces rather than copying them (moves are their own
commits, rule 3).

### 5.1 Config file

TOML. Example with invented values:

```toml
listen = "127.0.0.1:8080"        # HTTP and WebSocket, behind the reverse proxy
media = "0.0.0.0:8443"           # UDP, WebRTC media, reachable directly
public_url = "https://radio.example.org"
public_ip = "203.0.113.5"        # the address listeners reach `media` on
trusted_proxy = "127.0.0.1"      # whose X-Forwarded-For is believed

[[station]]
slug = "morning"
name = "Morning Station"
key_hash = "…"                   # station key
guest_key = "…"                  # optional, in the clear; without it, the guest page is off
max_stream_listeners = 200
max_webrtc_listeners = 100

[[station.output]]               # any number; see §5.6
mount = "stream.mp3"
format = "mp3"
kbps = 192

[[station.output]]
mount = "low"
format = "opus"
kbps = 64
```

- Call: the file holds the **hash** of the station key, not the key. `craftradio-server key`
  prints a new random key once and the line to paste into the config. Comparison is
  constant-time.
- The guest key is in the clear (decided, 2026-11-24), so pairing can hand it to the app.
  It only allows sending guest audio and reading the guest state. The config file should
  be readable by the server's user only; the server warns at start when it is not.
- Call: TLS is the reverse proxy's job (Caddy, nginx) for HTTP and `wss://`. The server
  speaks plain HTTP on `listen`. The UDP media port is not proxied. `public_ip` replaces
  STUN on the server.
- **Risk to settle first in phase 1:** the app's Icecast source request
  (`output.rs`, `connect()`) is an HTTP/1.0 `PUT` with an endless `Content-Length`, and it
  takes silence as "accepted". A proxy that buffers request bodies (nginx by default) would
  pass no audio while the app reports success. Call: phase 1 tests the push through the
  sample Caddy config that ships in `packaging/`, and documents the nginx setting
  (`proxy_request_buffering off`). The server also takes optional `tls_cert` and `tls_key`
  paths and then serves TLS itself with `rustls`, with no proxy; that is the fallback if the
  proxied push can't be made reliable. The server answers a source `PUT` with an explicit
  status line at once, so the app's "accepted" is never a guess.
- The file is read at start. Reloading without a restart is left to the builder.

### 5.2 Endpoints

`<slug>` is a station's slug. "Station key" and "guest key" are sent as
`Authorization: Bearer …`, except where Icecast's Basic auth is named.

| Endpoint | Who | What |
|---|---|---|
| `PUT /<slug>/stream` | station key as the Basic password, any user | Icecast source: Ogg/Opus or MP3, as the app's Icecast output sends it |
| `GET /admin/metadata?mount=/<slug>/stream&mode=updinfo&song=…` | same | title updates, as the app sends them |
| `GET /<slug>/stream` | public | the HTTP stream, bytes as pushed, starting at a page or frame boundary |
| `GET /<slug>/<mount>` | public | a configured output (§5.6), re-encoded on the server |
| `GET /status-json.xsl` | public | Icecast-shaped stats with the title per mount, which the listener page already reads |
| `GET /api/station` | station key | the pairing document (§6.1) |
| `GET /<slug>/` | public | the listener page (`site/index.html`), phase 3 |
| `WS /<slug>/signal` | public | WebTorrent tracker messages for this station's swarm only, phase 3 |
| `WS /<slug>/feed` | station key | the app's signed packets, blocks and "now" messages, phase 5 |
| `GET /<slug>/live` | public | the guest page, phase 4 |
| `POST /<slug>/live/whip` | guest key | WHIP: SDP offer in, answer out, phase 4 |
| `GET /<slug>/live/feed` | station key | the guest's audio as Ogg/Opus, phase 4 |
| `GET /<slug>/live/state` | guest key or station key | whether a guest is connected and whether the station is reading the feed |

- One source per mount. A second `PUT` while one is live is refused.
- The mount path the app pushes to is `/<slug>/stream`. That mount takes the format the
  source sends and is served as pushed. Other formats and bitrates are outputs (§5.6).
- **Wrong keys:** five failures from one address within a minute lock that address out of
  all keyed endpoints for five minutes (HTTP 429). The address is the socket's, or
  `X-Forwarded-For` when the socket is `trusted_proxy`. Call: these numbers.
- Listener caps are per station. Over the cap, the HTTP stream answers 503 and the server
  stops answering WebRTC offers.
- Logs never contain keys, and listener addresses are not logged at the default level.

### 5.3 Signalling (phase 3)

`WS /<slug>/signal` speaks the subset of the WebTorrent tracker protocol that
`crates/stream/src/relay.rs` and `site/index.html` already use (announce, offers, answers).
The swarm is the slug's; the `info_hash` in a message is passed through and not used to pick
a swarm. Trust is unchanged: the station signs its answers with its Ed25519 key, and relay
answers are unsigned and protected by the signed blocks. The server needs no key for this
and holds none.

Per-connection and per-address limits on offers follow the ones in `relay.rs`
(`OFFERS_PER_SECOND`, `MAX_MESSAGE`).

### 5.4 Relay (phase 5)

- The app opens `WS /<slug>/feed` with the station key and sends, as binary and text
  messages, exactly what it sends a listener on the `audio` and `now` channels: audio
  packets (`KIND_AUDIO`), signed blocks (`KIND_BLOCK`), the relay hello and now-playing.
  Call: a WebSocket over the connection the app dials out, not a WebRTC link from station to
  server. It works behind any router and reuses the key check.
- The server answers offers in the station's swarm itself, as a relay: an unsigned answer
  with `a=x-radio-relay:1;<free>`, data channels matched by label, packets forwarded with
  `hops` = 1, blocks and "now" messages passed on unchanged. It decodes nothing.
- A page must be able to start from the server alone, with the station unreachable. The
  page's choice of sources (`site/index.html`, around the tracker answers) is checked
  against this in phase 5 and changed if it insists on a signed station answer first.
- **How it fits the page's rules** (calls):
  - A signed station answer still wins, as today. The server is the second source of a
    page that has the station, and the first source when the station is full or
    unreachable. It is not there to take load off a reachable station.
  - The server marks its answers `a=x-radio-server`. A page with "Help others listen" off
    (offers marked `a=x-radio-station-only`) accepts such an answer when it came through
    the server's own signalling address: the server is the station's own infrastructure,
    and no other listener sees that page's address. The packets are still verified against
    the station's blocks. The wording of the sharing switch does not change.
  - The station's `webrtc.relay = false` stops listeners from relaying, as today. It does
    not turn the server relay off; unpairing does.
- The server sends no RTP track. A browser that can't play packets (no Opus in
  `AudioDecoder`) plays `/<slug>/stream`.
- Listener counts: the server reports its direct listeners and what pages report to it up
  the feed connection. The app shows them as reported, apart from its own direct count, as
  the relay plan §7 already requires.

### 5.5 Guest DJ (phase 4)

- The guest opens `https://server/<slug>/live#<guest key>`. The key is in the fragment, so
  it reaches neither the server's access log nor a referrer. The page sends it as a Bearer
  header.
- The page (Alpine.js): input device picker, a level meter, a limiter switch, Start and
  Stop. It captures with `getUserMedia`, and sends Opus over one `RTCPeerConnection`
  through WHIP.
- The server receives the RTP with `str0m`, takes the Opus packets as they are and muxes
  them to Ogg for `GET /<slug>/live/feed`. No decoding.
- One guest per station at a time. A second gets "busy", in words.
- The page shows three states in words, never by colour alone: not connected; connected,
  the station is not listening; connected, the station is receiving you. "Receiving" means
  the app is reading the feed. The page never says "on air", because the server can't know
  that (rule 7).
- Keyboard use, labels and contrast as rule 10 asks of the app.

### 5.6 Outputs (phase 1b)

- A station has any number of `[[station.output]]` entries: `mount`, `format` (`opus` or
  `mp3`) and `kbps`. Each is served at `/<slug>/<mount>`. A mount name can't be one of the
  station's fixed paths (`stream`, `live`, `signal`, `feed`).
- The server decodes the pushed stream once per station (48 kHz stereo) and encodes once
  per output, whatever the number of listeners on it. It uses what the workspace already
  has: the decoders of `crates/codecs` (Symphonia, native Opus) and the Opus and MP3
  encoders of `radio-stream`. No FFmpeg and no new codec crate.
- `/<slug>/stream` stays the pushed bytes, untouched. An output is never an "upscale" in
  quality, but the server does not refuse a bitrate above the source's; that is the
  operator's choice.
- Call: an output is encoded only while it has listeners, and the decoder runs only while
  some output does. The first listener of an idle output waits for the encoder to start
  (well under a second). An idle server costs no CPU for outputs.
- Call: `max_stream_listeners` counts the pushed mount and all outputs of the station
  together.
- Titles reach every output. MP3 outputs send ICY metadata to listeners that ask for it
  (`Icy-MetaData: 1`). `/status-json.xsl` lists every mount with its format, bitrate and
  listener count.
- When the source stops, every output ends as the pushed mount does. When the source's
  format changes on reconnect, outputs restart their encoders.
- The pairing document lists the outputs with their public addresses, so the app can show
  them. Call: the app's `public_stream`, which the listener page falls back to, stays the
  pushed mount unless one output has `default = true`.

## 6. The app

### 6.1 Pairing (phase 2)

- A new Preferences page, "Server" (rule 9): server address, key, a Connect button, and
  the state in words. The key goes to the keychain through the `SecretStore` the Icecast
  password already uses.
- Commands `server.pair` and `server.unpair` in `engine` (rule 6), so the CLI and MCP can
  pair too. `server.pair` fetches `GET /api/station` and writes ordinary settings:
  - `icecast.*`: host, port, mount, user, TLS, public URL; the station key is stored as that
    mount's password in the keychain.
  - `webrtc.*`: the server's `signal` address is added in front of the tracker list. The
    public trackers stay (decided, 2026-11-24), so listen links already shared through
    GitHub Pages keep working. `public_stream` becomes `/<slug>/stream`.
  - a `server.*` group: address, slug, and what the pairing document offered (signal, feed,
    guest feed, guest page).
- **The key is never a command argument** (call), because arguments travel through the
  control socket, MCP input and scripts. `server.pair` carries the address only and reads
  the key from the keychain. The Preferences page stores the key there first, the way the
  Icecast password is stored today (`crates/ui-engine/src/icecast.rs`); the CLI reads it
  from standard input or the `CRAFTRADIO_SERVER_KEY` environment variable and stores it the
  same way. The fetch and the keychain access run where `SecretStore` lives, not in
  `engine`; the engine command records the address and the fetched settings.
- The pairing document is JSON and versioned (`"version": 1`). The builder defines its
  fields from the list above.
- The app fetches it again at each start. Settings a person changed by hand after pairing
  are overwritten on the next fetch; the Server page says so. `server.unpair` restores the
  defaults for the settings pairing wrote, and removes the key.
- States shown (rule 7): not paired; paired and reachable; paired, server unreachable;
  paired, key refused. Only the second is a success state. The headless app prints the
  same.
- The listen link the app shows becomes `https://server/<slug>/#<station id>` while paired.

### 6.2 The guest input (phase 4)

- The playlist "Add" menu gets "Add guest DJ…" while the pairing offers a guest feed. It
  adds an ordinary `Source::Stream` item whose URL is the feed address. No model change.
- `StreamInput` gains an optional request header. Playout attaches the station key from the
  keychain when a stream's URL is the paired server's guest feed, and for no other URL. The
  key is never written into the item.
- The pairing document carries the guest key (decided, 2026-11-24). The app keeps it in
  the keychain, not in settings. The Server page shows the full guest link,
  `https://server/<slug>/live#<guest key>`, with a Copy button, and whether a guest is
  connected (`GET /<slug>/live/state` with the station key). The link is shown on that page
  only; it is not part of the station view that the CLI and MCP print.
- An empty feed (no guest connected) behaves as a stream that is down does today, including
  keep-alive.

### 6.3 The feed to the relay (phase 5)

While paired and the WebRTC output runs, it also keeps `WS /<slug>/feed` open and sends
each packet, block and "now" message there. A feed that is down never stops the output;
its state shows on the output's status line in words.

### 6.4 The listener page (phase 3)

`site/index.html`, served by the server at `/<slug>/`, uses `wss://<its origin>/<slug>/signal`
first and the public trackers after it. Opened without `#<station id>`, it plays the station's
default HTTP stream and does not try WebRTC: the server doesn't know the station's public
key, and pairing stays a single GET (call). Opened from GitHub Pages with an old link, it behaves as
today. The page stays one file with no build step and no framework.

## 7. Phases

Each phase is its own branch and PR, and ends usable.

1. **Server core.** Config, key hashing, lockout, Icecast source in, HTTP stream out, title
   updates, stats. Done: the app's existing Icecast output, set up by hand, streams to the
   server, and a browser plays `/<slug>/stream` with titles; a wrong key is refused and
   locked out.
   **1b. Outputs.** §5.6, as its own change after the core. Done: with a 320 kbps Opus
   source, a configured 192 kbps MP3 output plays in a browser with titles, and an output
   with no listeners uses no encoder.
2. **Pairing.** `/api/station`, the commands, the Preferences page. Done: on a fresh
   station, address plus key gives a working Icecast output with no other field touched.
3. **Meeting point.** Signalling, the page served by the server, the listen link. Done: with
   the public trackers unreachable, a listener on the server's page connects over WebRTC,
   and an old GitHub Pages link still connects through the public trackers.
4. **Guest DJ.** WHIP ingest, the feed, the guest page, "Add guest DJ…", rule 4 updated.
   Done: a guest in a browser is heard on a deck in the app, and without the station key the
   feed is refused.
5. **Relay.** The feed socket, the server as relay. Done: with the station's WebRTC port
   blocked, a listener still plays verified packets through the server.

Tests guard behaviour (rules 14–17): keys and lockout, one source per mount, a listener
joining mid-stream gets a decodable stream, caps, signalling stays inside one station's
swarm, forwarded packets verify against the station's blocks, a tampered packet is dropped
by the page as today.

## 8. Decided by the operator (2026-11-24)

1. **Guest link in the app.** The server config holds the guest key in the clear, pairing
   hands it to the app, and the Server page shows the full link (§5.1, §6.2).
2. **Public trackers while paired.** Kept alongside the server's signalling, so old listen
   links keep working (§6.1).

## 9. Left to the builder

- The HTTP layer (hand-rolled as in `webrtc.rs`, or a small crate that passes rule 12).
- Threads per listener or one fan-out thread, within the stated caps.
- Field names of the pairing document and of the `server.*` settings.
- Config reload, log format, packaging (a systemd unit and a sample proxy config are
  welcome in `packaging/`).
- Which shared pieces of `radio-stream` move, and where.
