# P2P listener relay: design

**Goal:** Listeners pass the station's audio on to other listeners. Then the number of
listeners no longer depends on the station's upload. A relaying listener cannot change the
audio without being caught. Second goal: the station needs fewer third-party servers.

**Status (2026-11-22):** P1 and P2 are built (§9). P3 and P4 are not. P2 went ahead with
the proposals in §8 for the decisions still open.

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
choice of parent (§5) and is clamped.

No Opus in-band FEC: it works only in the SILK and hybrid modes, and Program at 96 kbps stereo
with `Application::Audio` is coded in CELT. Two parents (P3) cover loss instead.

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
waits up to 200 ms for that block, so playback starts with 400 ms (20 packets) of checked
audio (§4). Inside that buffer, checking adds no further delay.

A page that tunes in by address (`http://…`, not a link) has no station key. It trusts the
DTLS link to the station and plays packets without blocks. Relaying (P2) needs the key, so
it works only from a link. A packet with no verified hash when its time comes counts as lost (§4).
Playing first and checking later was rejected: it would let a bad relay put up to 200 ms of
its own audio on air for every listener below it.

## 4. Playback in the page

Leaving RTP means leaving the browser's own jitter buffer, loss concealment and clock-drift
correction. The page has to do all three itself. This is the real cost of the design.

- **Decoder:** `new AudioDecoder({ codec: "opus", sampleRate: 48000, numberOfChannels: 2 })`.
  Support is **to verify** per browser: Chrome and Edge have it. Firefox and Safari have
  WebCodecs audio in recent versions, but Opus in `AudioDecoder` has to be checked with
  `AudioDecoder.isConfigSupported`. When it is not supported, use RTP (§2 item 5).
- **Jitter buffer:** checked packets are decoded in order of `seq` as soon as they are
  ready. The worklet's queue of decoded audio is the buffer. Playback starts once 20 checked
  packets (400 ms) wait. On loopback the queue held 15–28 packets (300–560 ms).
- **Loss:** when fewer than 3 packets are queued and a later packet is ready, the missing one
  becomes 20 ms of silence. The worklet fades out and back in over 2.5 ms (120 samples). With
  two parents, most losses never reach this point.
- **Clock drift:** the station's clock and the listener's sound card differ by up to a few
  hundred ppm. When the worklet holds more than 40 packets (800 ms), it fades out and drops
  one. When the queue runs dry, the page waits for 20 packets again (one rebuffer, about once
  an hour at 100 ppm), rather than repeating packets. Resampling is not needed at radio
  quality.
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

- `WebRtcConfig` gets `relay: bool` (pages may relay; on by default). The station sends it
  as the first message on the `now` channel, `{"type":"relay","allowed":…,"max_hops":6}`,
  and relays pass that hello on, so the station can switch relaying off for all listeners.
  `max_hops` is the constant `MAX_HOPS` (6), not a setting. P3 adds `direct_reserve`.
- Rule 6: `relay` is the `webrtc.relay` setting, which the generic `settings.set` command (so
  the CLI and MCP) already sets. `craftradio --no-relay` turns it off for one run. Rule 9: the
  switch is on Preferences › WebRTC, under "Passing it on", with a note that listeners who
  pass it on see each other's IP addresses. Like the other WebRTC settings, it is taken up at
  the next start of the app.
- Station status shows **direct listeners** (counted by the station) and **reported listeners
  through relays** (`WebRtcStatus::Listening::reported`). The second number is what pages
  report on the `now` channel, `{"type":"report","listeners":n}`, on each change and every
  30 s, summed up the tree. The station caps each page's report at 1000. Rule 7: the status
  line and the output list say "3 listeners, 5 more reported by listeners", and the headless
  app prints them as "not checked". They never add up to one number.
- `ChannelOpen(id, label)`: the station matches channels by label (`now`, `audio`) instead of
  taking the first one.
- The page always offers `recvonly` RTP audio, so an older station, which sends only RTP,
  still plays. The station stops RTP to a listener once its `audio` channel opens. When no
  packet is decoded within 4 s of connecting (an older station, or a browser that can't decode
it), or the decoder fails more than 10 times, the page closes `audio` and plays the RTP track
instead. The page
  opens `audio` before `now`, because an older station takes the last channel as `now`.
  `?rtp` in the page address forces the RTP track, to compare the two.

