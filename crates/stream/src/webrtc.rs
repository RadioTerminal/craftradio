//! WebRTC listening: browsers play Program peer to peer, as Opus over RTP.
//!
//! One port carries the listener page and WHEP signalling (TCP) and the media (UDP).
//! A public STUN server tells the station its address outside the NAT, for remote listeners.
//! There is no TURN, so a listener behind a strict NAT does not connect.

use crate::{OpusEncoder, StreamError};
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use str0m::format::Codec;
use str0m::media::{Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};

/// Free public STUN servers, in the order the station tries them.
pub const DEFAULT_STUN: &[&str] = &["stun.l.google.com:19302", "stun.cloudflare.com:3478"];

/// The listener page. The station serves it, and the project site publishes it too.
pub const LISTEN_PAGE: &str = include_str!("../../../site/index.html");

/// Samples per 20 ms Opus packet: 960 frames of interleaved stereo.
const PACKET: usize = 1920;
const MAX_REQUEST: usize = 64 * 1024;
/// The station drops a listener that does not connect in this time.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the station asks STUN again. This also keeps the NAT mapping open.
const STUN_REFRESH: Duration = Duration::from_secs(25);

/// Settings for [`WebRtcServer::start`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcConfig {
    /// The page and WHEP listen on TCP, the media on UDP, both on this port. With an
    /// unspecified address (`0.0.0.0`) the media uses the address of the default route.
    pub bind: SocketAddr,
    /// STUN servers as `host:port`. When it is empty, the station offers only its local address.
    pub stun: Vec<String>,
    pub kbps: u32,
    pub max_listeners: usize,
}

impl WebRtcConfig {
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            stun: DEFAULT_STUN.iter().map(|s| (*s).to_owned()).collect(),
            kbps: 96,
            max_listeners: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcStatus {
    Starting,
    Listening {
        /// The listener page on the local network.
        url: String,
        /// The address that STUN reported, when a server answered. Without it, only
        /// listeners on the local network can connect.
        public: Option<SocketAddr>,
        listeners: usize,
        since: jiff::Timestamp,
    },
    Failed(String),
    Stopped,
}

enum Control {
    Add(Box<Listener>),
    Remove(String),
    Stop,
}

/// Owns the server threads. Dropping it stops them.
pub struct WebRtcHandle {
    control: mpsc::Sender<Control>,
    status: Arc<Mutex<WebRtcStatus>>,
    title: Arc<Mutex<String>>,
    http: Option<SocketAddr>,
}

impl WebRtcHandle {
    pub fn status(&self) -> WebRtcStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(WebRtcStatus::Stopped)
    }
    /// Sets the title that `GET /now` gives to the listener page.
    pub fn set_title(&self, title: String) {
        if let Ok(mut t) = self.title.lock() {
            *t = title;
        }
    }
    /// The bound TCP address, when the server could bind it.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.http
    }
    pub fn stop(&self) {
        let _ = self.control.send(Control::Stop);
        if let Ok(mut s) = self.status.lock()
            && !matches!(*s, WebRtcStatus::Failed(_))
        {
            *s = WebRtcStatus::Stopped;
        }
        // A connection wakes the accept loop, so that it sees the stop.
        if let Some(addr) = self.http {
            let _ = TcpStream::connect_timeout(&loopback(addr), Duration::from_millis(200));
        }
    }
}

impl Drop for WebRtcHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn loopback(addr: SocketAddr) -> SocketAddr {
    if addr.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
    } else {
        addr
    }
}

fn set_status(shared: &Mutex<WebRtcStatus>, value: WebRtcStatus) {
    if let Ok(mut s) = shared.lock() {
        *s = value;
    }
}

fn is_running(shared: &Mutex<WebRtcStatus>) -> bool {
    shared
        .lock()
        .is_ok_and(|s| matches!(*s, WebRtcStatus::Starting | WebRtcStatus::Listening { .. }))
}

pub struct WebRtcServer;

