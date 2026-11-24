use crate::StreamError;
use base64::Engine;
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use std::{
    cell::Cell,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

/// Why an encoder or the Ogg muxer failed.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Opus(#[from] opus::Error),
}

/// Encodes exactly 20 ms of 48 kHz stereo audio per packet.
pub struct OpusEncoder(opus::Encoder);
impl OpusEncoder {
    pub fn new(kbps: u32) -> Result<Self, EncodeError> {
        if !(6..=510).contains(&kbps) {
            return Err(EncodeError::Invalid("bitrate must be 6..510 kbps".into()));
        }
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)?;
        encoder.set_bitrate(opus::Bitrate::Bits((kbps * 1000) as i32))?;
        Ok(Self(encoder))
    }
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, EncodeError> {
        if samples.len() != 1920 {
            return Err(EncodeError::Invalid(
                "expected 1920 interleaved samples".into(),
            ));
        }
        let mut packet = vec![0; 4000];
        let len = self.0.encode_float(samples, &mut packet)?;
        packet.truncate(len);
        Ok(packet)
    }
    pub fn pre_skip(&mut self) -> Result<u16, EncodeError> {
        Ok(self.0.get_lookahead()? as u16)
    }
}

/// Writes Ogg pages, with a distinct serial number for each metadata chain.
pub struct OggMuxer<W: Write> {
    writer: PacketWriter<'static, W>,
    serial: u32,
    granule: u64,
}
impl<W: Write> OggMuxer<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: PacketWriter::new(writer),
            serial: 0,
            granule: 0,
        }
    }
    /// The writer the pages go to.
    pub fn get_mut(&mut self) -> &mut W {
        self.writer.inner_mut()
    }
    pub fn start(
        &mut self,
        title: &str,
        artist: Option<&str>,
        pre_skip: u16,
    ) -> Result<(), EncodeError> {
        self.serial = self.serial.wrapping_add(1);
        self.granule = 0;
        let mut head = b"OpusHead\x01\x02".to_vec();
        head.extend(pre_skip.to_le_bytes());
        head.extend(48_000u32.to_le_bytes());
        head.extend([0, 0, 0]);
        self.writer
            .write_packet(head, self.serial, PacketWriteEndInfo::EndPage, 0)?;
        let mut tags = b"OpusTags".to_vec();
        tags.extend(12u32.to_le_bytes());
        tags.extend(b"radio-stream");
        tags.extend(if artist.is_some() { 2u32 } else { 1u32 }.to_le_bytes());
        for comment in
            std::iter::once(format!("TITLE={title}")).chain(artist.map(|a| format!("ARTIST={a}")))
        {
            tags.extend((comment.len() as u32).to_le_bytes());
            tags.extend(comment.as_bytes());
        }
        self.writer
            .write_packet(tags, self.serial, PacketWriteEndInfo::EndPage, 0)?;
        Ok(())
    }
    pub fn packet(&mut self, packet: Vec<u8>, end: bool) -> Result<(), EncodeError> {
        self.granule += 960;
        self.writer.write_packet(
            packet,
            self.serial,
            if end {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::EndPage
            },
            self.granule,
        )?;
        Ok(())
    }
}

/// Output encoding and bitrate in kilobits per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFormat {
    Opus {
        kbps: u32,
    },
    /// Requires the default `mp3` feature.
    Mp3 {
        kbps: u32,
    },
}
impl StreamFormat {
    fn kbps(self) -> u32 {
        match self {
            Self::Opus { kbps } | Self::Mp3 { kbps } => kbps,
        }
    }
    fn content_type(self) -> &'static str {
        match self {
            Self::Opus { .. } => "audio/ogg",
            Self::Mp3 { .. } => "audio/mpeg",
        }
    }
}

/// Icecast source settings. TLS verifies the server certificate and host.
#[derive(Clone)]
pub struct IcecastConfig {
    pub host: String,
    pub port: u16,
    pub mount: String,
    pub user: String,
    pub password: String,
    pub name: String,
    pub description: String,
    pub genre: String,
    pub format: StreamFormat,
    pub tls: bool,
}
/// Use TLS except for numeric loopback addresses and localhost.
pub fn default_tls(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return false;
    }
    !host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

