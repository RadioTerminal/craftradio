use crate::StreamError;
use std::{
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use symphonia::core::{
    codecs::audio::AudioDecoderOptions,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
};
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
};

/// Incremental ICY parser state. Use one state for each connection.
#[derive(Default)]
pub struct IcyState {
    audio: usize,
    remaining: usize,
    metadata: Vec<u8>,
    reading: bool,
}
/// Removes ICY metadata even when a length byte or title spans reads.
pub fn parse_icy(chunk: &[u8], metaint: usize, state: &mut IcyState) -> (Vec<u8>, Option<String>) {
    if metaint == 0 {
        return (chunk.to_vec(), None);
    }
    let mut audio = Vec::new();
    let mut title = None;
    for &byte in chunk {
        if state.reading {
            state.metadata.push(byte);
            state.remaining -= 1;
            if state.remaining == 0 {
                let text = String::from_utf8_lossy(&state.metadata);
                if let Some(value) = text
                    .split("StreamTitle='")
                    .nth(1)
                    .and_then(|s| s.split("';").next())
                {
                    title = Some(value.to_owned());
                }
                state.metadata.clear();
                state.reading = false;
                state.audio = 0;
            }
        } else if state.audio == metaint {
            state.remaining = usize::from(byte) * 16;
            state.reading = state.remaining > 0;
            if !state.reading {
                state.audio = 0;
            }
        } else {
            audio.push(byte);
            state.audio += 1;
        }
    }
    (audio, title)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamInputEvent {
    Down,
}
/// Input lifetime, latest ICY title, and timeout events.
pub struct StreamInputHandle {
    stop: Arc<AtomicBool>,
    title: Arc<Mutex<Option<String>>>,
    error: Arc<Mutex<Option<String>>>,
    events: mpsc::Receiver<StreamInputEvent>,
}
impl StreamInputHandle {
    pub fn icy_title(&self) -> Option<String> {
        self.title.lock().ok().and_then(|t| t.clone())
    }
    /// Why the last connection failed or ended, for example a certificate that the trust
    /// roots do not accept. A connection that brings audio clears it.
    pub fn last_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }
    pub fn try_event(&self) -> Option<StreamInputEvent> {
        self.events.try_recv().ok()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Drop for StreamInputHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
pub struct StreamInput;
impl StreamInput {
    /// Opens a reconnecting input with a 15 second outage timeout.
    pub fn open(
        url: impl Into<String>,
        frames: rtrb::Producer<f32>,
    ) -> Result<StreamInputHandle, StreamError> {
        Self::open_with_timeout(url, frames, Duration::from_secs(15))
    }
    pub fn open_with_timeout(
        url: impl Into<String>,
        frames: rtrb::Producer<f32>,
        timeout: Duration,
    ) -> Result<StreamInputHandle, StreamError> {
        Self::open_with_timeouts(url, frames, timeout, Duration::from_secs(10))
    }
    /// Opens an input with separate outage and audio-stall timeouts.
    pub fn open_with_timeouts(
        url: impl Into<String>,
        frames: rtrb::Producer<f32>,
        timeout: Duration,
        stall_timeout: Duration,
    ) -> Result<StreamInputHandle, StreamError> {
        Self::open_with_extra_roots(url, frames, timeout, stall_timeout, Vec::new())
    }

    /// Opens an input that also trusts the DER certificates `extra_roots`. Tests use it
    /// for a local TLS server with its own certificate.
    #[doc(hidden)]
    pub fn open_with_extra_roots(
        url: impl Into<String>,
        mut frames: rtrb::Producer<f32>,
        timeout: Duration,
        stall_timeout: Duration,
        extra_roots: Vec<Vec<u8>>,
    ) -> Result<StreamInputHandle, StreamError> {
        let url = url.into();
        let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
        if !matches!(scheme.as_deref(), Some("http" | "https")) {
            return Err(StreamError::Invalid(
                "expected an http:// or https:// URL".into(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let title = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let (tx, events) = mpsc::channel();
        let flag = stop.clone();
        let metadata = title.clone();
        let failure = error.clone();
        thread::Builder::new()
            .name("stream-input".into())
            .spawn(move || {
                // The trust roots load on this thread, so no caller waits for them.
                let tls = tls_config(&extra_roots);
                let mut last_audio = Instant::now();
                let mut down = false;
                while !flag.load(Ordering::Relaxed) {
                    let result = match &tls {
                        Ok(tls) => {
                            let link = Link {
                                url: &url,
                                tls,
                                stop: &flag,
                                title: &metadata,
                                error: &failure,
                                stall_timeout,
                            };
                            decode(&link, &mut frames, &mut last_audio, &mut down)
                        }
                        Err(e) => Err(e.clone().into()),
                    };
                    if let (Err(e), Ok(mut slot)) = (result, failure.lock()) {
                        *slot = Some(e.to_string());
                    }
                    if last_audio.elapsed() >= timeout && !down {
                        let _ = tx.send(StreamInputEvent::Down);
                        down = true;
                    }
                    for _ in 0..100 {
                        if flag.load(Ordering::Relaxed) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            })?;
        Ok(StreamInputHandle {
            stop,
            title,
            error,
            events,
        })
    }
}

/// The system trust roots as DER, loaded once per process.
static SYSTEM_ROOTS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();

/// The TLS settings of an input: the ring provider, and the system trust roots plus
/// `extra`. Certificate checks are always on.
fn tls_config(extra: &[Vec<u8>]) -> Result<ureq::tls::TlsConfig, String> {
    let system = SYSTEM_ROOTS.get_or_init(|| {
        rustls_native_certs::load_native_certs()
            .certs
            .into_iter()
            .map(|c| c.to_vec())
            .collect()
    });
    let certs: Vec<ureq::tls::Certificate<'static>> = system
        .iter()
        .chain(extra)
        .map(|der| ureq::tls::Certificate::from_der(der).to_owned())
        .collect();
    if certs.is_empty() {
        return Err("TLS: no trust roots found on this system".into());
    }
    Ok(ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::Specific(Arc::new(certs)))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build())
}

struct IcyReader {
    reader: Box<dyn Read + Send + Sync>,
    metaint: usize,
    last_audio: Instant,
    stall_timeout: Duration,
    deadline: Arc<Mutex<Option<Instant>>>,
    state: IcyState,
    pending: std::collections::VecDeque<u8>,
    title: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}
impl Read for IcyReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.pending.is_empty() {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(0);
            }
            if self.last_audio.elapsed() >= self.stall_timeout {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "audio stream stalled",
                ));
            }
            let mut bytes = [0; 4096];
            let n = self.reader.read(&mut bytes)?;
            if n == 0 {
                return Ok(0);
            }
            let (audio, title) = parse_icy(&bytes[..n], self.metaint, &mut self.state);
            if !audio.is_empty() {
                self.last_audio = Instant::now();
                if let Ok(mut deadline) = self.deadline.lock() {
                    *deadline = self.last_audio.checked_add(self.stall_timeout);
                }
            }
            self.pending.extend(audio);
            if let Some(title) = title
                && let Ok(mut target) = self.title.lock()
            {
                *target = Some(title);
            }
        }
        let n = out.len().min(self.pending.len());
        for byte in &mut out[..n] {
            if let Some(value) = self.pending.pop_front() {
                *byte = value;
            }
        }
        Ok(n)
    }
}
impl Seek for IcyReader {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::other("live stream cannot seek"))
    }
}
impl MediaSource for IcyReader {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}
/// A connection error in words. A TLS error says so, for example a certificate that the
/// trust roots do not accept.
fn describe(e: ureq::Error) -> String {
    let tls = match &e {
        ureq::Error::Rustls(t) => Some(t),
        // A handshake error reaches ureq inside an I/O error.
        ureq::Error::Io(io) => io.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()),
        _ => None,
    };
    match tls {
        Some(rustls::Error::InvalidCertificate(reason)) => {
            format!("TLS: certificate not trusted ({reason:?})")
        }
        Some(other) => format!("TLS: {other}"),
        None => e.to_string(),
    }
}

