use radio_stream::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use str0m::channel::{ChannelConfig, Reliability};
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, RtcConfig};

fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: test\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let code = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (code, head.to_owned(), body.to_owned())
}

fn listeners(h: &WebRtcHandle) -> Option<usize> {
    match h.status() {
        WebRtcStatus::Listening { listeners, .. } => Some(listeners),
        _ => None,
    }
}

fn wait(h: &WebRtcHandle, f: impl Fn(Option<usize>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f(listeners(h)) {
        assert!(Instant::now() < deadline, "status stayed {:?}", h.status());
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_whep_listener_receives_program_audio_and_leaves() {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    let (mut producer, consumer) = rtrb::RingBuffer::new(48_000 * 2);
    let server = WebRtcServer::start(cfg, consumer);
    let addr = server.local_addr().unwrap();
    wait(&server, |l| l == Some(0));

    let running = Arc::new(AtomicBool::new(true));
    let feeding = running.clone();
    let feed = std::thread::spawn(move || {
        let mut n = 0u64;
        while feeding.load(Ordering::Relaxed) {
            for _ in 0..960 {
                let s = ((n as f32 * 440.0 * std::f32::consts::TAU) / 48_000.0).sin() * 0.2;
                let _ = producer.push(s);
                let _ = producer.push(s);
                n += 1;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    let (code, _, page) = http(addr, "GET", "/", "");
    assert_eq!(code, 200);
    assert!(page.contains("RTCPeerConnection"));

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let local = socket.local_addr().unwrap();
    let mut rtc = RtcConfig::new()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        .build(Instant::now());
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
    let mut api = rtc.sdp_api();
    api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
    let (offer, pending) = api.apply().unwrap();

    let (code, head, answer) = http(addr, "POST", "/whep", &offer.to_sdp_string());
    assert_eq!(code, 201, "{answer}");
    let location = head
        .lines()
        .find_map(|l| l.strip_prefix("Location: "))
        .unwrap()
        .to_owned();
    let answer = str0m::change::SdpAnswer::from_sdp_string(&answer).unwrap();
    rtc.sdp_api().accept_answer(pending, answer).unwrap();

    let mut buf = vec![0u8; 2000];
    let mut audio = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    while audio < 10 {
        assert!(Instant::now() < deadline, "no audio arrived");
        let timeout = loop {
            match rtc.poll_output().unwrap() {
                Output::Timeout(t) => break t,
                Output::Transmit(t) => {
                    socket.send_to(&t.contents, t.destination).unwrap();
                }
                Output::Event(Event::MediaData(d)) => {
                    assert!(!d.data.is_empty());
                    audio += 1;
                }
                Output::Event(_) => {}
            }
        };
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .clamp(Duration::from_millis(1), Duration::from_millis(20));
        socket.set_read_timeout(Some(wait)).unwrap();
        let input = match socket.recv_from(&mut buf) {
            Ok((n, source)) => Input::Receive(
                Instant::now(),
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination: local,
                    contents: buf[..n].try_into().unwrap(),
                },
            ),
            Err(_) => Input::Timeout(Instant::now()),
        };
        rtc.handle_input(input).unwrap();
    }
    assert_eq!(listeners(&server), Some(1));

    // Another page cannot guess the resource and drop this listener.
    for guess in ["/whep/1", "/whep/0", &format!("/whep/{}", "0".repeat(32))] {
        http(addr, "DELETE", guess, "");
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        listeners(&server),
        Some(1),
        "a guessed DELETE dropped the listener"
    );

    let (code, _, _) = http(addr, "DELETE", &location, "");
    assert_eq!(code, 200);
    wait(&server, |l| l == Some(0));

    server.stop();
    assert_eq!(server.status(), WebRtcStatus::Stopped);
    running.store(false, Ordering::Relaxed);
    feed.join().unwrap();
}

#[test]
fn a_station_that_cannot_bind_reports_failed() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = WebRtcConfig::new(taken.local_addr().unwrap());
    cfg.stun.clear();
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    assert!(matches!(server.status(), WebRtcStatus::Failed(_)));
}

#[test]
fn whep_refuses_a_listener_past_the_maximum() {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    cfg.max_listeners = 0;
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let (code, _, _) = http(server.local_addr().unwrap(), "POST", "/whep", "v=0\r\n");
    assert_eq!(code, 503);
}

fn xor_mapped(id: [u8; 12], addr: SocketAddr) -> Vec<u8> {
    let SocketAddr::V4(v4) = addr else {
        unreachable!()
    };
    let magic = 0x2112_A442u32.to_be_bytes();
    let mut attr = vec![0x00, 0x20, 0x00, 0x08, 0x00, 0x01];
    attr.extend((v4.port() ^ 0x2112).to_be_bytes());
    attr.extend(v4.ip().octets().iter().zip(magic).map(|(b, m)| b ^ m));
    let mut msg = vec![0x01, 0x01];
    msg.extend((attr.len() as u16).to_be_bytes());
    msg.extend(magic);
    msg.extend(id);
    msg.extend(attr);
    msg
}

#[test]
fn stun_response_gives_the_mapped_address_for_its_own_request_only() {
    let id = [7u8; 12];
    let public: SocketAddr = "203.0.113.9:54321".parse().unwrap();
    let request = binding_request(id);
    assert_eq!(&request[8..], &id);
    let response = xor_mapped(id, public);
    assert_eq!(parse_binding_response(&response, id), Some(public));
    assert_eq!(parse_binding_response(&response, [8u8; 12]), None);
    assert_eq!(parse_binding_response(&response[..24], id), None);
}

fn offer() -> String {
    let mut rtc = RtcConfig::new()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        .build(Instant::now());
    let mut api = rtc.sdp_api();
    api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
    api.apply().unwrap().0.to_sdp_string()
}

#[test]
fn listeners_that_still_connect_count_toward_the_maximum() {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    cfg.max_listeners = 1;
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let addr = server.local_addr().unwrap();
    let (first, _, _) = http(addr, "POST", "/whep", &offer());
    let (second, _, _) = http(addr, "POST", "/whep", &offer());
    assert_eq!(first, 201);
    assert_eq!(
        second, 503,
        "a second offer passed while the first still connects"
    );
}

#[test]
fn a_client_that_sends_nothing_does_not_hold_up_others() {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let addr = server.local_addr().unwrap();
    let _silent: Vec<TcpStream> = (0..3).map(|_| TcpStream::connect(addr).unwrap()).collect();
    let start = Instant::now();
    let (code, _, _) = http(addr, "GET", "/now", "");
    assert_eq!(code, 200);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn one_address_cannot_fill_the_station_with_offers_that_never_connect() {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let addr = server.local_addr().unwrap();
    let codes: Vec<u16> = (0..4)
        .map(|_| http(addr, "POST", "/whep", &offer()).0)
        .collect();
    assert_eq!(codes.iter().filter(|c| **c == 201).count(), 2, "{codes:?}");
    assert!(codes[2..].iter().all(|c| *c == 503), "{codes:?}");
}

#[test]
fn an_offer_cannot_point_the_station_at_other_hosts() {
    let sdp = "v=0\r\n\
        a=candidate:1 1 udp 1 192.168.1.20 5000 typ host\r\n\
        a=candidate:2 1 udp 1 127.0.0.1 5000 typ host\r\n\
        a=candidate:3 1 udp 1 10.0.0.1 5000 typ host\r\n\
        a=candidate:4 1 udp 1 81.2.69.160 5000 typ srflx raddr 0.0.0.0 rport 0\r\n\
        a=candidate:5 1 udp 1 abcd.local 5000 typ host\r\n\
        a=candidate:6 1 udp 1 ::ffff:10.0.0.1 5000 typ host\r\n\
        a=candidate:7 1 udp 1 ::10.0.0.1 5000 typ host\r\n\
        a=candidate:8 1 udp 1 2002:a00:1:: 5000 typ host\r\n\
        a=candidate:9 1 udp 1 fec0::1 5000 typ host\r\n\
        a=candidate:10 1 udp 1 0.1.2.3 5000 typ host\r\n\
        a=mid:0\r\n";
    // A page on the internet, or a page that a person on the local network opened.
    for peer in ["81.2.69.1", "192.168.1.9"] {
        let kept = filter_candidates(sdp, peer.parse().unwrap());
        assert!(kept.contains("81.2.69.160"), "{peer}");
        assert!(kept.contains("a=mid:0"));
        assert_eq!(kept.matches("a=candidate:").count(), 1, "{peer}: {kept}");
    }
    // An offer from a tracker has no client address. Its `::` candidate must not pass.
    let relayed = filter_candidates(
        "a=candidate:1 1 udp 1 :: 5000 typ host\r\na=candidate:2 1 udp 1 0.0.0.0 5000 typ host\r\n",
        "::".parse().unwrap(),
    );
    assert_eq!(relayed.matches("a=candidate:").count(), 0, "{relayed}");
    // The client's own address stays, so a listener on this computer still connects.
    let own = filter_candidates(sdp, "127.0.0.1".parse().unwrap());
    assert!(own.contains(" 127.0.0.1 "));
    assert!(!own.contains("192.168.1.20"));
    let many: String = (0..20)
        .map(|i| format!("a=candidate:{i} 1 udp 1 81.2.69.{i} 5000 typ host\r\n"))
        .collect();
    let kept = filter_candidates(&many, "81.2.69.200".parse().unwrap());
    assert!(kept.matches("a=candidate:").count() < 20);
}

#[test]
fn parallel_offers_from_one_address_cannot_pass_its_limit() {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let addr = server.local_addr().unwrap();
    let offers: Vec<String> = (0..8).map(|_| offer()).collect();
    let threads: Vec<_> = offers
        .into_iter()
        .map(|o| std::thread::spawn(move || http(addr, "POST", "/whep", &o).0))
        .collect();
    let codes: Vec<u16> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(codes.iter().filter(|c| **c == 201).count(), 2, "{codes:?}");
}

/// Feeds a tone into Program until the returned flag goes false.
fn feed_tone(mut producer: rtrb::Producer<f32>) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let running = Arc::new(AtomicBool::new(true));
    let feeding = running.clone();
    let feed = std::thread::spawn(move || {
        let mut n = 0u64;
        while feeding.load(Ordering::Relaxed) {
            for _ in 0..960 {
                let s = ((n as f32 * 440.0 * std::f32::consts::TAU) / 48_000.0).sin() * 0.2;
                let _ = producer.push(s);
                let _ = producer.push(s);
                n += 1;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    (running, feed)
}

#[test]
fn a_page_on_the_audio_channel_gets_packets_that_match_signed_blocks_and_rtp_stops() {
    let pkcs8 = Identity::generate().unwrap();
    let public_key = ring::signature::Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map(|k| ring::signature::KeyPair::public_key(&k).as_ref().to_vec())
        .unwrap();
    let station_id = Identity::new(&pkcs8).unwrap().id();
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    cfg.trackers.clear();
    cfg.identity = Some(pkcs8);
    let (producer, consumer) = rtrb::RingBuffer::new(48_000 * 2);
    let server = WebRtcServer::start(cfg, consumer);
    let addr = server.local_addr().unwrap();
    wait(&server, |l| l == Some(0));
    let (running, feed) = feed_tone(producer);

    // The page's offer: RTP audio for older stations, and the two data channels.
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let local = socket.local_addr().unwrap();
    let mut rtc = RtcConfig::new()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        .build(Instant::now());
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
    let mut api = rtc.sdp_api();
    api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
    let now_id = api.add_channel("now".into());
    let audio_id = api.add_channel_with_config(ChannelConfig {
        label: AUDIO_CHANNEL.into(),
        ordered: false,
        reliability: Reliability::MaxRetransmits { retransmits: 0 },
        ..Default::default()
    });
    let (offer, pending) = api.apply().unwrap();
    let (code, _, answer) = http(addr, "POST", "/whep", &offer.to_sdp_string());
    assert_eq!(code, 201, "{answer}");
    let answer = str0m::change::SdpAnswer::from_sdp_string(&answer).unwrap();
    rtc.sdp_api().accept_answer(pending, answer).unwrap();

    let mut packets: std::collections::BTreeMap<u32, Vec<u8>> = Default::default();
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut rtp = 0usize;
    let mut buf = vec![0u8; 2000];
    let deadline = Instant::now() + Duration::from_secs(10);
    while blocks.len() < 3 {
        assert!(
            Instant::now() < deadline,
            "got {} packets and {} blocks",
            packets.len(),
            blocks.len()
        );
        let timeout = loop {
            match rtc.poll_output().unwrap() {
                Output::Timeout(t) => break t,
                Output::Transmit(t) => {
                    socket.send_to(&t.contents, t.destination).unwrap();
                }
                // RTP may flow in the moment before the channel opens, not after.
                Output::Event(Event::MediaData(_)) if packets.len() >= 10 => rtp += 1,
                Output::Event(Event::ChannelData(d)) if d.id == audio_id => {
                    assert_eq!(d.data[0], KIND_AUDIO);
                    assert_eq!(d.data[5], 0, "the station sends hops 0");
                    let seq = u32::from_be_bytes(d.data[1..5].try_into().unwrap());
                    packets.insert(seq, d.data[6..].to_vec());
                }
                Output::Event(Event::ChannelData(d)) if d.id == now_id && d.binary => {
                    blocks.push(d.data.clone());
                }
                Output::Event(_) => {}
            }
        };
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .clamp(Duration::from_millis(1), Duration::from_millis(20));
        socket.set_read_timeout(Some(wait)).unwrap();
        let input = match socket.recv_from(&mut buf) {
            Ok((n, source)) => Input::Receive(
                Instant::now(),
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination: local,
                    contents: buf[..n].try_into().unwrap(),
                },
            ),
            Err(_) => Input::Timeout(Instant::now()),
        };
        rtc.handle_input(input).unwrap();
    }
    running.store(false, Ordering::Relaxed);
    feed.join().unwrap();
    assert_eq!(
        rtp, 0,
        "once the audio channel is open, the station sends no RTP"
    );

    let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &public_key);
    let mut checked = 0;
    for block in &blocks {
        assert_eq!(block[0], KIND_BLOCK);
        let first = u32::from_be_bytes(block[1..5].try_into().unwrap());
        let count = block[5] as usize;
        assert_eq!(count, BLOCK_PACKETS);
        let signed_len = 6 + count * HASH_LEN;
        assert_eq!(block.len(), signed_len + SIGNATURE_LEN);
        let mut message = BLOCK_CONTEXT.to_vec();
        message.extend_from_slice(station_id.as_bytes());
        message.extend_from_slice(&block[..signed_len]);
        key.verify(&message, &block[signed_len..])
            .expect("the block carries the station's signature");
        let mut forged = message.clone();
        *forged.last_mut().unwrap() ^= 1;
        assert!(key.verify(&forged, &block[signed_len..]).is_err());

        for i in 0..count {
            let seq = first.wrapping_add(i as u32);
            let hash = &block[6 + i * HASH_LEN..6 + (i + 1) * HASH_LEN];
            if let Some(opus) = packets.get(&seq) {
                assert_eq!(
                    packet_hash(seq, opus),
                    hash,
                    "packet {seq} matches its block"
                );
                let mut changed = opus.clone();
                changed[0] ^= 1;
                assert_ne!(packet_hash(seq, &changed), hash, "a changed packet fails");
                checked += 1;
            }
        }
    }
    assert!(
        checked >= BLOCK_PACKETS,
        "only {checked} packets were in blocks"
    );
    let seqs: Vec<u32> = packets.keys().copied().collect();
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "sequence numbers have gaps on loopback: {seqs:?}"
    );
}

/// A page's connection to the station: RTP audio, then the "audio" and "now" channels.
struct Page {
    rtc: str0m::Rtc,
    socket: UdpSocket,
    now: str0m::channel::ChannelId,
}

impl Page {
    fn connect(addr: SocketAddr) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let local = socket.local_addr().unwrap();
        let mut rtc = RtcConfig::new()
            .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
        let mut api = rtc.sdp_api();
        api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
        api.add_channel_with_config(ChannelConfig {
            label: AUDIO_CHANNEL.into(),
            ordered: false,
            reliability: Reliability::MaxRetransmits { retransmits: 0 },
            ..Default::default()
        });
        let now = api.add_channel("now".into());
        let (offer, pending) = api.apply().unwrap();
        let (code, _, answer) = http(addr, "POST", "/whep", &offer.to_sdp_string());
        assert_eq!(code, 201, "{answer}");
        let answer = str0m::change::SdpAnswer::from_sdp_string(&answer).unwrap();
        rtc.sdp_api().accept_answer(pending, answer).unwrap();
        Self { rtc, socket, now }
    }

    /// Runs the connection until `done` returns true for the text messages on "now" so far,
    /// and returns them.
    fn until(
        &mut self,
        what: &str,
        mut done: impl FnMut(&mut str0m::Rtc, &[String]) -> bool,
    ) -> Vec<String> {
        let local = self.socket.local_addr().unwrap();
        let mut texts = Vec::new();
        let mut buf = vec![0u8; 2000];
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&mut self.rtc, &texts) {
            assert!(Instant::now() < deadline, "{what}: got {texts:?}");
            let timeout = loop {
                match self.rtc.poll_output().unwrap() {
                    Output::Timeout(t) => break t,
                    Output::Transmit(t) => {
                        self.socket.send_to(&t.contents, t.destination).unwrap();
                    }
                    Output::Event(Event::ChannelData(d)) if d.id == self.now && !d.binary => {
                        texts.push(String::from_utf8(d.data.clone()).unwrap());
                    }
                    Output::Event(_) => {}
                }
            };
            let wait = timeout
                .saturating_duration_since(Instant::now())
                .clamp(Duration::from_millis(1), Duration::from_millis(20));
            self.socket.set_read_timeout(Some(wait)).unwrap();
            let input = match self.socket.recv_from(&mut buf) {
                Ok((n, source)) => Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: local,
                        contents: buf[..n].try_into().unwrap(),
                    },
                ),
                Err(_) => Input::Timeout(Instant::now()),
            };
            self.rtc.handle_input(input).unwrap();
        }
        texts
    }
}

fn reported(h: &WebRtcHandle) -> Option<usize> {
    match h.status() {
        WebRtcStatus::Listening { reported, .. } => Some(reported),
        _ => None,
    }
}

fn relay_station(relay: bool) -> (WebRtcHandle, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    cfg.trackers.clear();
    cfg.relay = relay;
    let (producer, consumer) = rtrb::RingBuffer::new(48_000 * 2);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let (running, feed) = feed_tone(producer);
    (server, running, feed)
}

#[test]
fn a_page_learns_first_whether_it_may_pass_the_audio_on() {
    for relay in [true, false] {
        let (server, running, feed) = relay_station(relay);
        let mut page = Page::connect(server.local_addr().unwrap());
        let texts = page.until("the hello", |_, texts| !texts.is_empty());
        running.store(false, Ordering::Relaxed);
        feed.join().unwrap();
        let hello: serde_json::Value = serde_json::from_str(&texts[0]).unwrap();
        assert_eq!(hello["type"], "relay", "the first message is the hello");
        assert_eq!(hello["allowed"], relay);
        assert!(hello["max_hops"].as_u64().is_some_and(|h| h >= 1));
    }
}

/// Sends `report` on the page's "now" channel until the station shows `want` reported.
fn report_until(page: &mut Page, server: &WebRtcHandle, report: &str, want: usize) {
    let now = page.now;
    page.until(&format!("reported {want}"), |rtc, _| {
        if reported(server) == Some(want) {
            return true;
        }
        if let Some(mut c) = rtc.channel(now) {
            let _ = c.write(false, report.as_bytes());
        }
        false
    });
}

#[test]
fn listeners_that_pages_report_show_apart_from_direct_ones_and_are_capped() {
    let (server, running, feed) = relay_station(true);
    let mut page = Page::connect(server.local_addr().unwrap());
    page.until("the hello", |_, texts| !texts.is_empty());
    wait(&server, |l| l == Some(1));
    assert_eq!(reported(&server), Some(0));
    report_until(&mut page, &server, r#"{"type":"report","listeners":3}"#, 3);
    assert_eq!(
        listeners(&server),
        Some(1),
        "reported listeners are not direct ones"
    );
    report_until(
        &mut page,
        &server,
        r#"{"type":"report","listeners":5000000}"#,
        1000,
    );
    running.store(false, Ordering::Relaxed);
    feed.join().unwrap();
}

#[test]
fn a_station_that_forbids_relaying_counts_no_reported_listeners() {
    let (server, running, feed) = relay_station(false);
    let mut page = Page::connect(server.local_addr().unwrap());
    page.until("the hello", |_, texts| !texts.is_empty());
    wait(&server, |l| l == Some(1));
    let now = page.now;
    let until = Instant::now() + Duration::from_millis(500);
    page.until("time to pass", |rtc, _| {
        if let Some(mut c) = rtc.channel(now) {
            let _ = c.write(false, br#"{"type":"report","listeners":3}"#);
        }
        Instant::now() >= until
    });
    assert_eq!(reported(&server), Some(0));
    running.store(false, Ordering::Relaxed);
    feed.join().unwrap();
}

#[test]
fn the_station_answers_an_offer_that_asks_for_the_station_only() {
    // The page adds this line so that relays leave the offer alone. The station must not.
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    cfg.trackers.clear();
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let marked = offer() + "a=x-radio-station-only\r\n";
    let (code, _, answer) = http(server.local_addr().unwrap(), "POST", "/whep", &marked);
    assert_eq!(code, 201, "{answer}");
}

#[test]
fn the_station_serves_its_public_stream_only_when_it_has_one() {
    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg.clone(), consumer);
    wait(&server, |l| l == Some(0));
    let (code, _, _) = http(server.local_addr().unwrap(), "GET", "/stream", "");
    assert_eq!(code, 404, "no public stream is set");

    cfg.public_stream = Some("https://stream.example.org/live".into());
    let (_p, consumer) = rtrb::RingBuffer::new(4096);
    let server = WebRtcServer::start(cfg, consumer);
    wait(&server, |l| l == Some(0));
    let (code, _, body) = http(server.local_addr().unwrap(), "GET", "/stream", "");
    assert_eq!(code, 200);
    assert_eq!(body, "https://stream.example.org/live");
}

#[test]
fn a_station_link_carries_the_public_stream_the_page_falls_back_to() {
    let id = "A".repeat(43);
    let plain = link_for(&id, None);
    assert!(plain.ends_with(&format!("#{id}")));
    assert!(!plain.contains("stream="));
    let url = "https://stream.example.org/live?x=1&y=2#z";
    let link = link_for(&id, Some(url));
    assert!(
        link.ends_with(&format!("#{id}")),
        "the station ID stays last: {link}"
    );
    let (_, query) = link
        .split_once("?stream=")
        .expect("the link has the stream");
    let value = query.split('#').next().unwrap();
    assert!(
        !value.contains(['&', '#', '?', '/', ':']),
        "the stream address is escaped as one value: {value}"
    );
}