impl WebRtcServer {
    /// Binds the ports and serves Program from `frames`. A bind error shows as
    /// [`WebRtcStatus::Failed`].
    pub fn start(cfg: WebRtcConfig, frames: rtrb::Consumer<f32>) -> WebRtcHandle {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(WebRtcStatus::Starting));
        let title = Arc::new(Mutex::new(String::new()));
        let mut handle = WebRtcHandle {
            control: tx.clone(),
            status: status.clone(),
            title: title.clone(),
            http: None,
        };
        let opened = OpusEncoder::new(cfg.kbps).and_then(|encoder| {
            let tcp = TcpListener::bind(cfg.bind)?;
            let http = tcp.local_addr()?;
            let ip = media_ip(cfg.bind.ip())?;
            let udp = UdpSocket::bind(SocketAddr::new(ip, http.port()))?;
            Ok((encoder, tcp, http, udp))
        });
        let (encoder, tcp, http, udp) = match opened {
            Ok(v) => v,
            Err(e) => {
                set_status(&status, WebRtcStatus::Failed(e.to_string()));
                return handle;
            }
        };
        handle.http = Some(http);
        let candidates = Arc::new(Mutex::new(Vec::new()));
        let taken = Arc::new(AtomicUsize::new(0));
        let url_ip = udp.local_addr().map_or(http.ip(), |a| a.ip());
        let media = Media {
            socket: udp,
            frames,
            encoder,
            stun: cfg.stun.clone(),
            candidates: candidates.clone(),
            status: status.clone(),
            taken: taken.clone(),
            url: format!("http://{}/", SocketAddr::new(url_ip, http.port())),
        };
        if let Err(e) = thread::Builder::new()
            .name("webrtc-media".into())
            .spawn(move || media.run(rx))
        {
            set_status(&status, WebRtcStatus::Failed(e.to_string()));
            return handle;
        }
        let http_state = Http {
            control: tx,
            candidates,
            title,
            status: status.clone(),
            max: cfg.max_listeners,
            taken,
        };
        if let Err(e) = thread::Builder::new()
            .name("webrtc-http".into())
            .spawn(move || http_state.serve(tcp))
        {
            set_status(&status, WebRtcStatus::Failed(e.to_string()));
        }
        handle
    }
}

/// The local address for media: the given address, or the default route.
///
/// A UDP connect sends nothing. It only selects the route.
fn media_ip(bind: IpAddr) -> std::io::Result<IpAddr> {
    if !bind.is_unspecified() {
        return Ok(bind);
    }
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    match probe.connect((Ipv4Addr::new(192, 0, 2, 1), 9)) {
        Ok(()) => Ok(probe.local_addr()?.ip()),
        Err(_) => Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)),
    }
}

struct Listener {
    /// The secret in the WHEP resource URL. Only the listener knows it, so only it can leave.
    token: String,
    rtc: Rtc,
    mid: Option<Mid>,
    pt: Option<Pt>,
    connected: bool,
    created: Instant,
}

struct Media {
    socket: UdpSocket,
    frames: rtrb::Consumer<f32>,
    encoder: OpusEncoder,
    stun: Vec<String>,
    candidates: Arc<Mutex<Vec<Candidate>>>,
    status: Arc<Mutex<WebRtcStatus>>,
    taken: Arc<AtomicUsize>,
    url: String,
}

