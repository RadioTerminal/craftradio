use radio_stream::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    time::Duration,
};

#[test]
fn icy_split_metadata() {
    let mut bytes = vec![b'a'; 16];
    bytes.push(2);
    let mut metadata = b"StreamTitle='Cloud Signal';".to_vec();
    metadata.resize(32, 0);
    bytes.extend(metadata);
    bytes.extend([b'b'; 16]);
    bytes.push(0);
    let mut state = IcyState::default();
    let mut audio = Vec::new();
    let mut title = None;
    for chunk in bytes.chunks(3) {
        let (part, value) = parse_icy(chunk, 16, &mut state);
        audio.extend(part);
        title = value.or(title);
    }
    assert_eq!(audio, [vec![b'a'; 16], vec![b'b'; 16]].concat());
    assert_eq!(title.as_deref(), Some("Cloud Signal"));
}
#[test]
fn sine_encodes_and_chains() {
    let mut encoder = OpusEncoder::new(96).unwrap();
    let mut bytes = Vec::new();
    let mut mux = OggMuxer::new(&mut bytes);
    mux.start(
        "Cloud Signal",
        Some("North Relay"),
        encoder.pre_skip().unwrap(),
    )
    .unwrap();
    for n in 0..50 {
        let samples: Vec<f32> = (0..960)
            .flat_map(|i| {
                let s =
                    (((n * 960 + i) as f32 * 440.0 * std::f32::consts::TAU) / 48000.0).sin() * 0.2;
                [s, s]
            })
            .collect();
        let packet = encoder.encode(&samples).unwrap();
        assert!(!packet.is_empty());
        mux.packet(packet, n == 49).unwrap();
    }
    mux.start("Second Signal", None, 0).unwrap();
    let mut reader = ogg::reading::PacketReader::new(std::io::Cursor::new(bytes));
    let mut heads = 0;
    let mut tags = 0;
    while let Some(packet) = reader.read_packet().unwrap() {
        if packet.data.starts_with(b"OpusHead") {
            heads += 1;
        }
        if packet.data.starts_with(b"OpusTags") {
            tags += 1;
        }
    }
    assert_eq!(heads, 2);
    assert_eq!(tags, 2);
}
fn config(port: u16) -> IcecastConfig {
    IcecastConfig {
        host: "127.0.0.1".into(),
        port,
        mount: "/signal".into(),
        user: "source".into(),
        password: "test-only".into(),
        name: "Cloud Radio".into(),
        description: "Test source".into(),
        genre: "Test".into(),
        format: StreamFormat::Opus { kbps: 96 },
        tls: false,
    }
}
#[test]
fn source_headers_title_and_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(6)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).unwrap();
            assert!(header.starts_with("PUT "));
            assert!(header.contains("Authorization: Basic c291cmNlOnRlc3Qtb25seQ=="));
            assert!(header.contains("Content-Type: audio/ogg"));
            socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
            let mut received = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                received.extend(&buf[..n]);
                if received.windows(18).any(|w| w == b"TITLE=Exact Signal") {
                    break;
                }
            }
            assert!(received.starts_with(b"OggS"));
            assert!(received.windows(8).any(|w| w == b"OpusTags"));
            tx.send(attempt).unwrap();
        }
    });
    let (mut producer, consumer) = rtrb::RingBuffer::new(19200);
    let handle = IcecastClient::start(config(port), consumer);
    handle.set_title("Exact Signal".into());
    let feed = std::thread::spawn(move || {
        for _ in 0..400 {
            for _ in 0..960 {
                let _ = producer.push(0.0);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(8)).unwrap(), 0);
    assert_eq!(rx.recv_timeout(Duration::from_secs(8)).unwrap(), 1);
    handle.stop();
    feed.join().unwrap();
    server.join().unwrap();
}
#[test]
fn input_retries_and_reports_down() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (producer, _) = rtrb::RingBuffer::new(4096);
    let input = StreamInput::open_with_timeout(
        format!("http://127.0.0.1:{port}/stream"),
        producer,
        Duration::from_millis(20),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    while input.try_event().is_none() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn falls_back_to_source() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for method in ["PUT", "SOURCE"] {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            assert!(String::from_utf8(header).unwrap().starts_with(method));
            if method == "PUT" {
                socket
                    .write_all(b"HTTP/1.0 405 Method Not Allowed\r\n\r\n")
                    .unwrap();
            } else {
                socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
                let mut magic = [0; 4];
                socket.read_exact(&mut magic).unwrap();
                assert_eq!(&magic, b"OggS");
                tx.send(()).unwrap();
            }
        }
    });
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start(config(port), consumer);
    rx.recv_timeout(Duration::from_secs(6)).unwrap();
    handle.stop();
    server.join().unwrap();
}

