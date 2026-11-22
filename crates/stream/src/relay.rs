//! Signalling through public WebTorrent trackers, so listeners anywhere can connect.
//!
//! The station keeps a WebSocket to each tracker, in the swarm of its station ID.
//! A listener page posts its offer to that swarm, and the station answers.
//! The audio goes peer to peer.

// The station signs each answer with its Ed25519 key. The link holds the public key.
// So the page knows that the answer is from the station, not from the tracker.

use crate::webrtc::{Gate, Origin};
use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Connector, Message};

/// Free public trackers. The station joins all of them, so it works while one tracker fails.
pub const DEFAULT_TRACKERS: &[&str] = &[
    "wss://tracker.openwebtorrent.com",
    "wss://tracker.webtorrent.dev",
];

/// Tracker offers per second that the station answers, for all trackers together.
/// This limit keeps a flood of offers cheap.
const OFFERS_PER_SECOND: f64 = 10.0;
/// The station announces again after this time, unless the tracker asks for less.
const ANNOUNCE: Duration = Duration::from_secs(60);
const MAX_MESSAGE: usize = 256 * 1024;
/// The SDP line that holds the signature of the answer.
pub const SIGNATURE_LINE: &str = "a=x-radio-sig:";

/// The station key. Its public half is the station ID in the listen link.
pub struct Identity {
    pair: Ed25519KeyPair,
}

impl Identity {
    /// A new random key, as PKCS#8 bytes to keep.
    pub fn generate() -> Option<Vec<u8>> {
        let rng = ring::rand::SystemRandom::new();
        Ed25519KeyPair::generate_pkcs8(&rng)
            .ok()
            .map(|d| d.as_ref().to_vec())
    }

    pub fn new(pkcs8: &[u8]) -> Option<Self> {
        Ed25519KeyPair::from_pkcs8(pkcs8)
            .ok()
            .map(|pair| Self { pair })
    }

    /// The public key in base64url without padding: 43 characters.
    pub fn id(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref())
    }

    /// The swarm on the trackers: the first 20 hex digits of SHA-256 of the public key.
    pub fn info_hash(&self) -> String {
        let digest = ring::digest::digest(&ring::digest::SHA256, self.pair.public_key().as_ref());
        hex(digest.as_ref())[..20].to_owned()
    }

    /// The answer with a signature line at the end. The signature covers the offer ID,
    /// a newline and the SDP before the signature line.
    pub fn sign_answer(&self, offer_id: &str, sdp: &str) -> String {
        let mut sdp = sdp.to_owned();
        if !sdp.ends_with("\r\n") {
            sdp.push_str("\r\n");
        }
        let message = format!("{offer_id}\n{sdp}");
        let sig = self.pair.sign(message.as_bytes());
        let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.as_ref());
        format!("{sdp}{SIGNATURE_LINE}{sig}\r\n")
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Starts one thread per tracker. Each thread ends when the station stops.
pub(crate) fn start(
    trackers: &[String],
    identity: Identity,
    gate: Gate,
    connected: Arc<AtomicUsize>,
) {
    let shared = Arc::new(Shared {
        identity,
        gate,
        budget: Mutex::new(Budget::new()),
    });
    for url in trackers {
        let (url, shared, connected) = (url.clone(), shared.clone(), connected.clone());
        let _ = thread::Builder::new()
            .name("webrtc-relay".into())
            .spawn(move || run(&url, &shared, &connected));
    }
}

/// What all tracker threads of one station share.
struct Shared {
    identity: Identity,
    gate: Gate,
    budget: Mutex<Budget>,
}

/// A token bucket for [`OFFERS_PER_SECOND`], with a burst of one second.
struct Budget {
    tokens: f64,
    last: Instant,
}

impl Budget {
    fn new() -> Self {
        Self {
            tokens: OFFERS_PER_SECOND,
            last: Instant::now(),
        }
    }