impl Media {
    fn run(mut self, control: mpsc::Receiver<Control>) {
        let local = match self.socket.local_addr() {
            Ok(a) => a,
            Err(e) => return set_status(&self.status, WebRtcStatus::Failed(e.to_string())),
        };
        let mut stun = Stun::new(&self.stun, local);
        let mut public = stun.query_blocking(&self.socket);
        self.publish_candidates(local, public);
        let since = jiff::Timestamp::now();
        let mut listeners: Vec<Listener> = Vec::new();
        let mut samples: Vec<f32> = Vec::with_capacity(PACKET);
        let mut sample_pos: u64 = 0;
        let mut buf = vec![0u8; 2000];
        let mut shown = None;
        loop {
            loop {
                match control.try_recv() {
                    Ok(Control::Add(l)) => listeners.push(*l),
                    Ok(Control::Remove(token)) => {
                        let before = listeners.len();
                        listeners.retain(|l| l.token != token);
                        self.taken
                            .fetch_sub(before - listeners.len(), Ordering::AcqRel);
                    }
                    Ok(Control::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                        set_status(&self.status, WebRtcStatus::Stopped);
                        return;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            stun.refresh(&self.socket);
            let now = Instant::now();
            // A full ring means that this thread fell behind. Drop the backlog.
            if self.frames.slots() == self.frames.buffer().capacity() {
                while self.frames.pop().is_ok() {}
                samples.clear();
            }
            loop {
                while samples.len() < PACKET {
                    match self.frames.pop() {
                        Ok(s) => samples.push(s),
                        Err(_) => break,
                    }
                }
                if samples.len() < PACKET {
                    break;
                }
                let time = MediaTime::new(sample_pos, Frequency::FORTY_EIGHT_KHZ);
                sample_pos += (PACKET / 2) as u64;
                if listeners.iter().any(|l| l.connected)
                    && let Ok(packet) = self.encoder.encode(&samples)
                {
                    let packet: Arc<[u8]> = packet.into();
                    for l in listeners.iter_mut().filter(|l| l.connected) {
                        if let (Some(mid), Some(pt)) = (l.mid, l.pt)
                            && let Some(w) = l.rtc.writer(mid)
                        {
                            let _ = w.write(pt, now, time, packet.clone());
                        }
                    }
                }
                samples.clear();
            }
            let mut wake = now + Duration::from_millis(5);
            for l in &mut listeners {
                if let Some(t) = l.poll(&self.socket) {
                    wake = wake.min(t);
                }
            }
            let before = listeners.len();
            listeners.retain(|l| {
                l.rtc.is_alive() && (l.connected || l.created.elapsed() < CONNECT_TIMEOUT)
            });
            self.taken
                .fetch_sub(before - listeners.len(), Ordering::AcqRel);
            let count = listeners.iter().filter(|l| l.connected).count();
            if shown != Some((count, public)) {
                shown = Some((count, public));
                set_status(
                    &self.status,
                    WebRtcStatus::Listening {
                        url: self.url.clone(),
                        public,
                        listeners: count,
                        since,
                    },
                );
            }
            let wait = wake
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1));
            let _ = self.socket.set_read_timeout(Some(wait));
            let input_at = Instant::now();
            // Errors are timeouts, or ICMP "port unreachable" from a listener that left.
            if let Ok((n, source)) = self.socket.recv_from(&mut buf) {
                let data = &buf[..n];
                if let Some(found) = stun.take_response(data) {
                    if public != Some(found) {
                        public = Some(found);
                        self.publish_candidates(local, public);
                    }
                } else if let Ok(contents) = data.try_into() {
                    let input = Input::Receive(
                        input_at,
                        Receive {
                            proto: Protocol::Udp,
                            source,
                            destination: local,
                            contents,
                        },
                    );
                    if let Some(l) = listeners.iter_mut().find(|l| l.rtc.accepts(&input))
                        && l.rtc.handle_input(input).is_err()
                    {
                        l.rtc.disconnect();
                    }
                }
            }
            let now = Instant::now();
            for l in &mut listeners {
                if l.rtc.handle_input(Input::Timeout(now)).is_err() {
                    l.rtc.disconnect();
                }
            }
        }
    }

    fn publish_candidates(&self, local: SocketAddr, public: Option<SocketAddr>) {
        let mut list: Vec<Candidate> = Candidate::host(local, "udp").into_iter().collect();
        if let Some(public) = public.filter(|p| *p != local)
            && let Ok(c) = Candidate::server_reflexive(public, local, "udp")
        {
            list.push(c);
        }
        if let Ok(mut c) = self.candidates.lock() {
            *c = list;
        }
    }
}

impl Listener {
    /// Sends what the connection has to send. Returns the time it next needs.
    fn poll(&mut self, socket: &UdpSocket) -> Option<Instant> {
        loop {
            match self.rtc.poll_output() {
                Ok(Output::Timeout(t)) => return Some(t),
                Ok(Output::Transmit(t)) => {
                    let _ = socket.send_to(&t.contents, t.destination);
                }
                Ok(Output::Event(e)) => match e {
                    Event::MediaAdded(m) if m.kind == MediaKind::Audio => {
                        self.mid = Some(m.mid);
                        self.pt = self.rtc.writer(m.mid).and_then(|w| {
                            w.payload_params()
                                .find(|p| p.spec().codec == Codec::Opus)
                                .map(|p| p.pt())
                        });
                    }
                    Event::Connected => self.connected = true,
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        self.rtc.disconnect();
                    }
                    _ => {}
                },
                Err(_) => {
                    self.rtc.disconnect();
                    return None;
                }
            }
        }
    }
}

/// STUN Binding requests (RFC 8489) find the address outside the NAT.
struct Stun {
    servers: Vec<SocketAddr>,
    pending: Option<[u8; 12]>,
    last: Option<Instant>,
}

impl Stun {
    fn new(names: &[String], local: SocketAddr) -> Self {
        let servers = names
            .iter()
            .filter_map(|n| {
                n.to_socket_addrs()
                    .ok()?
                    .find(|a| a.is_ipv4() == local.is_ipv4())
            })
            .collect();
        Self {
            servers,
            pending: None,
            last: None,
        }
    }

    fn send(&mut self, socket: &UdpSocket, server: SocketAddr) {
        let id = transaction_id();
        if socket.send_to(&binding_request(id), server).is_ok() {
            self.pending = Some(id);
        }
        self.last = Some(Instant::now());
    }