## 8. Open decisions (operator)

Decided: packet playback is the default from P1 on, before relaying ships, despite its
0.4–0.6 s more delay than RTP (operator, 2026-11-22).

1. **Page size (rule 4).** P1 took the page from 590 to 809 lines (decoding, buffer, loss,
   drift, checking blocks, the fallback), and P2 to 1,068 (relaying, one connection per
   offer, choosing a relay), and P3 to 1,178 (two sources, repair, the Settings card), not
   the 80–120 more estimated. It stays one file with no
   build step. Is that still "small"?
2. **Relaying is on by default or off.** A relaying listener's IP address is seen by its
   children and parents. Before P2 only the station and the trackers saw it. Built as
   proposed: on by default, with a switch on the page (stored in `localStorage`). Since P3
   the switch is "Help others listen" in a collapsed Settings card, with the note that
   listeners who share see each other's IP addresses, not a line under the player (operator,
   2026-11-23). The station can also turn it off for everyone (§7).
3. **`direct_reserve`.** Decided: 10 of the default 50 (operator, 2026-11-23).
4. **Mobile data.** Browsers barely say whether the connection is metered
   (`navigator.connection` exists only in Chromium). Proposal: on phones, pages default to 1
   child, not 3.

## 9. Phases

Each phase ships on its own and leaves the station working.

**P1: Opus over a signed data channel, station only.** Station: the `audio` and `sig`
channels, block signing, channels matched by label, RTP kept for the fallback. What plays
now holds at most 64 KiB of the connection's shared 128 KiB send buffer, so covers leave room
for the audio. Page:
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
- Done (2026-11-22), in the app's Chromium browser pane against a local tone station:
  - by address: plays from packets;
  - by link through the public trackers: plays checked packets, the queue held 15–28 packets
    over 20 s and never ran dry;
  - a byte flipped in every 50th packet: all 10 rejected, no underrun;
  - `?rtp`: plays the RTP track;
  - an older station (channel renamed in the page): falls back to RTP;
  - a decoder made to fail against a new station: the page closes `audio`, and the station
    sends RTP again;
  - `cargo test` of the audio channel test, 20 runs in a row: no flakes.
  Not done: Firefox, Safari, 10-minute runs, throttled networks.

**P2: listeners relay, one parent each.** Page: announce in the swarm, answer offers, forward
`audio` and `sig`, accept unsigned relay answers, one connection per offer, the `hops` and
`free` lines. Station: `relay`, `max_hops`, the reported count, and the commands, flags and
Preferences page from §7. Spec edit for §2 item 8.
- Built (2026-11-22). What changed on the way:
  - **One connection per offer had to come in P2, not P3.** With one SDP under five offer
    IDs, every relay that answered started its ICE checks against the same offer. The page
    took one answer, but another relay's checks got through, and DTLS never finished. Now each
    offer is its own `RTCPeerConnection`; the page keeps the chosen one and closes the rest.
    An offer goes out again only while no one has answered it.
  - **Station first (operator, 2026-11-23), until P3 gives each listener two sources.** The
    station's answer wins at once. Only when relays alone answer does the page wait 1.5 s for
    the closest one: a relay answers only once it has gathered its candidates. So relays
    carry listeners only once the station is full. This replaces "the closest relay wins" in
    §5 until P3.
  - Any answer uses up its offer, even one the page throws away (a relay for a page that
    plays from the station only, or a bad signature), because the peer that sent it already
    checks against it. When all five are used and none was usable, the page starts again
    with new connections.
  - A page that takes the station only (the switch off, or no packet path) marks its offers
    `a=x-radio-station-only`, and relays leave them alone. With the switch off, no relay sees
    the page, though trackers still pass its offers to the station's swarm.
  - A relay answers each listener once, though the listener sends five offers.
  - A page drops its relay, and doesn't take it again, when any of these happens:
    - it can't reach it in 5 s (both behind NAT);
    - the connection fails or is lost;
    - nothing decodes within 4 s;
    - no packet arrives for 3 s;
    - more than 25 packets fail the check.

    A relay sends no RTP, so the RTP fallback is only for the station.
  - A relay's tracker socket that closes is dropped, so the relay joins the swarm again.
  - Titles, covers, history and the hello pass through relays **unsigned**: a relay can change
    what its listeners see as playing, though not what they hear. Signing them is left open.
  - The relay keeps the hello, the history, its thumbnails and the messages since the last
    `now`, to greet a new listener with them.
