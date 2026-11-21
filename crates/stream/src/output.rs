use crate::StreamError;
use base64::Engine;
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

/// Encodes exactly 20 ms of 48 kHz stereo audio per packet.
pub struct OpusEncoder(opus::Encoder);
impl OpusEncoder {
    pub fn new(kbps: u32) -> Result<Self, StreamError> {
        if !(6..=510).contains(&kbps) {
            return Err(StreamError::Invalid("bitrate must be 6..510 kbps".into()));
        }
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)?;
        encoder.set_bitrate(opus::Bitrate::Bits((kbps * 1000) as i32))?;
        Ok(Self(encoder))
    }
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, StreamError> {
        if samples.len() != 1920 {
            return Err(StreamError::Invalid(
                "expected 1920 interleaved samples".into(),
            ));
        }
        let mut packet = vec![0; 4000];
        let len = self.0.encode_float(samples, &mut packet)?;
        packet.truncate(len);
        Ok(packet)
    }
    pub fn pre_skip(&mut self) -> Result<u16, StreamError> {
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
    pub fn start(
        &mut self,
        title: &str,
        artist: Option<&str>,
        pre_skip: u16,
    ) -> Result<(), StreamError> {
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
    pub fn packet(&mut self, packet: Vec<u8>, end: bool) -> Result<(), StreamError> {
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

/// Plain HTTP Icecast source settings. TLS is rejected until supported.
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
    pub kbps: u32,
    pub tls: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamStatus {
    Connecting,
    Connected { since: jiff::Timestamp },
    Retrying { in_secs: u64, last_error: String },
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
        let _ = self.control.send(Control::Title(title));
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
    pub fn start(cfg: IcecastConfig, mut frames: rtrb::Consumer<f32>) -> IcecastHandle {
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
                    let result = send(&cfg, &mut frames, &rx, &state, &mut title, &mut delay);
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
fn connect(cfg: &IcecastConfig, method: &str) -> Result<TcpStream, StreamError> {
    if cfg.tls {
        return Err(StreamError::Invalid("TLS output is unsupported".into()));
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
    let address = (cfg.host.as_str(), cfg.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| StreamError::Invalid("host has no address".into()))?;
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    let auth =
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", cfg.user, cfg.password));
    write!(
        socket,
        "{method} {} HTTP/1.0\r\nHost: {}\r\nAuthorization: Basic {auth}\r\nContent-Type: audio/ogg\r\nIce-Name: {}\r\nIce-Description: {}\r\nIce-Genre: {}\r\nIce-Bitrate: {}\r\n\r\n",
        cfg.mount, cfg.host, cfg.name, cfg.description, cfg.genre, cfg.kbps
    )?;
    let mut response = Vec::new();
    while response.len() < 8192 && !response.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte)?;
        response.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&response);
    let code = text.split_whitespace().nth(1).unwrap_or("");
    if code == "405" && method == "PUT" {
        return connect(cfg, "SOURCE");
    }
    if code != "200" {
        return Err(StreamError::Invalid(format!(
            "source rejected: HTTP {code}"
        )));
    }
    Ok(socket)
}
fn send(
    cfg: &IcecastConfig,
    frames: &mut rtrb::Consumer<f32>,
    rx: &mpsc::Receiver<Control>,
    state: &Mutex<StreamStatus>,
    title: &mut String,
    delay: &mut u64,
) -> Result<(), StreamError> {
    let mut encoder = OpusEncoder::new(cfg.kbps)?;
    let socket = connect(cfg, "PUT")?;
    let mut muxer = OggMuxer::new(socket);
    muxer.start(title, None, encoder.pre_skip()?)?;
    status(
        state,
        StreamStatus::Connected {
            since: jiff::Timestamp::now(),
        },
    );
    *delay = 1;
    let mut samples = Vec::with_capacity(1920);
    loop {
        match rx.try_recv() {
            Ok(Control::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Ok(Control::Title(t)) => {
                // End the old chain with a complete frame, then reset the codec.
                samples.resize(1920, 0.0);
                muxer.packet(encoder.encode(&samples)?, true)?;
                samples.clear();
                *title = t;
                encoder = OpusEncoder::new(cfg.kbps)?;
                muxer.start(title, None, encoder.pre_skip()?)?;
            }
            Err(mpsc::TryRecvError::Empty) => {}
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
            muxer.packet(encoder.encode(&samples)?, false)?;
            samples.clear();
        } else {
            thread::sleep(Duration::from_millis(2));
        }
    }
}
