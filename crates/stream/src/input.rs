use crate::StreamError;
use std::{
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    formats::FormatOptions,
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
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
    events: mpsc::Receiver<StreamInputEvent>,
}
impl StreamInputHandle {
    pub fn icy_title(&self) -> Option<String> {
        self.title.lock().ok().and_then(|t| t.clone())
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
        mut frames: rtrb::Producer<f32>,
        timeout: Duration,
    ) -> Result<StreamInputHandle, StreamError> {
        let url = url.into();
        if !url.starts_with("http://") {
            return Err(StreamError::Invalid(
                "expected plain HTTP URL (TLS input is unsupported)".into(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let title = Arc::new(Mutex::new(None));
        let (tx, events) = mpsc::channel();
        let flag = stop.clone();
        let metadata = title.clone();
        thread::Builder::new()
            .name("stream-input".into())
            .spawn(move || {
                let mut last_audio = Instant::now();
                let mut down = false;
                while !flag.load(Ordering::Relaxed) {
                    let _ = decode(
                        &url,
                        &mut frames,
                        &flag,
                        &metadata,
                        &mut last_audio,
                        &mut down,
                    );
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
            events,
        })
    }
}
struct IcyReader {
    reader: Box<dyn Read + Send + Sync>,
    metaint: usize,
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
            let mut bytes = [0; 4096];
            let n = self.reader.read(&mut bytes)?;
            if n == 0 {
                return Ok(0);
            }
            let (audio, title) = parse_icy(&bytes[..n], self.metaint, &mut self.state);
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
fn decode(
    url: &str,
    frames: &mut rtrb::Producer<f32>,
    stop: &Arc<AtomicBool>,
    title: &Arc<Mutex<Option<String>>>,
    last_audio: &mut Instant,
    down: &mut bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_recv_body(Some(Duration::from_secs(2)))
        .build()
        .into();
    let response = agent.get(url).header("Icy-MetaData", "1").call()?;
    let metaint = response
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let source = IcyReader {
        reader: Box::new(response.into_body().into_reader()),
        metaint,
        state: IcyState::default(),
        pending: Default::default(),
        title: title.clone(),
        stop: stop.clone(),
    };
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut format = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?
        .format;
    let track = format.default_track().ok_or("no audio track")?;
    let id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut phase = 0.0;
    let mut previous = [0.0f32; 2];
    while !stop.load(Ordering::Relaxed) {
        let packet = format.next_packet()?;
        if packet.track_id() != id {
            continue;
        }
        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if channels == 0 || spec.rate == 0 {
            return Err("invalid audio format".into());
        }
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        // A bounded occupancy correction compensates for independent stream clocks.
        let fill = 1.0 - frames.slots() as f64 / frames.buffer().capacity() as f64;
        let ratio = 48_000.0 / f64::from(spec.rate) * (1.0 + (0.5 - fill) * 0.002);
        for frame in buffer.samples().chunks_exact(channels) {
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
    }
    Ok(())
}