struct SourceConfig {
    settings: IcecastConfig,
    extra_roots: Vec<Vec<u8>>,
    dropped_system_roots: Cell<usize>,
}
impl std::ops::Deref for SourceConfig {
    type Target = IcecastConfig;
    fn deref(&self) -> &Self::Target {
        &self.settings
    }
}

trait Transport: Read + Write {
    /// How long a read waits for the server.
    fn read_within(&mut self, timeout: Duration) -> std::io::Result<()>;
}
impl Transport for TcpStream {
    fn read_within(&mut self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))
    }
}
impl Transport for rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    fn read_within(&mut self, timeout: Duration) -> std::io::Result<()> {
        self.sock.set_read_timeout(Some(timeout))
    }
}
type SourceStream = Box<dyn Transport>;

struct SystemRoots {
    store: rustls::RootCertStore,
    dropped: usize,
}

static SYSTEM_ROOTS: OnceLock<SystemRoots> = OnceLock::new();
static SYSTEM_TLS: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();

fn tls_config(cfg: &SourceConfig) -> Result<Arc<rustls::ClientConfig>, String> {
    if cfg.extra_roots.is_empty() {
        let config = system_tls()?;
        cfg.dropped_system_roots
            .set(SYSTEM_ROOTS.get().map_or(0, |roots| roots.dropped));
        return Ok(config);
    }
    let (config, dropped) = build_tls_config(&cfg.extra_roots)?;
    cfg.dropped_system_roots.set(dropped);
    Ok(config)
}

/// The TLS client settings with the system trust roots, built once.
pub(crate) fn system_tls() -> Result<Arc<rustls::ClientConfig>, String> {
    if let Some(config) = SYSTEM_TLS.get() {
        return Ok(config.clone());
    }
    let (config, _) = build_tls_config(&[])?;
    let _ = SYSTEM_TLS.set(config.clone());
    Ok(config)
}

/// Returns the settings and the count of system roots that did not load.
fn build_tls_config(extra_roots: &[Vec<u8>]) -> Result<(Arc<rustls::ClientConfig>, usize), String> {
    let loaded;
    let system = if let Some(system) = SYSTEM_ROOTS.get() {
        system
    } else {
        let mut store = rustls::RootCertStore::empty();
        let (_, dropped) =
            store.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        loaded = SystemRoots { store, dropped };
        if !loaded.store.is_empty() {
            let _ = SYSTEM_ROOTS.set(loaded);
            SYSTEM_ROOTS.get().ok_or("trust root cache unavailable")?
        } else {
            &loaded
        }
    };
    let mut roots = system.store.clone();
    for der in extra_roots {
        roots
            .add(der.clone().into())
            .map_err(|_| "invalid extra root")?;
    }
    if roots.is_empty() {
        return Err(format!(
            "no usable trust roots ({} dropped system roots)",
            system.dropped
        ));
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "configuration failed")?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok((Arc::new(config), system.dropped))
}

/// Apply the total limit inside complete_io, which can perform many reads.
struct HandshakeIo<'a> {
    socket: &'a mut TcpStream,
    deadline: Instant,
}
impl HandshakeIo<'_> {
    fn timeout(&self) -> std::io::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "TLS handshake deadline",
            ));
        }
        Ok(remaining.min(Duration::from_secs(2)))
    }
}
impl Read for HandshakeIo<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket.set_read_timeout(Some(self.timeout()?))?;
        self.socket.read(bytes)
    }
}
impl Write for HandshakeIo<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.socket.set_write_timeout(Some(self.timeout()?))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.timeout()?;
        self.socket.flush()
    }
}