#[test]
fn input_decodes_icy_wrapped_wav_to_stereo() {
    // Generate a short mono PCM WAV in memory, with a different sample rate.
    let samples = 2400u32;
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + samples * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(24000u32.to_le_bytes());
    wav.extend(48000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples * 2).to_le_bytes());
    for i in 0..samples {
        wav.extend((((i as f32 * 0.1).sin() * 4000.0) as i16).to_le_bytes());
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        assert!(
            String::from_utf8(request)
                .unwrap()
                .to_lowercase()
                .contains("icy-metadata: 1")
        );
        socket
            .write_all(b"HTTP/1.0 200 OK\r\nicy-metaint: 256\r\n\r\n")
            .unwrap();
        for chunk in wav.chunks(256) {
            socket.write_all(chunk).unwrap();
            if chunk.len() == 256 {
                socket.write_all(&[2]).unwrap();
                let mut title = b"StreamTitle='Cloud Input';".to_vec();
                title.resize(32, 0);
                socket.write_all(&title).unwrap();
            }
        }
    });
    let (producer, mut consumer) = rtrb::RingBuffer::new(24000);
    let input = StreamInput::open(format!("http://127.0.0.1:{port}/audio"), producer).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while consumer.slots() < 8000 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(input.icy_title().as_deref(), Some("Cloud Input"));
    let mut nonzero = false;
    while consumer.slots() >= 2 {
        let left = consumer.pop().unwrap();
        let right = consumer.pop().unwrap();
        assert_eq!(left, right);
        nonzero |= left.abs() > 0.01;
    }
    assert!(nonzero);
    input.stop();
    server.join().unwrap();
}

#[cfg(feature = "mp3")]
mod mp3;

fn request_header(socket: &mut std::net::TcpStream) -> String {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).unwrap()
}

fn live_wav_header() -> Vec<u8> {
    let mut wav = b"RIFF".to_vec();
    wav.extend(1_000_036u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(24000u32.to_le_bytes());
    wav.extend(48000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(1_000_000u32.to_le_bytes());
    wav
}

#[test]
fn input_keeps_receiving_slow_live_audio() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        request_header(&mut socket);
        socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        socket.write_all(&live_wav_header()).unwrap();
        let start = std::time::Instant::now();
        for _ in 0..24 {
            socket.write_all(&[1; 4096]).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(start.elapsed() > Duration::from_secs(4));
    });
    let (producer, mut consumer) = rtrb::RingBuffer::new(24000);
    let input = StreamInput::open(format!("http://127.0.0.1:{port}/live"), producer).unwrap();
    let start = std::time::Instant::now();
    let mut last_received = start;
    while start.elapsed() < Duration::from_millis(4500) {
        if consumer.pop().is_ok() {
            last_received = std::time::Instant::now();
            while consumer.pop().is_ok() {}
        }
        assert!(input.try_event().is_none());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(last_received.duration_since(start) > Duration::from_secs(4));
    server.join().unwrap();
    input.stop();
}

#[test]
fn input_reconnects_after_audio_stalls() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        request_header(&mut first);
        first.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        first.write_all(&live_wav_header()).unwrap();
        first.write_all(&[1; 8192]).unwrap();
        let (mut next, _) = listener.accept().unwrap();
        request_header(&mut next);
        tx.send(()).unwrap();
    });
    let (producer, _) = rtrb::RingBuffer::new(24000);
    let input = StreamInput::open_with_timeouts(
        format!("http://127.0.0.1:{port}/live"),
        producer,
        Duration::from_millis(100),
        Duration::from_millis(300),
    )
    .unwrap();
    rx.recv_timeout(Duration::from_secs(4)).unwrap();
    input.stop();
    server.join().unwrap();
}

#[test]
fn source_discards_stale_ring_before_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        request_header(&mut socket);
        socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        let mut magic = [0; 4];
        socket.read_exact(&mut magic).unwrap();
        assert_eq!(&magic, b"OggS");
        tx.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
    });
    let (mut producer, consumer) = rtrb::RingBuffer::new(4096);
    while producer.push(0.0).is_ok() {}
    let handle = IcecastClient::start(config(port), consumer);
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(matches!(handle.status(), StreamStatus::Connected { .. }));
    handle.stop();
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn invalid_source_settings_report_clear_errors() {
    for (field, value, expected) in [
        ("host", "bad host", "whitespace"),
        ("mount", "/bad\tmount", "whitespace"),
        ("user", "bad:user", "user"),
    ] {
        let mut cfg = config(1);
        match field {
            "host" => cfg.host = value.into(),
            "mount" => cfg.mount = value.into(),
            _ => cfg.user = value.into(),
        }
        let (_producer, consumer) = rtrb::RingBuffer::new(4096);
        let handle = IcecastClient::start(cfg, consumer);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let StreamStatus::Retrying { last_error, .. } = handle.status() {
                assert!(last_error.contains(expected), "{last_error}");
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        handle.stop();
    }
}

#[test]
fn source_rejection_does_not_copy_status_text() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        request_header(&mut socket);
        socket
            .write_all(b"HTTP/1.0 private-text Rejected\r\n\r\n")
            .unwrap();
    });
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start(config(port), consumer);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let StreamStatus::Retrying { last_error, .. } = handle.status() {
            assert!(!last_error.contains("private-text"));
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    handle.stop();
    server.join().unwrap();
}

#[test]
fn short_source_connections_keep_increasing_backoff() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (ready, wait_ready) = mpsc::channel();
    let (resume, wait_resume) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            request_header(&mut socket);
            ready.send(()).unwrap();
            wait_resume.recv_timeout(Duration::from_secs(3)).unwrap();
            socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
            let mut bytes = [0; 4096];
            while socket.read(&mut bytes).is_ok_and(|n| n > 0) {}
        }
    });
    let (mut producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start(config(port), consumer);
    for expected_delay in [1, 2] {
        wait_ready.recv_timeout(Duration::from_secs(4)).unwrap();
        // Fill after the stale-audio drain, while the handshake is pending.
        while producer.push(0.0).is_ok() {}
        resume.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let StreamStatus::Retrying {
                in_secs,
                last_error,
            } = handle.status()
            {
                assert!(last_error.contains("ring full"));
                assert_eq!(in_secs, expected_delay);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    handle.stop();
    server.join().unwrap();
}

mod tls;
