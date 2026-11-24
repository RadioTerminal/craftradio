//! The radio app's own Icecast output and stream input against the station server: the
//! real client, not a hand-written one.

use base64::Engine;
use radio_server::{Config, Server, ServerHandle, hash_key};
use radio_stream::{
    IcecastClient, IcecastConfig, IcecastHandle, StreamFormat, StreamInput, StreamStatus,
};
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const KEY: &str = "invented-morning-key-7Lp2";
const SLUG: &str = "morning";
/// The longest a test waits for the client to connect, or for a title to show.
const PATIENCE: Duration = Duration::from_secs(5);
/// The longest a test waits for audio to come back through the server.
const AUDIO_PATIENCE: Duration = Duration::from_secs(10);
/// Samples in one 20 ms block of 48 kHz stereo, what the app's output encodes at a time.
const BLOCK: usize = 1920;

fn config() -> Config {
    Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
         [[station]]\nslug = \"{SLUG}\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\n",
        hash_key(KEY)
    ))
    .unwrap()
}

/// The app's Icecast settings for the station on `addr`.
fn icecast(addr: SocketAddr, password: &str, format: StreamFormat, tls: bool) -> IcecastConfig {
    IcecastConfig {
        host: "127.0.0.1".into(),
        port: addr.port(),
        mount: format!("/{SLUG}/stream"),
        user: "source".into(),
        password: password.into(),
        name: "Cloud Signal FM".into(),
        description: "Invented test stream".into(),
        genre: "Ambient".into(),
        format,
        tls,
    }
}

