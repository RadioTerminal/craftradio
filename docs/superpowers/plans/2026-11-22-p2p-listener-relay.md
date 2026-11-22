# P2P listener relay: design

**Goal:** Listeners pass the station's audio on to other listeners. Then the number of
listeners no longer depends on the station's upload. A relaying listener cannot change the
audio without being caught. Second goal: the station needs fewer third-party servers.

**Status (2026-11-22):** design only. Nothing is built. The open decisions (§8) need the
operator before phase 1 starts.

**Spec:** `docs/2026-11-21-radio-station-manager-design.md` §2 item 8 (WebRTC listening, listen
link). Roadmap: `docs/superpowers/plans/2026-11-21-roadmap.md` (proposed row in §9).

**Changes the spec's stance.** §2 item 8 says WebRTC listening "is for quick trials, not a
CDN: each listener costs the station's own upload". With relaying, WebRTC becomes a way to
reach many listeners. That sentence has to change when phase 2 ships (§9).

## 1. Today

- `crates/stream/src/webrtc.rs`: str0m. Each listener is one `Rtc`. The Opus encoder makes
  20 ms packets (`PACKET` = 1920 samples) and writes each to every listener as RTP
  (`writer(mid).write(...)`, around line 481). `max_listeners` defaults to 50; `kbps` to 96.
  One data channel per listener carries "now playing" (`send_now`). The station takes the
  first channel the page opens (`Event::ChannelOpen(id, _)`) and ignores its label.
- `crates/stream/src/relay.rs`: the station keeps a WebSocket to each public WebTorrent
  tracker and announces in the swarm of its station ID (`numwant: 0`, `offers: []`). It
  answers the offers the tracker hands it, and signs each answer with its Ed25519 key over
  `offer_id + "\n" + sdp` (`a=x-radio-sig:` line).
- `site/index.html` (590 lines): one `RTCPeerConnection`. It posts that one offer SDP under
  five `offer_id`s and keeps the first answer whose signature checks out. Audio arrives as
  an RTP track in `<audio>`. The page keeps MediaSession and a level meter on that stream.
- `crates/stream/src/upnp.rs`: UPnP, else NAT-PMP, maps the UDP port and returns the
  router's external address (`get_external_ip`).
- Third parties: two trackers (signalling), Google and Cloudflare STUN, and GitHub Pages
  (the page; the station also serves it itself).

The shape is a star. Station upload ≈ listeners × (kbps + overhead): 50 listeners at 96 kbps
take about 5 Mbit/s.

## 2. Design summary

1. The station also sends Opus packets as **binary messages on an unreliable data channel**
   (`audio`). It sends signed packet hashes on a **reliable channel** (`sig`).
2. The page decodes with WebCodecs `AudioDecoder` and plays through an AudioWorklet. A
   listener that relays forwards the same bytes to its children. There is no decoding or
   re-encoding on any hop.
3. A listener with free slots stays in the station's tracker swarm and **answers offers
   itself**, like the station does.
4. Each listener takes audio from **two sources** (parents), drops duplicates by sequence
   number and replaces a source that fails.
5. Browsers without Opus in `AudioDecoder` keep today's path: RTP from the station, and they
   don't relay.

## 3. Wire format

### `audio` channel (unordered, `maxRetransmits: 0`, opened by the page)

Binary message, all integers big-endian:

| bytes | field |
|---|---|
| 1 | kind = 1 (audio) |
| 4 | `seq`, +1 per 20 ms packet, wraps |
| 1 | `hops` from the station (0 = sent by the station) |
| n | the Opus packet |

A relay raises `hops` by one when it forwards. `hops` is not signed; it only steers the
choice of parent (§5) and is clamped. Turn on Opus in-band FEC in `OpusEncoder` so a single
lost packet can be recovered from the next one.

### `sig` channel (reliable, ordered; the existing `now` channel can carry it)

Every 10 packets (200 ms), the station sends a block:

| bytes | field |
|---|---|
| 1 | kind = 2 (block) |
| 4 | `first_seq` |
| 1 | `count` (10) |
| 16 × count | the first 16 bytes of SHA-256(`seq` ‖ packet) for each packet |
| 64 | Ed25519 signature over `"craftradio-block\n"` ‖ station ID ‖ everything above |

Why a list of per-packet hashes rather than one hash over the block: the audio channel loses
packets. A single hash over the block could not be checked once one packet was missing.
With a list, each packet that arrives is checked on its own.

The station's Ed25519 key is the one it already has (`relay::Identity`). Signing 5 blocks per
second costs the station little. The page verifies 5 signatures per second with
`crypto.subtle` (Ed25519). Browsers that can't verify Ed25519 already fail on a listen link.

### Verify, then play

The page plays a packet only after its block has been verified. The first packet of a block
waits up to 200 ms for that block, so the jitter buffer never goes below 250 ms (§4). Inside
that buffer, checking adds no further delay. A packet with no verified hash when its time comes counts as lost (§4).
Playing first and checking later was rejected: it would let a bad relay put up to 200 ms of
its own audio on air for every listener below it.

