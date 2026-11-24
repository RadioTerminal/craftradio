//! The radio app's own Icecast output and stream input against the station server: the
//! real client, not a hand-written one.

use crate::support::{PATIENCE, read_to_end_within, request, source_entry, stats, tls_files};
use radio_server::{Config, Server, ServerHandle, hash_key};
use radio_stream::{
    IcecastClient, IcecastConfig, IcecastHandle, StreamFormat, StreamInput, StreamInputHandle,
    StreamStatus,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const KEY: &str = "invented-morning-key-7Lp2";
const SLUG: &str = "morning";
/// The longest a test waits for audio to come back through the server.
const AUDIO_PATIENCE: Duration = Duration::from_secs(10);
/// Samples in one 20 ms block of 48 kHz stereo, what the app's output encodes at a time.
const BLOCK: usize = 1920;

/// The station's outputs in [`config_with_outputs`]: an MP3 one and an Opus one below the
/// app's Opus push.
const MP3_MOUNT: &str = "stream.mp3";
const LOW_MOUNT: &str = "low";

fn config() -> Config {
    station_config("")
}

fn config_with_outputs() -> Config {
    station_config(&format!(
        "[[station.output]]\nmount = \"{MP3_MOUNT}\"\nformat = \"mp3\"\nkbps = 192\n\n\
         [[station.output]]\nmount = \"{LOW_MOUNT}\"\nformat = \"opus\"\nkbps = 64\n"
    ))
}

/// One station, with `outputs` (TOML `[[station.output]]` tables) after it.
fn station_config(outputs: &str) -> Config {
    Config::parse(&format!(
        "listen = \"127.0.0.1:0\"\npublic_url = \"https://radio.example.test\"\n\n\
         [[station]]\nslug = \"{SLUG}\"\nname = \"Morning Station\"\nkey_hash = \"{}\"\n\n\
         {outputs}",
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

/// The app's own stream input, listening to `url`, and the ring it plays into.
fn app_input(url: String, extra_roots: Vec<Vec<u8>>) -> (StreamInputHandle, rtrb::Consumer<f32>) {
    let (producer, consumer) = rtrb::RingBuffer::new(48_000);
    let input = StreamInput::open_with_extra_roots(
        url,
        producer,
        Duration::from_secs(15),
        Duration::from_secs(3),
        extra_roots,
    )
    .unwrap();
    (input, consumer)
}

/// Listens to the station with the app's own stream input and checks the tone comes out.
/// The input decodes with Symphonia's codecs alone, which have no Opus, so this hears an
/// MP3 mount only; [`assert_opus_heard`] listens to an Ogg/Opus one.
fn assert_heard(url: String, extra_roots: Vec<Vec<u8>>) {
    let (input, mut consumer) = app_input(url, extra_roots);
    let heard = hears_audio(&mut consumer);
    let error = input.last_error();
    input.stop();
    assert!(heard, "no audio came through the server: {error:?}");
}

/// Listens to the station's Ogg/Opus mount `/<slug>/<mount>` over plain HTTP and decodes it
/// with the station's own codecs (`radio-codecs`, which the app's player uses), as the
/// app's stream input decodes a stream, and checks the tone comes out.
fn assert_opus_heard(addr: SocketAddr, mount: &str) {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
    use symphonia::core::meta::MetadataOptions;

    let deadline = Instant::now() + AUDIO_PATIENCE;
    // The client's read timeout fails a stream that stops, instead of hanging the test.
    // An output still starting answers 503 and says so; ask again until it plays.
    let (stream, head) = loop {
        let (mut stream, head) = request(addr, "GET", &format!("/{SLUG}/{mount}"));
        if head.status != 503 {
            assert_eq!(head.status, 200, "/{SLUG}/{mount} plays");
            break (stream, head);
        }
        let body = read_to_end_within(&mut stream, PATIENCE);
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("starting"), "/{SLUG}/{mount} refused: {body}");
        assert!(
            Instant::now() < deadline,
            "/{SLUG}/{mount} was still starting after {AUDIO_PATIENCE:?}"
        );
    };
    assert_eq!(head.header("Content-Type"), Some("audio/ogg"));

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

fn wait_for_title(addr: SocketAddr, title: &str) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let found = source_entry(&stats(addr), SLUG);
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

/// Waits until the app's stream input has read `title` from the stream's ICY metadata.
fn wait_for_icy_title(input: &StreamInputHandle, title: &str) {
    let deadline = Instant::now() + AUDIO_PATIENCE;
    loop {
        let found = input.icy_title();
        if found.as_deref() == Some(title) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the app's stream input never read the title {title:?}: {found:?}, {:?}",
            input.last_error()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn mount_url(scheme: &str, server: &ServerHandle, mount: &str) -> String {
    format!(
        "{scheme}://127.0.0.1:{}/{SLUG}/{mount}",
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
    assert_opus_heard(server.local_addr(), "stream");
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
    assert_heard(mount_url("http", &server, "stream"), Vec::new());
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
    assert_eq!(
        source_entry(&stats(addr), SLUG),
        None,
        "a refused source is not on the air"
    );
    server.stop();
}

/// The fallback route with no proxy: the app pushes over TLS to the server's own TLS, and
/// the app's stream input hears it over https. MP3, which the input decodes.
#[test]
fn the_apps_tls_output_is_heard_through_the_servers_own_tls() {
    let files = tls_files();
    let root = files.root.to_vec();
    let mut config = config();
    config.tls_cert = Some(files.cert.clone());
    config.tls_key = Some(files.key.clone());
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
    assert_heard(mount_url("https", &server, "stream"), vec![root]);
    drop(out);
    server.stop();
}

/// The app pushes Opus; its stream input, which plays no Opus, hears the station on the MP3
/// output, titles included: they reach it from the Ogg tags of the push, through ICY.
#[test]
fn the_apps_opus_push_is_heard_as_mp3_by_the_apps_stream_input() {
    let server = Server::start(config_with_outputs()).unwrap();
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
    out.handle.set_title("Cloud Signal".into());
    let (input, mut consumer) = app_input(mount_url("http", &server, MP3_MOUNT), Vec::new());
    let heard = hears_audio(&mut consumer);
    assert!(
        heard,
        "no audio came through the MP3 output: {:?}",
        input.last_error()
    );
    wait_for_icy_title(&input, "Cloud Signal");
    // A second track: the output's title follows it, not only the first one.
    out.handle.set_title("Paper Harbour".into());
    wait_for_icy_title(&input, "Paper Harbour");
    input.stop();
    drop(out);
    server.stop();
}

/// The app pushes Opus at 96 kbit/s; a listener hears it on the station's 64 kbit/s Opus
/// output.
#[test]
fn the_apps_opus_push_is_heard_on_a_lower_opus_output() {
    let server = Server::start(config_with_outputs()).unwrap();
    let addr = server.local_addr();
    let out = output(
        icecast(addr, KEY, StreamFormat::Opus { kbps: 96 }, false),
        Vec::new(),
    );
    wait_connected(&out);
    assert_opus_heard(addr, LOW_MOUNT);
    drop(out);
    server.stop();
}

#[test]
fn the_apps_mp3_push_is_heard_on_an_opus_output() {
    let server = Server::start(config_with_outputs()).unwrap();
    let addr = server.local_addr();
    let out = output(
        icecast(addr, KEY, StreamFormat::Mp3 { kbps: 128 }, false),
        Vec::new(),
    );
    wait_connected(&out);
    assert_opus_heard(addr, LOW_MOUNT);
    drop(out);
    server.stop();
}
