use base64::Engine;
use radio_stream::*;
use std::net::{TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, RtcConfig};

fn relays(h: &WebRtcHandle) -> Option<usize> {
    match h.status() {
        WebRtcStatus::Listening { relays, .. } => Some(relays),
        _ => None,
    }
}

/// A listener anywhere plays the station through a tracker, and checks that the answer
/// came from the station key in the link.
#[test]
fn a_listener_connects_through_a_tracker_and_gets_audio_and_the_title() {
    let pkcs8 = Identity::generate().unwrap();
    let identity = Identity::new(&pkcs8).unwrap();
    let link = listen_link(&pkcs8).unwrap();
    assert!(link.starts_with(LISTEN_SITE));
    assert!(link.ends_with(&identity.id()));

    let tracker = TcpListener::bind("127.0.0.1:0").unwrap();
    let tracker_url = format!("ws://{}", tracker.local_addr().unwrap());
    let (offer_tx, offer_rx) = mpsc::channel::<String>();
    let (answer_tx, answer_rx) = mpsc::channel::<serde_json::Value>();
    let info_hash = identity.info_hash();
    let fake = std::thread::spawn(move || {
        let (stream, _) = tracker.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let hello: serde_json::Value =
            serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(hello["info_hash"], info_hash.as_str());
        let station_peer = hello["peer_id"].as_str().unwrap().to_owned();
        assert_eq!(station_peer.len(), 20);
        let offer = offer_rx.recv().unwrap();
        let msg = serde_json::json!({
            "action": "announce", "info_hash": info_hash, "peer_id": "listener-peer-000001",
            "offer_id": "offer-0001", "offer": { "type": "offer", "sdp": offer },
        });
        ws.send(tungstenite::Message::text(msg.to_string()))
            .unwrap();
        loop {
            let reply: serde_json::Value = match ws.read().unwrap() {
                tungstenite::Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
                _ => continue,
            };
            if reply.get("answer").is_some() {
                answer_tx.send(reply).unwrap();
                break;
            }
        }
        // Keep the socket open while the listener connects.
        std::thread::sleep(Duration::from_secs(3));
    });

    let mut cfg = WebRtcConfig::new("127.0.0.1:0".parse().unwrap());
    cfg.stun.clear();
    cfg.identity = Some(pkcs8);
    cfg.trackers = vec![tracker_url];
    let (mut producer, consumer) = rtrb::RingBuffer::new(48_000 * 2);
    let server = WebRtcServer::start(cfg, consumer);
    server.set_title("Relay Title".into());
    let deadline = Instant::now() + Duration::from_secs(5);
    while relays(&server) != Some(1) {
        assert!(Instant::now() < deadline, "{:?}", server.status());
        std::thread::sleep(Duration::from_millis(20));
    }
    let feed = std::thread::spawn(move || {
        for n in 0..500u64 {
            for i in 0..960u64 {
                let s = (((n * 960 + i) as f32) * 0.06).sin() * 0.2;
                let _ = producer.push(s);
                let _ = producer.push(s);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let local = socket.local_addr().unwrap();
    let mut rtc = RtcConfig::new()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        .build(Instant::now());
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
    let mut api = rtc.sdp_api();
    api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
    api.add_channel("now".into());
    let (offer, pending) = api.apply().unwrap();
    offer_tx.send(offer.to_sdp_string()).unwrap();

    let reply = answer_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(reply["to_peer_id"], "listener-peer-000001");
    assert_eq!(reply["offer_id"], "offer-0001");
    let signed = reply["answer"]["sdp"].as_str().unwrap();
    let (sdp, sig) = signed.split_once(SIGNATURE_LINE).unwrap();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let public = b64.decode(identity.id()).unwrap();
    let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public);
    let sig = b64.decode(sig.trim()).unwrap();
    key.verify(format!("offer-0001\n{sdp}").as_bytes(), &sig)
        .expect("the answer carries the station signature");
    let mut forged = sdp.replace("a=", "a= ");
    forged.truncate(sdp.len());
    assert!(
        key.verify(format!("offer-0001\n{forged}").as_bytes(), &sig)
            .is_err()
    );

    let answer = str0m::change::SdpAnswer::from_sdp_string(sdp).unwrap();
    rtc.sdp_api().accept_answer(pending, answer).unwrap();
    let mut buf = vec![0u8; 2000];
    let (mut audio, mut title) = (0, String::new());
    let deadline = Instant::now() + Duration::from_secs(10);
    while audio < 10 || title.is_empty() {
        assert!(Instant::now() < deadline, "audio {audio}, title {title:?}");
        let timeout = loop {
            match rtc.poll_output().unwrap() {
                Output::Timeout(t) => break t,
                Output::Transmit(t) => {
                    socket.send_to(&t.contents, t.destination).unwrap();
                }
                Output::Event(Event::MediaData(_)) => audio += 1,
                Output::Event(Event::ChannelData(d)) => {
                    let m: serde_json::Value = serde_json::from_slice(&d.data).unwrap();
                    if m["type"] == "now" {
                        title = m["title"].as_str().unwrap().to_owned();
                    }
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
    assert_eq!(title, "Relay Title");
    server.stop();
    feed.join().unwrap();
    fake.join().unwrap();
}

#[test]
fn a_large_cover_arrives_in_parts_that_browsers_accept() {
    let cover: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let messages = cover_messages("Night Shift", "North Relay", 9, "image/jpeg", &cover);
    let meta: serde_json::Value = serde_json::from_str(&messages[0]).unwrap();
    assert_eq!(meta["type"], "now");
    assert_eq!(meta["cover"]["id"], 9);
    let parts = meta["cover"]["parts"].as_u64().unwrap() as usize;
    assert_eq!(parts, messages.len() - 1);
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut back = Vec::new();
    for (i, m) in messages[1..].iter().enumerate() {
        assert!(m.len() < 256 * 1024, "part {i} is {} bytes", m.len());
        let part: serde_json::Value = serde_json::from_str(m).unwrap();
        assert_eq!(part["part"], i);
        back.extend(b64.decode(part["data"].as_str().unwrap()).unwrap());
    }
    assert_eq!(back, cover);
}