/// Feeds a 440 Hz stereo tone into the app's audio ring at the pace of real time, as the
/// mixer does. It stops when dropped, so a failing test cannot leave it running.
struct Tone {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tone {
    fn start(mut producer: rtrb::Producer<f32>) -> Tone {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut frame = 0u64;
            let mut next = Instant::now();
            while !flag.load(Ordering::Relaxed) && !producer.is_abandoned() {
                for _ in 0..BLOCK / 2 {
                    let t = frame as f32 / 48_000.0;
                    let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                    // A full ring drops the block, as the mixer's tap does.
                    let _ = producer.push(sample);
                    let _ = producer.push(sample);
                    frame += 1;
                }
                next += Duration::from_millis(20);
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
            }
        });
        Tone {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The app's Icecast output, fed a tone. Both stop when this is dropped.
struct Output {
    handle: IcecastHandle,
    _tone: Tone,
}

impl Drop for Output {
    fn drop(&mut self) {
        self.handle.stop();
    }
}

fn output(cfg: IcecastConfig, extra_roots: Vec<Vec<u8>>) -> Output {
    // Two seconds of audio, as the app's own output ring holds.
    let (producer, consumer) = rtrb::RingBuffer::new(48_000 * 2 * 2);
    let tone = Tone::start(producer);
    let handle = if extra_roots.is_empty() {
        IcecastClient::start(cfg, consumer)
    } else {
        IcecastClient::start_with_extra_roots(cfg, consumer, extra_roots)
    };
    Output {
        handle,
        _tone: tone,
    }
}

/// Waits until the output is connected. A retry on the way is allowed; the last status is
/// in the failure message.
fn wait_connected(output: &Output) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let status = output.handle.status();
        if matches!(status, StreamStatus::Connected { .. }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the app's output never connected: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Waits until `consumer` holds audio that is not silence, as the stream crate's own TLS
/// tests do.
fn hears_audio(consumer: &mut rtrb::Consumer<f32>) -> bool {
    let deadline = Instant::now() + AUDIO_PATIENCE;
    while Instant::now() < deadline {
        while let Ok(sample) = consumer.pop() {
            if sample.abs() > 0.01 {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Listens to the station with the app's own stream input and checks the tone comes out.
/// The input decodes with Symphonia's codecs alone, which have no Opus, so this hears an
/// MP3 mount only; [`assert_opus_heard`] listens to an Ogg/Opus one.
fn assert_heard(url: String, extra_roots: Vec<Vec<u8>>) {
    let (producer, mut consumer) = rtrb::RingBuffer::new(48_000);
    let input = StreamInput::open_with_extra_roots(
        url,
        producer,
        Duration::from_secs(15),
        Duration::from_secs(3),
        extra_roots,
    )
    .unwrap();
    let heard = hears_audio(&mut consumer);
    let error = input.last_error();
    input.stop();
    assert!(heard, "no audio came through the server: {error:?}");
}

/// Listens to the station's Ogg/Opus mount over plain HTTP and decodes it with the
/// station's own codecs (`radio-codecs`, which the app's player uses), as the app's stream
/// input decodes a stream, and checks the tone comes out.
fn assert_opus_heard(addr: SocketAddr) {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
    use symphonia::core::meta::MetadataOptions;

    let deadline = Instant::now() + AUDIO_PATIENCE;
    let mut stream = TcpStream::connect(addr).unwrap();
    // A stream that stops fails the read, and the test, instead of hanging it.
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "GET /{SLUG}/stream HTTP/1.1\r\nHost: radio.example.test\r\n\r\n"
    )
    .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("a reply head");
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.0 200 "), "{head}");
    assert!(head.contains("Content-Type: audio/ogg\r\n"), "{head}");

    let source = MediaSourceStream::new(Box::new(ReadOnlySource::new(stream)), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("the mount is a stream Symphonia reads");
    let track = format
        .default_track(TrackType::Audio)
        .expect("an audio track");
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .expect("audio parameters")
        .clone();
    let mut decoder = radio_codecs::registry()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .expect("the station's codecs decode the mount");
    let mut samples: Vec<f32> = Vec::new();
    while Instant::now() < deadline {
        let packet = format
            .next_packet()
            .expect("the stream goes on")
            .expect("the stream does not end");
        if packet.track_id != id {
            continue;
        }
        let decoded = decoder.decode(&packet).expect("a packet decodes");
        decoded.copy_to_vec_interleaved(&mut samples);
        if samples.iter().any(|sample| sample.abs() > 0.01) {
            return;
        }
    }
    panic!("no audio came through the server within {AUDIO_PATIENCE:?}");
}

/// The public stats document, read over plain HTTP.
fn stats(addr: SocketAddr) -> Value {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(PATIENCE)).unwrap();
    stream
        .write_all(b"GET /status-json.xsl HTTP/1.1\r\nHost: radio.example.test\r\n\r\n")
        .unwrap();
    let mut reply = Vec::new();
    match stream.read_to_end(&mut reply) {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::ConnectionReset => {}
        Err(e) => panic!("reading the stats: {e}"),
    }
    let text = String::from_utf8(reply).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.0 200 "), "{head}");
    serde_json::from_str(body).expect("the stats are JSON")
}

/// The station's entry in the stats, if it is live.
fn entry(addr: SocketAddr) -> Option<Value> {
    let path = format!("/{SLUG}/stream");
    stats(addr)["icestats"]["source"]
        .as_array()
        .expect("icestats.source is an array")
        .iter()
        .find(|source| {
            source["listenurl"]
                .as_str()
                .unwrap_or_default()
                .ends_with(&path)
        })
        .cloned()
}

fn wait_for_title(addr: SocketAddr, title: &str) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let found = entry(addr);
        if found.as_ref().and_then(|e| e["title"].as_str()) == Some(title) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the stats never showed the title {title:?}: {found:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stream_url(scheme: &str, server: &ServerHandle) -> String {
    format!(
        "{scheme}://127.0.0.1:{}/{SLUG}/stream",
        server.local_addr().port()
    )
}

#[test]
fn the_apps_opus_output_is_heard_through_the_server() {
    let server = Server::start(config()).unwrap();
    let out = output(
        icecast(
            server.local_addr(),
            KEY,
            StreamFormat::Opus { kbps: 96 },
            false,
        ),
        Vec::new(),
    );
    wait_connected(&out);
    assert_opus_heard(server.local_addr());
    drop(out);
    server.stop();
}

fn title_reaches_the_stats(format: StreamFormat) {
    let server = Server::start(config()).unwrap();
    let addr = server.local_addr();
    let out = output(icecast(addr, KEY, format, false), Vec::new());
    wait_connected(&out);
    out.handle.set_title("Cloud Signal".into());
    wait_for_title(addr, "Cloud Signal");
    // A second track: the title follows it, not only the first one.
    out.handle.set_title("Paper Harbour".into());
    wait_for_title(addr, "Paper Harbour");
    if let StreamStatus::Connected {
        last_metadata_error,
        ..
    } = out.handle.status()
    {
        assert_eq!(
            last_metadata_error, None,
            "the server took the title update"
        );
    }
    drop(out);
    server.stop();
}

#[test]
fn the_apps_title_reaches_the_stats() {
    title_reaches_the_stats(StreamFormat::Opus { kbps: 96 });
    title_reaches_the_stats(StreamFormat::Mp3 { kbps: 128 });
}

#[test]
fn the_apps_mp3_output_is_heard_through_the_server() {
    let server = Server::start(config()).unwrap();
    let out = output(
        icecast(
            server.local_addr(),
            KEY,
            StreamFormat::Mp3 { kbps: 128 },
            false,
        ),
        Vec::new(),
    );
    wait_connected(&out);
    assert_heard(stream_url("http", &server), Vec::new());
    drop(out);
    server.stop();
}

#[test]
fn the_app_reports_a_wrong_key_as_refused() {
    let server = Server::start(config()).unwrap();
    let addr = server.local_addr();
    let out = output(
        icecast(
            addr,
            "invented-wrong-key",
            StreamFormat::Opus { kbps: 96 },
            false,
        ),
        Vec::new(),
    );
    let deadline = Instant::now() + PATIENCE;
    let error = loop {
        let status = out.handle.status();
        assert!(
            !matches!(status, StreamStatus::Connected { .. }),
            "a wrong key must never look connected"
        );
        if let StreamStatus::Retrying { last_error, .. } = status {
            break last_error;
        }
        assert!(
            Instant::now() < deadline,
            "the app never reported the refusal: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    drop(out);
    assert!(error.contains("401"), "{error}");
    assert!(!error.contains("invented-wrong-key"), "{error}");
    assert_eq!(entry(addr), None, "a refused source is not on the air");
    server.stop();
}

/// DER as a PEM block, written by hand: the workspace's `rcgen` has no PEM feature.
fn pem(label: &str, der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = encoded
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        lines.join("\n")
    )
}

/// The fallback route with no proxy: the app pushes over TLS to the server's own TLS, and
/// the app's stream input hears it over https. MP3, which the input decodes.
#[test]
fn the_apps_tls_output_is_heard_through_the_servers_own_tls() {
    let made = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&cert, pem("CERTIFICATE", made.cert.der())).unwrap();
    std::fs::write(&key, pem("PRIVATE KEY", &made.signing_key.serialize_der())).unwrap();
    let root = made.cert.der().to_vec();
    let mut config = config();
    config.tls_cert = Some(cert);
    config.tls_key = Some(key);
    let server = Server::start(config).unwrap();
    let out = output(
        icecast(
            server.local_addr(),
            KEY,
            StreamFormat::Mp3 { kbps: 128 },
            true,
        ),
        vec![root.clone()],
    );
    wait_connected(&out);
    assert_heard(stream_url("https", &server), vec![root]);
    drop(out);
    server.stop();
}
