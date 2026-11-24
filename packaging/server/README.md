# Running craftradio-server on a host

`craftradio-server` takes the radio app's Icecast push for each station and serves it to
listeners at `/<slug>/stream`, with Icecast-shaped stats at `/status-json.xsl`. All names,
hosts and addresses below are invented; use your own.

Files here:

- `craftradio-server.example.toml`: a config with two stations and a comment for every key.
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
`https://example.org/radio`.

## Set the app's Icecast output by hand

In the app's Icecast output settings for the station:

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

## Not yet available

`media`, `public_ip`, `guest_key`, `max_webrtc_listeners` and `[[station.output]]` are
later phases; the server refuses a config that has them.

## Checked

2026-11-24: **Not checked through a proxy.** Caddy was not installed on the machine the
checks ran on, so neither the Caddyfile nor the nginx lines were run. What was checked,
by `crates/server/tests/it/e2e.rs`: the app's own Icecast output pushing to the server
directly, plain HTTP (Opus and MP3, heard back, titles in the stats, a wrong key reported
as HTTP 401) and over the server's own TLS (MP3, heard back over https by the app's
stream input).