    /// Asks each server in turn, and waits up to a second for each.
    fn query_blocking(&mut self, socket: &UdpSocket) -> Option<SocketAddr> {
        let mut buf = [0u8; 600];
        for server in self.servers.clone() {
            self.send(socket, server);
            let deadline = Instant::now() + Duration::from_secs(1);
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                let _ = socket.set_read_timeout(Some(left.max(Duration::from_millis(1))));
                match socket.recv_from(&mut buf) {
                    Ok((n, from)) if from == server => {
                        if let Some(addr) = self.take_response(&buf[..n]) {
                            return Some(addr);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        self.pending = None;
        None
    }

    /// Sends a new request when the last one is old. The answer comes back through
    /// [`Stun::take_response`].
    fn refresh(&mut self, socket: &UdpSocket) {
        let due = self.last.is_none_or(|t| t.elapsed() >= STUN_REFRESH);
        if due && let Some(server) = self.servers.first().copied() {
            self.send(socket, server);
        }
    }

    /// The mapped address, if `data` answers the request in flight.
    fn take_response(&mut self, data: &[u8]) -> Option<SocketAddr> {
        let addr = parse_binding_response(data, self.pending?)?;
        self.pending = None;
        Some(addr)
    }
}

const MAGIC: u32 = 0x2112_A442;

fn transaction_id() -> [u8; 12] {
    let mut id = [0u8; 12];
    let a = std::hash::RandomState::new().build_hasher().finish();
    let mut h = std::hash::RandomState::new().build_hasher();
    h.write_u64(a);
    let b = h.finish();
    id[..8].copy_from_slice(&a.to_le_bytes());
    id[8..].copy_from_slice(&b.to_le_bytes()[..4]);
    id
}

/// A Binding request without attributes.
pub fn binding_request(id: [u8; 12]) -> [u8; 20] {
    let mut m = [0u8; 20];
    m[..2].copy_from_slice(&0x0001u16.to_be_bytes());
    m[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    m[8..].copy_from_slice(&id);
    m
}

/// The address in a Binding success response for transaction `id`. It reads
/// XOR-MAPPED-ADDRESS, or MAPPED-ADDRESS from an old server.
pub fn parse_binding_response(data: &[u8], id: [u8; 12]) -> Option<SocketAddr> {
    let header = data.get(..20)?;
    if header[..2] != 0x0101u16.to_be_bytes()
        || header[4..8] != MAGIC.to_be_bytes()
        || header[8..20] != id
    {
        return None;
    }
    let len = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut attrs = data.get(20..20 + len)?;
    let mut mapped = None;
    while attrs.len() >= 4 {
        let kind = u16::from_be_bytes([attrs[0], attrs[1]]);
        let alen = u16::from_be_bytes([attrs[2], attrs[3]]) as usize;
        let value = attrs.get(4..4 + alen)?;
        match kind {
            0x0020 => return address(value, true, &id),
            0x0001 => mapped = address(value, false, &id),
            _ => {}
        }
        attrs = attrs.get((4 + alen).div_ceil(4) * 4..).unwrap_or(&[]);
    }
    mapped
}

fn address(value: &[u8], xor: bool, id: &[u8; 12]) -> Option<SocketAddr> {
    let family = *value.get(1)?;
    let mut port = u16::from_be_bytes([*value.get(2)?, *value.get(3)?]);
    let magic = MAGIC.to_be_bytes();
    if xor {
        port ^= (MAGIC >> 16) as u16;
    }
    match family {
        0x01 => {
            let mut ip: [u8; 4] = value.get(4..8)?.try_into().ok()?;
            if xor {
                for (b, m) in ip.iter_mut().zip(magic) {
                    *b ^= m;
                }
            }
            Some(SocketAddr::new(IpAddr::from(ip), port))
        }
        0x02 => {
            let mut ip: [u8; 16] = value.get(4..20)?.try_into().ok()?;
            if xor {
                for (b, k) in ip.iter_mut().zip(magic.iter().chain(id.iter())) {
                    *b ^= k;
                }
            }
            Some(SocketAddr::new(IpAddr::from(ip), port))
        }
        _ => None,
    }
}

struct Http {
    control: mpsc::Sender<Control>,
    candidates: Arc<Mutex<Vec<Candidate>>>,
    title: Arc<Mutex<String>>,
    status: Arc<Mutex<WebRtcStatus>>,
    max: usize,
    /// Listeners that connect or play now. The media thread counts down when one leaves.
    taken: Arc<AtomicUsize>,
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct Response {
    code: u16,
    content_type: &'static str,
    location: Option<String>,
    body: Vec<u8>,
}

impl Http {
    fn serve(mut self, tcp: TcpListener) {
        for stream in tcp.incoming() {
            if !is_running(&self.status) {
                return;
            }
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            let response = match read_request(&mut stream) {
                Ok(req) => self.route(&req),
                Err(_) => Response::text(400, "bad request"),
            };
            let _ = response.write(&mut stream);
        }
    }

    fn route(&mut self, req: &Request) -> Response {
        let path = req.path.split('?').next().unwrap_or("");
        match (req.method.as_str(), path) {
            ("OPTIONS", _) => Response::text(204, ""),
            ("GET", "/" | "/index.html") => Response {
                code: 200,
                content_type: "text/html; charset=utf-8",
                location: None,
                body: LISTEN_PAGE.as_bytes().to_vec(),
            },
            ("GET", "/now") => {
                let title = self.title.lock().map(|t| t.clone()).unwrap_or_default();
                Response::text(200, &title)
            }
            ("POST", "/whep") => self.whep(&req.body),
            ("DELETE", p) if p.starts_with("/whep/") => {
                let token = &p["/whep/".len()..];
                if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Response::text(404, "no such listener");
                }
                let _ = self.control.send(Control::Remove(token.to_owned()));
                Response::text(200, "")
            }
            (_, "/" | "/now" | "/whep") => Response::text(405, "method not allowed"),
            _ => Response::text(404, "not found"),
        }
    }

    fn whep(&mut self, body: &[u8]) -> Response {
        if !matches!(
            self.status.lock().map(|s| s.clone()),
            Ok(WebRtcStatus::Listening { .. })
        ) {
            return Response::text(503, "the station is not ready");
        }
        // The count includes listeners that still connect, so parallel offers cannot pass the limit.
        if self.taken.load(Ordering::Acquire) >= self.max {
            return Response::text(503, "the station has its maximum of listeners");
        }
        let Ok(sdp) = std::str::from_utf8(body) else {
            return Response::text(415, "expected an SDP offer");
        };
        let local = self
            .candidates
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        match answer(sdp, &local) {
            Ok((rtc, answer)) => {
                let Some(token) = listener_token() else {
                    return Response::text(503, "the station has no random source");
                };
                let listener = Listener {
                    token: token.clone(),
                    rtc,
                    mid: None,
                    pt: None,
                    connected: false,
                    created: Instant::now(),
                };
                if self.control.send(Control::Add(Box::new(listener))).is_err() {
                    return Response::text(503, "the station stopped");
                }
                self.taken.fetch_add(1, Ordering::AcqRel);
                Response {
                    code: 201,
                    content_type: "application/sdp",
                    location: Some(format!("/whep/{token}")),
                    body: answer.into_bytes(),
                }
            }
            Err(e) => Response::text(400, &e),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request, StreamError> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if data.len() > MAX_REQUEST {
            return Err(StreamError::Invalid("request too large".into()));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(StreamError::Invalid("connection closed".into()));
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_owned();
    let path = first.next().unwrap_or("").to_owned();
    let length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_REQUEST {
        return Err(StreamError::Invalid("body too large".into()));
    }
    let mut body = data[head_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Request { method, path, body })
}

impl Response {
    fn text(code: u16, body: &str) -> Self {
        Self {
            code,
            content_type: "text/plain; charset=utf-8",
            location: None,
            body: body.as_bytes().to_vec(),
        }
    }

    fn write(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        let reason = match self.code {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            400 => "Bad Request",
            404 => "Not Found",
            405 => "Method Not Allowed",
            415 => "Unsupported Media Type",
            503 => "Service Unavailable",
            _ => "Error",
        };
        // CORS and Private Network Access let the project site on GitHub Pages call
        // a station on this computer.
        let mut head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Allow-Private-Network: true\r\nAccess-Control-Expose-Headers: Location\r\n",
            self.code,
            self.content_type,
            self.body.len()
        );
        if let Some(l) = &self.location {
            head.push_str(&format!("Location: {l}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

/// 128 random bits from the system source, as hex.
fn listener_token() -> Option<String> {
    let mut bytes = [0u8; 16];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Makes the connection for one listener from its offer.
fn answer(offer: &str, local: &[Candidate]) -> Result<(Rtc, String), String> {
    let offer = str0m::change::SdpOffer::from_sdp_string(offer).map_err(|e| e.to_string())?;
    let mut rtc = RtcConfig::new()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        .build(Instant::now());
    for c in local {
        rtc.add_local_candidate(c.clone());
    }
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| e.to_string())?;
    Ok((rtc, answer.to_sdp_string()))
}