/// Open and verify TLS before any HTTP request or credentials are written.
fn open_transport(cfg: &SourceConfig) -> Result<SourceStream, StreamError> {
    let address = (cfg.host.as_str(), cfg.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| StreamError::Invalid("host has no address".into()))?;
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    if !cfg.tls {
        return Ok(Box::new(socket));
    }
    let tls_error = |message: &str| {
        // Do not include peer-supplied text or credentials in errors.
        StreamError::Invalid(format!("TLS: {message} for {}", cfg.host))
    };
    let config = tls_config(cfg).map_err(|error| tls_error(&error))?;
    let name = rustls::pki_types::ServerName::try_from(cfg.host.clone())
        .map_err(|_| tls_error("invalid server name"))?;
    let mut connection = rustls::ClientConnection::new(config, name)
        .map_err(|_| tls_error("configuration failed"))?;
    let mut io = HandshakeIo {
        socket: &mut socket,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    while connection.is_handshaking() {
        connection.complete_io(&mut io).map_err(|error| {
            if Instant::now() >= io.deadline {
                tls_error("handshake timed out")
            } else if matches!(
                error
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<rustls::Error>()),
                Some(rustls::Error::InvalidCertificate(_))
            ) {
                tls_error("certificate not trusted")
            } else {
                tls_error("handshake failed")
            }
        })?;
    }
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok(Box::new(rustls::StreamOwned::new(connection, socket)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamStatus {
    Connecting,
    Connected {
        since: jiff::Timestamp,
        last_metadata_error: Option<String>,
        /// System certificates rejected while loading the trust store.
        dropped_system_roots: usize,
    },
    Retrying {
        in_secs: u64,
        last_error: String,
    },
    Stopped,
}
enum Control {
    Title(String),
    Stop,
}
/// Owns the source worker. Dropping it requests shutdown without blocking.
pub struct IcecastHandle {
    control: mpsc::Sender<Control>,
    status: Arc<Mutex<StreamStatus>>,
}
impl IcecastHandle {
    pub fn set_title(&self, title: String) {
        let _ = self
            .control
            .send(Control::Title(title.replace(['\r', '\n'], "")));
    }
    pub fn status(&self) -> StreamStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(StreamStatus::Stopped)
    }
    pub fn stop(&self) {
        let _ = self.control.send(Control::Stop);
    }
}
impl Drop for IcecastHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
pub struct IcecastClient;
fn status(shared: &Mutex<StreamStatus>, value: StreamStatus) {
    if let Ok(mut s) = shared.lock() {
        *s = value;
    }
}
impl IcecastClient {
    pub fn start(cfg: IcecastConfig, frames: rtrb::Consumer<f32>) -> IcecastHandle {
        Self::start_metered(cfg, frames, Arc::default())
    }

    /// Like [`IcecastClient::start`], and `meter` gets each block that goes to the encoder,
    /// so it reads silence while the source is not connected.
    pub fn start_metered(
        cfg: IcecastConfig,
        frames: rtrb::Consumer<f32>,
        meter: Arc<crate::LevelMeter>,
    ) -> IcecastHandle {
        Self::start_inner(cfg, frames, Vec::new(), meter)
    }

    /// Add DER trust anchors, for private servers and TLS integration tests.
    /// Certificate and host verification remain enabled.
    #[doc(hidden)]
    pub fn start_with_extra_roots(
        cfg: IcecastConfig,
        frames: rtrb::Consumer<f32>,
        extra_roots: Vec<Vec<u8>>,
    ) -> IcecastHandle {
        Self::start_inner(cfg, frames, extra_roots, Arc::default())
    }

    fn start_inner(
        cfg: IcecastConfig,
        mut frames: rtrb::Consumer<f32>,
        extra_roots: Vec<Vec<u8>>,
        meter: Arc<crate::LevelMeter>,
    ) -> IcecastHandle {
        let cfg = SourceConfig {
            settings: cfg,
            extra_roots,
            dropped_system_roots: Cell::new(0),
        };
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(StreamStatus::Connecting));
        let state = shared.clone();
        let spawned = thread::Builder::new()
            .name("icecast-source".into())
            .spawn(move || {
                let mut title = String::new();
                let mut delay = 1;
                loop {
                    status(&state, StreamStatus::Connecting);
                    let result = send(
                        &cfg,
                        &mut frames,
                        &rx,
                        &state,
                        &mut title,
                        &mut delay,
                        &meter,
                    );
                    match result {
                        Ok(()) => break,
                        Err(error) => {
                            status(
                                &state,
                                StreamStatus::Retrying {
                                    in_secs: delay,
                                    last_error: error.to_string(),
                                },
                            );
                            let deadline = std::time::Instant::now() + Duration::from_secs(delay);
                            loop {
                                match rx.recv_timeout(
                                    deadline.saturating_duration_since(std::time::Instant::now()),
                                ) {
                                    Ok(Control::Title(t)) => title = t,
                                    Ok(Control::Stop)
                                    | Err(mpsc::RecvTimeoutError::Disconnected) => {
                                        status(&state, StreamStatus::Stopped);
                                        return;
                                    }
                                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                                }
                            }
                            delay = (delay * 2).min(30);
                        }
                    }
                }
                status(&state, StreamStatus::Stopped);
            });
        if spawned.is_err() {
            status(&shared, StreamStatus::Stopped);
        }
        IcecastHandle {
            control: tx,
            status: shared,
        }
    }
}
/// The body length a source request declares. The stream has no end, so this is the
/// largest length a server can hold in a signed 64-bit number. Without a declared body an
/// HTTP proxy in front of the server forwards the request and drops the audio.
const ENDLESS_BODY: u64 = i64::MAX as u64;

/// An open source connection, and whether the server said 200 before the audio.
struct Source {
    socket: SourceStream,
    /// False when the server sent nothing within the read timeout. A server that reads the
    /// declared body through a proxy replies only when the body ends.
    replied: bool,
}

/// The status code of a reply, or nothing when the reply has no three-digit code.
fn status_code(reply: &[u8]) -> String {
    String::from_utf8_lossy(reply)
        .split_whitespace()
        .nth(1)
        .filter(|code| code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("")
        .to_owned()
}

/// The reply of a server that accepted a source in silence. Behind an HTTP proxy a refusal
/// arrives only after the first audio, so the source keeps listening while it sends.
struct LateReply {
    bytes: Vec<u8>,
    next: Instant,
}
impl LateReply {
    /// Reads that return at once, so listening never holds up the audio.
    fn start(socket: &mut dyn Transport) -> Result<Self, StreamError> {
        socket.read_within(Duration::from_millis(1))?;
        Ok(Self {
            bytes: Vec::new(),
            next: Instant::now(),
        })
    }
    fn due(&self) -> bool {
        Instant::now() >= self.next
    }
    /// True when the server said 200. A refusal or a closed connection is an error.
    fn check(&mut self, socket: &mut dyn Transport) -> Result<bool, StreamError> {
        self.next = Instant::now() + Duration::from_millis(100);
        let mut part = [0; 256];
        loop {
            match socket.read(&mut part) {
                Ok(0) => {
                    return Err(StreamError::Invalid(
                        "the server closed the source connection".into(),
                    ));
                }
                Ok(n) => {
                    self.bytes.extend_from_slice(&part[..n]);
                    if self.bytes.contains(&b'\n') || self.bytes.len() >= 8192 {
                        let code = status_code(&self.bytes);
                        if code == "200" {
                            return Ok(true);
                        }
                        return Err(StreamError::Invalid(format!(
                            "source rejected: HTTP {code}"
                        )));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(false);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

fn connect(cfg: &SourceConfig, method: &str) -> Result<Source, StreamError> {
    if cfg.host.chars().any(char::is_whitespace) || cfg.mount.chars().any(char::is_whitespace) {
        return Err(StreamError::Invalid(
            "host and mount must not contain whitespace".into(),
        ));
    }
    if cfg.user.contains(':') {
        return Err(StreamError::Invalid("user must not contain ':'".into()));
    }
    if !cfg.mount.starts_with('/')
        || [
            &cfg.host,
            &cfg.mount,
            &cfg.user,
            &cfg.name,
            &cfg.description,
            &cfg.genre,
        ]
        .iter()
        .any(|s| s.contains(['\r', '\n']))
    {
        return Err(StreamError::Invalid("invalid source settings".into()));
    }
    let mut socket = open_transport(cfg)?;
    let auth =
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", cfg.user, cfg.password));
    write!(
        socket,
        "{method} {} HTTP/1.0\r\nHost: {}\r\nAuthorization: Basic {auth}\r\nContent-Type: {}\r\nContent-Length: {ENDLESS_BODY}\r\nIce-Name: {}\r\nIce-Description: {}\r\nIce-Genre: {}\r\nIce-Bitrate: {}\r\n\r\n",
        cfg.mount,
        cfg.host,
        cfg.format.content_type(),
        cfg.name,
        cfg.description,
        cfg.genre,
        cfg.format.kbps()
    )?;
    let mut response = Vec::new();
    while response.len() < 8192 && !response.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        match socket.read_exact(&mut byte) {
            Ok(()) => response.push(byte[0]),
            // Silence is not a refusal: a refusal arrives as a status line. A reply that
            // starts and then stalls stays an error.
            Err(error)
                if response.is_empty()
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
            {
                return Ok(Source {
                    socket,
                    replied: false,
                });
            }
            Err(error) => return Err(error.into()),
        }
    }
    let code = status_code(&response);
    if code == "405" && method == "PUT" {
        return connect(cfg, "SOURCE");
    }
    if code != "200" {
        return Err(StreamError::Invalid(format!(
            "source rejected: HTTP {code}"
        )));
    }
    Ok(Source {
        socket,
        replied: true,
    })
}
fn send(
    cfg: &SourceConfig,
    frames: &mut rtrb::Consumer<f32>,
    rx: &mpsc::Receiver<Control>,
    state: &Mutex<StreamStatus>,
    title: &mut String,
    delay: &mut u64,
    meter: &crate::LevelMeter,
) -> Result<(), StreamError> {
    // Discard audio collected during the outage before opening the source.
    let stale = frames.slots();
    for _ in 0..stale {
        let _ = frames.pop();
    }
    let (mut output, replied) = SourceOutput::new(cfg, title)?;
    let mut late = None;
    if !replied {
        // The wait for a reply is longer than a small ring holds, and listeners should not
        // hear that wait as delay.
        for _ in 0..frames.slots() {
            let _ = frames.pop();
        }
        late = Some(LateReply::start(output.socket())?);
    }
    let connected = std::time::Instant::now();
    status(
        state,
        StreamStatus::Connected {
            since: jiff::Timestamp::now(),
            last_metadata_error: None,
            dropped_system_roots: cfg.dropped_system_roots.get(),
        },
    );
    #[cfg(feature = "mp3")]
    if matches!(cfg.format, StreamFormat::Mp3 { .. }) && !title.is_empty() {
        metadata_status(state, update_mp3_title(cfg, title));
    }
    let mut samples = Vec::with_capacity(1920);
    loop {
        if connected.elapsed() >= Duration::from_secs(10) {
            *delay = 1;
        }
        match rx.try_recv() {
            Ok(Control::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Ok(Control::Title(t)) => {
                *title = t;
                output.set_title(cfg, title, &mut samples, state)?;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if let Some(reply) = &mut late
            && reply.due()
            && reply.check(output.socket())?
        {
            late = None;
        }
        if frames.slots() == frames.buffer().capacity() {
            while frames.pop().is_ok() {}
            return Err(StreamError::Invalid(
                "audio ring full; buffered audio dropped".into(),
            ));
        }
        while samples.len() < 1920 {
            match frames.pop() {
                Ok(s) => samples.push(s),
                Err(_) => break,
            }
        }
        if samples.len() == 1920 {
            meter.add(&samples);
            if let Err(error) = output.packet(&samples) {
                // A refusal that came just before the server hung up says more than the
                // failed write does.
                if let Some(reply) = &mut late {
                    reply.check(output.socket())?;
                }
                return Err(error);
            }
            samples.clear();
        } else {
            thread::sleep(Duration::from_millis(2));
        }
    }
}

enum SourceOutput {
    Opus {
        encoder: OpusEncoder,
        muxer: OggMuxer<SourceStream>,
    },
    #[cfg(feature = "mp3")]
    Mp3 {
        encoder: crate::mp3::Mp3Encoder,
        socket: SourceStream,
    },
}
impl SourceOutput {
    /// Opens the source. The flag is false when the server accepted without a reply.
    fn new(cfg: &SourceConfig, title: &str) -> Result<(Self, bool), StreamError> {
        match cfg.format {
            StreamFormat::Opus { kbps } => {
                let mut encoder = OpusEncoder::new(kbps)?;
                let Source { socket, replied } = connect(cfg, "PUT")?;
                let mut muxer = OggMuxer::new(socket);
                muxer.start(title, None, encoder.pre_skip()?)?;
                Ok((Self::Opus { encoder, muxer }, replied))
            }
            StreamFormat::Mp3 { kbps: _ } => {
                #[cfg(feature = "mp3")]
                {
                    let encoder = crate::mp3::Mp3Encoder::new(cfg.format.kbps())?;
                    let Source { socket, replied } = connect(cfg, "PUT")?;
                    Ok((Self::Mp3 { encoder, socket }, replied))
                }
                #[cfg(not(feature = "mp3"))]
                Err(StreamError::Invalid(
                    "MP3 output requires the mp3 feature".into(),
                ))
            }
        }
    }
    fn socket(&mut self) -> &mut dyn Transport {
        match self {
            Self::Opus { muxer, .. } => muxer.get_mut().as_mut(),
            #[cfg(feature = "mp3")]
            Self::Mp3 { socket, .. } => socket.as_mut(),
        }
    }
    fn packet(&mut self, samples: &[f32]) -> Result<(), StreamError> {
        match self {
            Self::Opus { encoder, muxer } => Ok(muxer.packet(encoder.encode(samples)?, false)?),
            #[cfg(feature = "mp3")]
            Self::Mp3 { encoder, socket } => {
                socket.write_all(&encoder.encode(samples)?)?;
                Ok(())
            }
        }
    }
    fn set_title(
        &mut self,
        cfg: &SourceConfig,
        title: &str,
        samples: &mut Vec<f32>,
        _state: &Mutex<StreamStatus>,
    ) -> Result<(), StreamError> {
        match self {
            Self::Opus { encoder, muxer } => {
                samples.resize(1920, 0.0);
                muxer.packet(encoder.encode(samples)?, true)?;
                samples.clear();
                *encoder = OpusEncoder::new(cfg.format.kbps())?;
                Ok(muxer.start(title, None, encoder.pre_skip()?)?)
            }
            #[cfg(feature = "mp3")]
            Self::Mp3 { .. } => {
                metadata_status(_state, update_mp3_title(cfg, title));
                Ok(())
            }
        }
    }
}

#[cfg(feature = "mp3")]
fn update_mp3_title(cfg: &SourceConfig, title: &str) -> Result<(), StreamError> {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
    let mut socket = open_transport(cfg)?;
    let auth =
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", cfg.user, cfg.password));
    write!(
        socket,
        "GET /admin/metadata?mount={}&mode=updinfo&song={} HTTP/1.0\r\nHost: {}\r\nAuthorization: Basic {auth}\r\nConnection: close\r\n\r\n",
        utf8_percent_encode(&cfg.mount, NON_ALPHANUMERIC),
        utf8_percent_encode(title, NON_ALPHANUMERIC),
        cfg.host
    )?;
    let mut response = Vec::new();
    while response.len() < 8192 && !response.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte)?;
        response.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&response);
    if text.split_whitespace().nth(1) != Some("200") {
        return Err(StreamError::Invalid("MP3 metadata update rejected".into()));
    }
    Ok(())
}

#[cfg(feature = "mp3")]
fn metadata_status(state: &Mutex<StreamStatus>, result: Result<(), StreamError>) {
    if let Ok(mut state) = state.lock()
        && let StreamStatus::Connected {
            last_metadata_error,
            ..
        } = &mut *state
    {
        *last_metadata_error = result.err().map(|error| error.to_string());
    }
}