/// What each connection of one input shares.
struct Link<'a> {
    url: &'a str,
    tls: &'a ureq::tls::TlsConfig,
    stop: &'a Arc<AtomicBool>,
    title: &'a Arc<Mutex<Option<String>>>,
    error: &'a Arc<Mutex<Option<String>>>,
    stall_timeout: Duration,
}

fn decode(
    link: &Link,
    frames: &mut rtrb::Producer<f32>,
    last_audio: &mut Instant,
    down: &mut bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let Link {
        url,
        tls,
        stop,
        title,
        error,
        stall_timeout,
    } = *link;
    let deadline = Arc::new(Mutex::new(None));
    let config = ureq::Agent::config_builder()
        .timeout_global(None)
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_recv_response(Some(Duration::from_secs(2)))
        .timeout_per_read(Some(stall_timeout))
        .tls_config(tls.clone())
        .build();
    let agent = ureq::Agent::with_parts(
        config,
        DefaultConnector::default().chain(StallConnector(deadline.clone())),
        ureq::unversioned::resolver::DefaultResolver::default(),
    );
    let response = agent
        .get(url)
        .header("Icy-MetaData", "1")
        .call()
        .map_err(describe)?;
    let metaint = response
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let received = Instant::now();
    if let Ok(mut limit) = deadline.lock() {
        *limit = received.checked_add(stall_timeout);
    }
    let source = IcyReader {
        reader: Box::new(response.into_body().into_reader()),
        metaint,
        last_audio: received,
        stall_timeout,
        deadline,
        state: IcyState::default(),
        pending: Default::default(),
        title: title.clone(),
        stop: stop.clone(),
    };
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut format = symphonia::default::get_probe().probe(
        &Hint::new(),
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("no audio track")?;
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("no audio track")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;
    let mut phase = 0.0;
    let mut previous = [0.0f32; 2];
    let mut samples: Vec<f32> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        // The end of a stream is an error here, as it was before: the caller reconnects.
        let packet = format.next_packet()?.ok_or("end of stream")?;
        if packet.track_id != id {
            continue;
        }
        let decoded = decoder.decode(&packet)?;
        let spec = decoded.spec().clone();
        let channels = spec.channels().count();
        if channels == 0 || spec.rate() == 0 {
            return Err("invalid audio format".into());
        }
        decoded.copy_to_vec_interleaved(&mut samples);
        // A bounded occupancy correction compensates for independent stream clocks.
        let fill = 1.0 - frames.slots() as f64 / frames.buffer().capacity() as f64;
        let ratio = 48_000.0 / f64::from(spec.rate()) * (1.0 + (0.5 - fill) * 0.002);
        for frame in samples.chunks_exact(channels) {
            let current = [frame[0], frame[if channels > 1 { 1 } else { 0 }]];
            phase += ratio;
            while phase >= 1.0 {
                phase -= 1.0;
                let fraction = (1.0 - phase / ratio) as f32;
                let stereo = [
                    previous[0] + (current[0] - previous[0]) * fraction,
                    previous[1] + (current[1] - previous[1]) * fraction,
                ];
                if frames.slots() >= 2 {
                    let _ = frames.push(stereo[0]);
                    let _ = frames.push(stereo[1]);
                }
            }
            previous = current;
        }
        *last_audio = Instant::now();
        *down = false;
        if let Ok(mut e) = error.lock() {
            *e = None;
        }
    }
    Ok(())
}

// A body read can perform several socket reads. Bound all of them by the
// audio deadline, so metadata or partial HTTP chunks cannot keep a stall alive.

#[derive(Debug)]
struct StallConnector(Arc<Mutex<Option<Instant>>>);
impl<T: Transport> Connector<T> for StallConnector {
    type Out = StallTransport<T>;
    fn connect(
        &self,
        _: &ConnectionDetails,
        transport: Option<T>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(transport.map(|inner| StallTransport {
            inner,
            deadline: self.0.clone(),
        }))
    }
}

#[derive(Debug)]
struct StallTransport<T> {
    inner: T,
    deadline: Arc<Mutex<Option<Instant>>>,
}
impl<T: Transport> Transport for StallTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.inner.transmit_output(amount, timeout)
    }
    fn await_input(&mut self, mut timeout: NextTimeout) -> Result<bool, ureq::Error> {
        if let Ok(deadline) = self.deadline.lock()
            && let Some(deadline) = *deadline
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ureq::Error::Timeout(ureq::Timeout::PerRead));
            }
            timeout.per_read = Some(remaining.into());
        }
        self.inner.await_input(timeout)
    }
    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }
    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