    /// True when the station can answer one more offer now.
    fn take(&mut self) -> bool {
        let now = Instant::now();
        let refill = now.duration_since(self.last).as_secs_f64() * OFFERS_PER_SECOND;
        self.tokens = (self.tokens + refill).min(OFFERS_PER_SECOND);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Counts one connected tracker until it drops.
struct Connected(Arc<AtomicUsize>);
impl Drop for Connected {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn run(url: &str, shared: &Shared, connected: &Arc<AtomicUsize>) {
    let gate = &shared.gate;
    let peer_id = crate::webrtc::random_hex(10).unwrap_or_else(|| "radio-station-000000".into());
    let mut delay = 2;
    while gate.running() {
        let started = Instant::now();
        if let Ok(socket) = connect(url) {
            connected.fetch_add(1, Ordering::AcqRel);
            let _count = Connected(connected.clone());
            serve(socket, shared, &peer_id);
        }
        if started.elapsed() > Duration::from_secs(60) {
            delay = 2;
        }
        // Sleep in short steps, so a stop ends the thread soon.
        let until = Instant::now() + Duration::from_secs(delay);
        while gate.running() && Instant::now() < until {
            thread::sleep(Duration::from_millis(200));
        }
        delay = (delay * 2).min(60);
    }
}

type Socket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

fn connect(url: &str) -> Result<Socket, String> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("wss://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("ws://") {
        (false, r)
    } else {
        return Err("a tracker address starts with wss:// or ws://".into());
    };
    let host_port = rest.split('/').next().unwrap_or("");
    let default_port = if tls { 443 } else { 80 };
    let addr = if host_port.contains(':') {
        host_port.to_owned()
    } else {
        format!("{host_port}:{default_port}")
    };
    let target = addr
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("the tracker has no address")?;
    let stream =
        TcpStream::connect_timeout(&target, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let connector = if tls {
        Connector::Rustls(crate::output::system_tls()?)
    } else {
        Connector::Plain
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let (socket, _) =
        tungstenite::client_tls_with_config(url, stream, Some(config), Some(connector))
            .map_err(|e| e.to_string())?;
    // Short reads let the thread announce again and see a stop.
    if let Some(tcp) = tcp_of(&socket) {
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(1)));
    }
    Ok(socket)
}

fn tcp_of(socket: &Socket) -> Option<&TcpStream> {
    match socket.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => Some(s),
        tungstenite::stream::MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
        _ => None,
    }
}

fn serve(mut socket: Socket, shared: &Shared, peer_id: &str) {
    let gate = &shared.gate;
    let info_hash = shared.identity.info_hash();
    let announce = |event: Option<&str>| {
        let mut m = json!({
            "action": "announce",
            "info_hash": info_hash,
            "peer_id": peer_id,
            "uploaded": 0,
            "downloaded": 0,
            "left": 0,
            "numwant": 0,
            "offers": [],
        });
        if let Some(e) = event {
            m["event"] = json!(e);
        }
        Message::text(m.to_string())
    };
    if socket.send(announce(Some("started"))).is_err() {
        return;
    }
    let mut interval = ANNOUNCE;
    let mut last = Instant::now();
    while gate.running() {
        if last.elapsed() >= interval {
            if socket.send(announce(None)).is_err() {
                return;
            }
            last = Instant::now();
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let Ok(msg) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                if msg["info_hash"].as_str() != Some(info_hash.as_str()) {
                    continue;
                }
                if let Some(secs) = msg["interval"].as_u64() {
                    interval = Duration::from_secs(secs.clamp(10, 300)).min(ANNOUNCE);
                }
                if let Some(reply) = answer_offer(&msg, shared, &info_hash, peer_id)
                    && socket.send(Message::text(reply.to_string())).is_err()
                {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
    }
    let _ = socket.send(announce(Some("stopped")));
    let _ = socket.close(None);
}

/// The tracker message that answers `msg`, when it carries an offer.
fn answer_offer(msg: &Value, shared: &Shared, info_hash: &str, peer_id: &str) -> Option<Value> {
    let sdp = msg["offer"]["sdp"].as_str()?;
    let offer_id = msg["offer_id"].as_str()?;
    let from = msg["peer_id"].as_str()?;
    if offer_id.len() > 64 || from.chars().count() != 20 {
        return None;
    }
    // Over the budget, the offer gets no answer. The page posts it again after 5 s.
    if !shared.budget.lock().is_ok_and(|mut b| b.take()) {
        return None;
    }
    let identity = &shared.identity;
    let (_token, answer) = shared.gate.admit(sdp, Origin::Tracker).ok()?;
    Some(json!({
        "action": "announce",
        "info_hash": info_hash,
        "peer_id": peer_id,
        "to_peer_id": from,
        "offer_id": offer_id,
        "answer": { "type": "answer", "sdp": identity.sign_answer(offer_id, &answer) },
    }))
}