- Tests (Rust, `crates/stream/tests/it/webrtc.rs`):
  - the hello comes first and says whether relaying is allowed;
  - reported listeners stay apart from direct ones in the status, capped at 1000;
  - a station that forbids relaying counts no reported listeners.

  - the station answers an offer with the station-only line.

  Test in `crates/ui-egui/tests/it/listen_link.rs`: the status text names reported listeners
  apart from the direct count.
- The relay logic is in the page, which has no test harness (rule 4: no build step). The
  check is manual and written in this plan: one station with `max_listeners = 1`, and three
  browser profiles open the listen link. The second and third play through the first; the
  station shows 1 direct and 2 reported. Close the first: the others reconnect within 5 s.
  Make one relay send changed bytes (a DevTools override): its children drop it and keep
  playing.
- Done (2026-11-22) with three tabs of the app's Chromium browser pane, a station with one
  slot, and the public trackers:
  - tabs 2 and 3 played through tab 1 (2 hops, no packet rejected), and the station showed 1
    direct and 2 reported;
  - tab 1 stopped: tab 3 took the station's slot, and tab 2 played through tab 3. That took
    about 13 s (a 4 s check, a 2 s retry, then the search), not 5 s. P3's second parent is
    what closes that gap;
  - tab 1 flipped a byte in every packet it passed on: tab 2 dropped it within 1 s. With the
    station's one slot taken, it then found no source; P3's reserve is for that.

- Done again (2026-11-23) after the fixes, with a station with two slots:
  - station first: tabs 1 and 2 took the station's slots, and tab 3 played through a relay;
  - both relays made unreachable (`iceTransportPolicy: "relay"` with no TURN): tab 3 dropped
    each after 5 s. Once a station slot was free, it played from the station;
  - a `?rtp` page next to a relay: the relay saw 10 offers and answered none.

  Not done: separate browser profiles or machines (every tab here shares one browser and one
  network, so no real NAT stood between them), Firefox, Safari.

**P3: two parents, repair, reserve.** Two parents with deduplication, repair on loss, the
`hops` loop rule, `direct_reserve` and `a=x-radio-no-relay`.
- Tests (Rust): with the reserve in use, offers without `no-relay` get no answer and offers
  with it do.
- Manual check: six profiles. Close relays one at a time. No listener has a gap longer than
  the buffer.
- Built (2026-11-23). Decisions and what changed on the way:
  - **A listener on the station needs no second source (operator, 2026-11-23).** The
    station's answer still wins at once. A page that plays through a relay looks for a
    second one; when the station answers that search, the page plays from the station alone
    and lets its relays go, once the station's packets arrive.
  - **Reserve.** `direct_reserve` (10) is a WebRTC setting, on Preferences › WebRTC ›
    Passing it on. It applies only to offers through trackers: offers by address never wait
    for it. Offers with `a=x-radio-no-relay` or `a=x-radio-station-only` may take it (a
    page with the switch off, `?rtp`, or no decoder cannot play through a relay). Without
    relaying there is no reserve, and at least one slot always stays for everyone. The
    station checks this before the offer costs any of its answer budget.
  - **`no-relay` round.** A first search adds `a=x-radio-no-relay` to the offers it posts
    again after 4 s with no relay answer, not after a whole failed search (20 s).
  - **One player, many sources.** Packets from both sources go into one buffer. The first
    copy of each `seq` is kept and passed on; a second copy that differs is kept until the
    first passes the check, and a failure counts against the source that sent it. Blocks
    are passed on once, after their signature checks out.
  - **Depth** is 1 + the highest hops of the page's sources. A second source must be closer
    to the station than the page (hops below its depth), so no page plays through its own
    listeners. A page ignores its own offers and answers.
  - **Repairs cost relays little.** A page keeps one tracker peer ID for a whole tune-in, so
    a relay that already feeds it doesn't answer its repair offers. A repair offer carries
    `a=x-radio-depth:<n>`, and relays at or past that depth stay silent. A search that finds
    no one waits 15 s before the next, then twice as long each time, up to 4 minutes.
  - **Counted once.** A page reports its listeners to its first source and tells the second
    `{"type":"report","listeners":0,"counted":false}`, so the second leaves it out of its
    total. Titles and the hello come from the first source only.
  - A source is dropped, and a relay not tried again, when it can't be reached (5 s for a
    relay, 10 s for the station), is lost for 5 s, sends nothing for 4 s, stops for 3 s, or
    sends more than 5 packets or blocks that fail the check. The page starts over only when
    its last source goes.
  - **What the listener sees (operator, 2026-11-23).** The player says "Live", never
    which or how many listeners it plays through; that goes to the browser console only.
    Retry messages don't name relays. The switch moved from under the player to a
    collapsed "Settings" card ("Help others listen"), which shows for every station link,
    before play too, with the note about IP addresses. On a page that can't share (`?rtp`,
    no decoder, the RTP fallback) the switch is disabled, with the reason.