## 4. Playback in the page

Leaving RTP means leaving the browser's own jitter buffer, loss concealment and clock-drift
correction. The page has to do all three itself. This is the real cost of the design.

- **Decoder:** `new AudioDecoder({ codec: "opus", sampleRate: 48000, numberOfChannels: 2 })`.
  Support is **to verify** per browser: Chrome and Edge have it. Firefox and Safari have
  WebCodecs audio in recent versions, but Opus in `AudioDecoder` has to be checked with
  `AudioDecoder.isConfigSupported`. When it is not supported, use RTP (§2 item 5).
- **Jitter buffer:** packets wait in order of `seq` up to a target depth (start 300 ms; move
  between 250 and 600 ms with the measured jitter; never below 250 ms, see §3).
- **Loss:** a missing packet is decoded from the next packet's FEC when it is there. Otherwise
  it becomes 20 ms of silence faded in and out over 2–3 ms. With two parents, most losses
  never reach this point.
- **Clock drift:** the station's clock and the listener's sound card differ by up to a few
  hundred ppm. When the buffer sits above its target for a while, skip one 20 ms packet.
  When it runs low, play one packet twice. Both crossfade over 5 ms. Resampling is not
  needed at radio quality.
- **AudioWorklet in one file:** the worklet's code is a string in the page, loaded through a
  Blob URL (`audioWorklet.addModule(URL.createObjectURL(new Blob([...])))`). Decoded
  `AudioData` goes to the worklet through a `MessagePort` (transferable buffers).
- **Output:** worklet → `MediaStreamAudioDestinationNode` → `audio.srcObject`. The
  `<audio>` element, MediaSession, background play and the level meter keep working as
  today, because they still get a `MediaStream`.

## 5. Relaying

### Finding sources

The tracker protocol already passes offers to any peer in a swarm, not only to the station.
A peer that announces in the swarm gets offers, and its answers go back through the tracker.

- When a listener has a verified stream, a healthy buffer and free slots, it announces in the
  station's swarm as the station does (`numwant: 0`, `offers: []`, `left: 0`). It answers the
  offers it gets, while it has free slots. Its answer carries `a=x-radio-relay:<hops>;<free>`.
- **New trust model:** a relay's answer cannot carry the station's signature, because its SDP
  holds the relay's own DTLS fingerprint. The page therefore accepts **unsigned answers from
  relays**. The audio is protected by the signed blocks (§3), not by the answer. A relay that
  sends no verified packets within 3 s is dropped and not tried again in this session.
  Answers from the station stay signed as today.
- **One connection per source:** today the page posts one SDP under five offer IDs, and one
  `RTCPeerConnection` can accept only one answer. For two parents the page makes one
  `RTCPeerConnection` per offer (5 at first). It keeps the two best answers and closes the
  rest. Best means a signed station answer when the page has no source yet, else the
  lowest `hops`, then the most `free` slots.

### Fan-out and depth

- Each listener feeds at most **3 children**. At 96 kbps that is about 0.3 Mbit/s of upload.
  Check the page's own upload: if packets queue in `bufferedAmount`, lower its free slots.
- A relay does not take children while its own `hops` is 6 or more. That bounds latency: each
  hop adds about 20–80 ms of network time.
- The station keeps `direct_reserve` of its `max_listeners` slots (proposed 10) for listeners
  that found no relay, or that run the RTP fallback. When the station is full except for that
  reserve, it answers new offers only from pages that report they found no relay. The offer
  carries `a=x-radio-no-relay` after the first round fails.

### Two parents, and repair

- Each listener keeps two parents, if it can. Packets from both feed the same buffer, and
  the second copy of a `seq` is dropped.
- When a parent goes away or stops sending verified packets, the page announces again and
  replaces that parent. The other parent keeps the audio going in the meantime.
- **Loops:** a relay never takes a parent with a `hops` value at or above its own. So a child
  can't become its own grandparent through the tree.
- **The `sig` stream** travels the same way. Each parent sends its blocks on its reliable
  channel, and the page drops blocks it already has.

### Rejected alternatives

- **Forward the received track (`pc.addTrack(remoteTrack)`).** It's only a few lines of
  code, but the browser decodes and re-encodes at every hop. Quality drops with each hop,
  it costs CPU, and the audio can't be signed. Rejected.
- **Segment swarming (P2P HLS, as in p2p-media-loader).** A proven way to do this, but it adds
  seconds of delay and needs a JS library, which breaks rule 4. Rejected.

## 6. Fewer third parties

These changes are independent of each other and of relaying. Each one is its own small change.

1. **UPnP address instead of STUN.** When `upnp.rs` maps the port, the station uses the
   router's external address as its public candidate and stops asking STUN. When the mapping
   fails, STUN is used as today. Listeners still use STUN in the page for their own address.
2. **The station as its own tracker.** The station already listens on TCP for WHEP. It could
   also speak the WebTorrent tracker protocol over a WebSocket on that port, for its own swarm
   only. Pages that can reach the station address use it first. Public trackers stay the
   fallback for the listen link (`#<public key>`), which carries no address.
