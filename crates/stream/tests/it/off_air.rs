use radio_stream::*;
use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use crate::{config, request_header};

/// How long a test waits for something the source should do soon.
const PATIENCE: Duration = Duration::from_secs(4);

fn wait_for(handle: &IcecastHandle, what: &str, wanted: impl Fn(&StreamStatus) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let status = handle.status();
        if wanted(&status) {
            return;
        }
        assert!(Instant::now() < deadline, "waiting for {what}: {status:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Accepts a source and says 200 to it.
fn accept_source(listener: &TcpListener) -> TcpStream {
    let (mut socket, _) = listener.accept().unwrap();
    request_header(&mut socket);
    socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
    socket
}

/// True when the source closed the connection; false when it stayed past the read timeout.
fn source_leaves(socket: &mut TcpStream) -> bool {
    let mut bytes = [0; 4096];
    loop {
        match socket.read(&mut bytes) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return false;
            }
            Err(_) => return true,
        }
    }
}

#[test]
fn off_air_closes_the_source_and_on_air_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (events, event) = mpsc::channel();
    let (release, wait_release) = mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let mut first = accept_source(&listener);
        first.set_read_timeout(Some(PATIENCE)).unwrap();
        events.send("connected").unwrap();
        events
            .send(if source_leaves(&mut first) {
                "left"
            } else {
                "stayed"
            })
            .unwrap();
        let _again = accept_source(&listener);
        events.send("connected").unwrap();
        let _ = wait_release.recv_timeout(PATIENCE);
    });
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered(config(port), consumer, Arc::default());
    assert_eq!(event.recv_timeout(PATIENCE).unwrap(), "connected");
    wait_for(&handle, "connected", |s| {
        matches!(s, StreamStatus::Connected { .. })
    });

    handle.set_on_air(false);
    assert_eq!(event.recv_timeout(PATIENCE).unwrap(), "left");
    wait_for(&handle, "off air", |s| *s == StreamStatus::OffAir);

    handle.set_on_air(true);
    assert_eq!(event.recv_timeout(PATIENCE).unwrap(), "connected");
    wait_for(&handle, "connected again", |s| {
        matches!(s, StreamStatus::Connected { .. })
    });
    handle.stop();
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn starting_off_air_never_dials() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered_off_air(config(port), consumer, Arc::default());
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        listener.accept().map(|_| ()).map_err(|e| e.kind()),
        Err(ErrorKind::WouldBlock),
        "an off-air source dialled the server"
    );
    assert_eq!(handle.status(), StreamStatus::OffAir);
    handle.stop();
}

#[test]
fn on_then_off_at_once_never_dials() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered_off_air(config(port), consumer, Arc::default());
    handle.set_on_air(true);
    handle.set_on_air(false);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        listener.accept().map(|_| ()).map_err(|e| e.kind()),
        Err(ErrorKind::WouldBlock),
        "a switch that ended off air dialled the server"
    );
    assert_eq!(handle.status(), StreamStatus::OffAir);
    handle.stop();
}

#[test]
fn off_air_while_retrying_stops_retrying() {
    // A port that refuses: bind, take the port, close.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered(config(port), consumer, Arc::default());
    wait_for(&handle, "retrying", |s| {
        matches!(s, StreamStatus::Retrying { .. })
    });
    let asked = Instant::now();
    handle.set_on_air(false);
    wait_for(&handle, "off air", |s| *s == StreamStatus::OffAir);
    assert!(
        asked.elapsed() < Duration::from_millis(500),
        "off air waited for the retry delay: {:?}",
        asked.elapsed()
    );

    // A server comes up now: an off-air source must not reach it, however long it waits.
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::sleep(Duration::from_millis(3500));
    assert_eq!(
        listener.accept().map(|_| ()).map_err(|e| e.kind()),
        Err(ErrorKind::WouldBlock),
        "an off-air source kept retrying"
    );

    listener.set_nonblocking(false).unwrap();
    handle.set_on_air(true);
    let (connected, wait_connected) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let _socket = accept_source(&listener);
        connected.send(()).unwrap();
    });
    wait_connected.recv_timeout(PATIENCE).unwrap();
    handle.stop();
    server.join().unwrap();
}

/// The peak of each decoded Opus packet in an Ogg stream, after the stream headers.
fn packet_peaks(ogg_bytes: Vec<u8>) -> Vec<f32> {
    let mut reader = ogg::reading::PacketReader::new(std::io::Cursor::new(ogg_bytes));
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Stereo).unwrap();
    let mut pcm = vec![0.0f32; 5760 * 2];
    let mut peaks = Vec::new();
    while let Ok(Some(packet)) = reader.read_packet() {
        if packet.data.starts_with(b"OpusHead") || packet.data.starts_with(b"OpusTags") {
            continue;
        }
        let frames = decoder.decode_float(&packet.data, &mut pcm, false).unwrap();
        peaks.push(pcm[..frames * 2].iter().fold(0.0f32, |p, s| p.max(s.abs())));
    }
    peaks
}

#[test]
fn off_air_drains_the_tap() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (heard, wait_heard) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut socket = accept_source(&listener);
        // About the first second of a 96 kbit/s stream.
        let mut bytes = vec![0; 12_000];
        socket.read_exact(&mut bytes).unwrap();
        heard.send(packet_peaks(bytes)).unwrap();
    });
    // Half a second of ring, so two seconds off air would fill it four times over.
    let (mut producer, consumer) = rtrb::RingBuffer::new(48_000);
    let handle = IcecastClient::start_metered_off_air(config(port), consumer, Arc::default());

    let mut refused = 0;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        for _ in 0..960 {
            refused += usize::from(producer.push(0.0).is_err());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(refused, 0, "the tap filled up while off air");

    handle.set_on_air(true);
    // A marker tone, written only after going on air.
    let mut n = 0u64;
    let peaks = loop {
        for _ in 0..480 {
            let s = (n as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            let _ = producer.push(s);
            let _ = producer.push(s);
            n += 1;
        }
        if let Ok(peaks) = wait_heard.try_recv() {
            break peaks;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no audio arrived"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        peaks.len() > 20,
        "too little audio decoded: {}",
        peaks.len()
    );
    let silent = peaks.iter().filter(|&&peak| peak < 0.05).count();
    assert!(
        silent <= 1,
        "{silent} of the first {} packets are the silence from off air",
        peaks.len()
    );
    handle.stop();
    server.join().unwrap();
}