- Done (2026-11-23), in the app's Chromium browser pane, a station with two slots:
  - tabs 1 and 2 on the station, tab 3 through both (2 sources, hops 1, depth 2), and the
    station reported 1, not 2;
  - tab 2 closed: tab 3 played on from tab 1 with no pause past its usual decoding pace
    (about 220 ms in a background tab, inside the 400 ms buffer). Its next search found the
    freed station slot, and it moved to the station with no pause either. A first version
    that let the relays go before the station's packets came paused for 420 ms; fixed.
  - `no-relay` round, a station with two slots and one kept back: a `?rtp` tab took the open
    slot, and the next tab was refused, then played from the kept slot after 5.3 s;
  - three slots, four tabs: tab 4 through tabs 1 and 3. Tab 3 flipped a byte in every packet
    it passed on: tab 4 dropped it within 1 s (6 failed packets) with no pause, and its next
    search took tab 2, the free relay, within 4 s;
  - the two reserve tests, 20 runs in a row: no flakes.

  Not done: six profiles, separate machines or NATs (every tab here shares one browser and
  one network), Firefox, Safari, and relays under many listeners at once.

**P4: fewer third parties.** §6 items 1 and 2, each as its own change. Item 3 after that.
- Tests (Rust): with a mapped port, the station's candidate is the external address and
  nothing is sent to STUN; the tracker endpoint answers an announce for the station's swarm
  and ignores other swarms.
- Item 1 built (2026-11-23). While the router maps the port (UPnP or NAT-PMP), the station
  offers the router's address as its one outside candidate and sends no STUN requests; when
  the mapping goes, STUN refreshes start again at once. The station still asks STUN once at
  start, before the router has answered (a mapping takes up to about 11 s to fail), so a
  station without a mapping is reachable as soon as before. Tests (unit, `webrtc.rs`): the
  router address wins over the STUN address; no request reaches a STUN server while mapped,
  and one does once the mapping is gone. Not checked against a real router here.
- **Item 2 is blocked as written (2026-11-23).** A tracker on the station's TCP port can't
  be reached by the pages that need it:
  - pages from the listen link run on `https://radioterminal.github.io`, and browsers block
    `ws://` from an `https://` page as mixed content (except `localhost`); `wss://` needs a
    certificate a browser trusts for the station's address, which a home station doesn't
    have;
  - a page that the station serves itself, over `http://` on the local network, is not a
    secure context, so it can't check the station's signatures (`crypto.subtle`), and such
    pages play by address through WHEP anyway, with no trackers;
  - UPnP maps only the UDP media port, so the TCP port isn't reachable from outside.

  Ways on that were weighed: (a) signalling through the mesh: a page that already plays
  sends its repair offers through its sources' `now` channels, so repairs need no tracker;
  (b) item 3, a second route over `wss://` (Nostr relays); (c) drop item 2.
- **Decided (operator, 2026-11-23): item 2 is dropped, and signalling through the mesh is
  not pursued: too complicated to support.** Listeners keep finding the station and each
  other through the public trackers.

Proposed roadmap row: **M5.5 P2P listening**, relaying of WebRTC listeners (this plan), with
P1–P4 as above. Ends with: three browsers on one station slot all play, and the audio is
verified at every hop.