3. **A second signalling route** (for example Nostr relays) so the listen link survives both
   public trackers failing. This is optional; decide after 1 and 2.

A browser cannot find peers with no meeting point at all: the DHT and mDNS are not reachable
from a page. So the goal is fewer single points of failure, not zero servers.

**Not fixed:** there is still no TURN. Two listeners behind symmetric NATs cannot connect to
each other. They need the station or a relay with an open NAT. Relays add paths to try, not a
guarantee.

## 7. Station, settings and what the operator sees

- `WebRtcConfig` gets `relay: bool` (pages may relay; default per §8), `direct_reserve: usize`
  and `max_hops: u8`. They go to the page in the `now` channel's first message, so the
  station can switch relaying off for all listeners.
- Rule 6: each setting is an engine command, a `radio --webrtc` CLI flag and an MCP tool.
  Rule 9: they live on Preferences → the WebRTC listening page.
- Station status shows **direct listeners** (counted by the station) and **reported listeners
  through relays**. The second number is what pages report on the `now` channel, every 30 s,
  summed up the tree. Rule 7: it is labelled "reported", because the station cannot check it.
  It must never look like the measured number.
- `ChannelOpen(id, label)`: the station matches channels by label (`now`, `audio`) instead of
  taking the first one.
- The station keeps sending RTP to the listeners that use the fallback. A page that opens
  `audio` gets no RTP track. It offers `recvonly` audio only on the fallback path.

## 8. Open decisions (operator)

1. **Page size (rule 4).** The page grows by an estimated **350–450 lines**: about 150 for
   decoding, the jitter buffer, loss and drift, about 150 for relaying and choosing parents,
   and about 100 for checking signatures, several connections and the fallback. It roughly
   doubles. It stays one file with no build step. Is that still "small"?
2. **Relaying is on by default or off.** A relaying listener's IP address is seen by its
   children and parents. Today only the station and the trackers see it. Proposal: on by
   default, with a visible "Sharing the stream with N listeners" line and a switch on the
   page (stored in `localStorage`). The station can also turn it off for everyone (§7).
3. **`direct_reserve`.** Proposal: 10 of the default 50.
4. **Mobile data.** Browsers barely say whether the connection is metered
   (`navigator.connection` exists only in Chromium). Proposal: on phones, pages default to 1
   child, not 3.

## 9. Phases

Each phase ships on its own and leaves the station working.

**P1: Opus over a signed data channel, station only.** Station: the `audio` and `sig`
channels, block signing, FEC, channels matched by label, RTP kept for the fallback. Page:
`AudioDecoder`, the worklet, the jitter buffer, loss and drift, verify-then-play, the
fallback. No relaying yet.
- Tests (`crates/stream/tests/it/webrtc.rs`): a str0m client in the test opens `audio` and
  `sig`. Assert that
  - every packet it gets matches a hash in a block whose signature verifies with the station
    key;
  - `seq` increases with no gaps on a loopback link;
  - a changed packet fails to verify;
  - a client that opens no `audio` channel still gets RTP.
- Manual check: Chrome, Firefox and Safari each play for 10 minutes with no audible gap.
  Throttle the network in DevTools: the buffer grows and comes back down, and there is no
  drift over 10 minutes.

**P2: listeners relay, one parent each.** Page: announce in the swarm, answer offers, forward
`audio` and `sig`, accept unsigned relay answers, one connection per offer, the `hops` and
`free` lines. Station: `relay`, `max_hops`, the reported count, and the commands, flags and
Preferences page from §7. Spec edit for §2 item 8.
- Tests (Rust): the station sends `relay: false` and the new settings to the page; the
  reported count is kept apart from the direct count in status.
- The relay logic is in the page, which has no test harness (rule 4: no build step). The
  check is manual and written in this plan: one station with `max_listeners = 1`, and three
  browser profiles open the listen link. The second and third play through the first; the
  station shows 1 direct and 2 reported. Close the first: the others reconnect within 5 s.
  Make one relay send changed bytes (a DevTools override): its children drop it and keep
  playing.

**P3: two parents, repair, reserve.** Two parents with deduplication, repair on loss, the
`hops` loop rule, `direct_reserve` and `a=x-radio-no-relay`.
- Tests (Rust): with the reserve in use, offers without `no-relay` get no answer and offers
  with it do.
- Manual check: six profiles. Close relays one at a time. No listener has a gap longer than
  the buffer.

**P4: fewer third parties.** §6 items 1 and 2, each as its own change. Item 3 after that.
- Tests (Rust): with a mapped port, the station's candidate is the external address and
  nothing is sent to STUN; the tracker endpoint answers an announce for the station's swarm
  and ignores other swarms.

Proposed roadmap row: **M5.5 P2P listening**, relaying of WebRTC listeners (this plan), with
P1–P4 as above. Ends with: three browsers on one station slot all play, and the audio is
verified at every hop.
