use super::*;
use std::sync::Arc;

#[test]
fn tls_defaults_exclude_only_loopback_hosts() {
    for host in [
        "localhost",
        "LOCALHOST",
        "127.0.0.1",
        "127.42.3.4",
        "127.255.255.255",
        "::1",
    ] {
        assert!(!default_tls(host), "{host}");
    }
    for host in [
        "radio.example",
        "localhost.example",
        "128.0.0.1",
        "126.255.255.255",
        "192.168.1.1",
        "::",
        "2001:db8::1",
    ] {
        assert!(default_tls(host), "{host}");
    }
}

fn server_config() -> (Arc<rustls::ServerConfig>, Vec<u8>) {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let der = cert.cert.der().to_vec();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
    )
    .unwrap();
    (Arc::new(config), der)
}

fn accept(
    listener: &TcpListener,
    config: Arc<rustls::ServerConfig>,
) -> rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream> {
    let (socket, _) = listener.accept().unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    rustls::StreamOwned::new(rustls::ServerConnection::new(config).unwrap(), socket)
}

fn tls_header(stream: &mut impl Read) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 8192);
    }
    String::from_utf8(bytes).unwrap()
}

#[test]
fn trusted_tls_source_and_fallback_send_authenticated_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(listener.local_addr().unwrap().port());
    cfg.tls = true;
    let (tls, root) = server_config();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for method in ["PUT", "SOURCE"] {
            let mut stream = accept(&listener, tls.clone());
            let request = tls_header(&mut stream);
            assert!(request.starts_with(&format!("{method} /signal HTTP/1.0\r\n")));
            assert!(request.contains("Authorization: Basic c291cmNlOnRlc3Qtb25seQ=="));
            if method == "PUT" {
                stream
                    .write_all(b"HTTP/1.0 405 Method Not Allowed\r\n\r\n")
                    .unwrap();
                stream.flush().unwrap();
            } else {
                stream.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
                stream.flush().unwrap();
                let mut magic = [0; 4];
                stream.read_exact(&mut magic).unwrap();
                assert_eq!(&magic, b"OggS");
            }
        }
        tx.send(()).unwrap();
    });
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_with_extra_roots(cfg, consumer, vec![root]);
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.stop();
    server.join().unwrap();
}

#[test]
fn untrusted_tls_reports_error_without_sending_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(listener.local_addr().unwrap().port());
    cfg.tls = true;
    let password = cfg.password.clone();
    let (tls, _) = server_config();
    let server = std::thread::spawn(move || {
        let mut stream = accept(&listener, tls);
        let mut application_data = Vec::new();
        let mut bytes = [0; 4096];
        loop {
            match stream.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(count) => application_data.extend_from_slice(&bytes[..count]),
            }
        }
        while let Ok(count) = stream.conn.reader().read(&mut bytes) {
            if count == 0 {
                break;
            }
            application_data.extend_from_slice(&bytes[..count]);
        }
        application_data
    });
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start(cfg, consumer);
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    loop {
        if let StreamStatus::Retrying {
            last_error,
            in_secs,
        } = handle.status()
        {
            assert_eq!(last_error, "TLS: certificate not trusted for 127.0.0.1");
            assert!(!last_error.contains(&password));
            assert_eq!(in_secs, 1);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    handle.stop();
    assert!(server.join().unwrap().is_empty());
}

#[cfg(feature = "mp3")]
#[test]
fn mp3_admin_request_uses_verified_tls() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(listener.local_addr().unwrap().port());
    cfg.tls = true;
    cfg.format = StreamFormat::Mp3 { kbps: 128 };
    let (tls, root) = server_config();
    let (tx, rx) = mpsc::channel();
    let server =
        std::thread::spawn(move || {
            let mut source = accept(&listener, tls.clone());
            assert!(tls_header(&mut source).starts_with("PUT "));
            source.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
            source.flush().unwrap();
            tx.send(()).unwrap();
            let mut admin = accept(&listener, tls);
            let request = tls_header(&mut admin);
            assert!(request.starts_with(
                "GET /admin/metadata?mount=%2Fsignal&mode=updinfo&song=Cloud%20Relay "
            ));
            assert!(request.contains("Authorization: Basic c291cmNlOnRlc3Qtb25seQ=="));
            admin.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
            admin.flush().unwrap();
            tx.send(()).unwrap();
        });
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_with_extra_roots(cfg, consumer, vec![root]);
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.set_title("Cloud Relay".into());
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.stop();
    server.join().unwrap();
}

#[test]
fn slow_tls_handshake_has_total_deadline_and_stop_finishes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(listener.local_addr().unwrap().port());
    cfg.tls = true;
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        // An incomplete TLS record, delivered faster than the read timeout.
        for byte in [22, 3, 3, 0, 100].into_iter().chain(std::iter::repeat(0)) {
            if socket.write_all(&[byte]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let started = std::time::Instant::now();
    let handle = IcecastClient::start(cfg, consumer);
    loop {
        if let StreamStatus::Retrying { last_error, .. } = handle.status() {
            assert!(last_error.contains("handshake timed out"), "{last_error}");
            assert!(
                (Duration::from_millis(4500)..=Duration::from_secs(6)).contains(&started.elapsed())
            );
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "handshake did not time out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    handle.stop();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while handle.status() != StreamStatus::Stopped {
        assert!(
            std::time::Instant::now() < deadline,
            "handshake delayed stop"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    server.join().unwrap();
}

/// Answers one GET on `stream` with live audio: a WAV header, then a tone. It keeps the
/// connection open until `done` says so.
fn serve_audio(stream: &mut (impl Read + Write), done: mpsc::Receiver<()>) {
    let request = tls_header(stream);
    assert!(request.starts_with("GET /live HTTP/1.1\r\n"), "{request}");
    stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
    stream.write_all(&live_wav_header()).unwrap();
    let mut tone = Vec::new();
    for i in 0..24_000u32 {
        tone.extend((((i as f32 * 0.1).sin() * 4000.0) as i16).to_le_bytes());
    }
    for chunk in tone.chunks(4096) {
        if stream
            .write_all(chunk)
            .and_then(|_| stream.flush())
            .is_err()
        {
            break;
        }
    }
    let _ = done.recv_timeout(Duration::from_secs(10));
}

/// Waits until `consumer` holds audio that is not silence.
fn hears_audio(consumer: &mut rtrb::Consumer<f32>) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        while let Ok(x) = consumer.pop() {
            if x.abs() > 0.01 {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn input_plays_an_https_stream_from_a_trusted_server() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tls, root) = server_config();
    let (done, wait) = mpsc::channel();
    let server = std::thread::spawn(move || serve_audio(&mut accept(&listener, tls), wait));
    let (producer, mut consumer) = rtrb::RingBuffer::new(48_000);
    let input = StreamInput::open_with_extra_roots(
        format!("https://127.0.0.1:{port}/live"),
        producer,
        Duration::from_secs(15),
        Duration::from_secs(3),
        vec![root],
    )
    .unwrap();
    assert!(hears_audio(&mut consumer), "the https stream plays");
    assert_eq!(input.last_error(), None);
    done.send(()).unwrap();
    input.stop();
    server.join().unwrap();
}

#[test]
fn input_follows_a_redirect_from_http_to_https() {
    let secure = TcpListener::bind("127.0.0.1:0").unwrap();
    let secure_port = secure.local_addr().unwrap().port();
    let plain = TcpListener::bind("127.0.0.1:0").unwrap();
    let plain_port = plain.local_addr().unwrap().port();
    let (tls, root) = server_config();
    let redirect = std::thread::spawn(move || {
        let (mut socket, _) = plain.accept().unwrap();
        request_header(&mut socket);
        let answer = format!(
            "HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{secure_port}/live\r\n\
             Content-Length: 0\r\n\r\n"
        );
        socket.write_all(answer.as_bytes()).unwrap();
    });
    let (done, wait) = mpsc::channel();
    let server = std::thread::spawn(move || serve_audio(&mut accept(&secure, tls), wait));
    let (producer, mut consumer) = rtrb::RingBuffer::new(48_000);
    let input = StreamInput::open_with_extra_roots(
        format!("http://127.0.0.1:{plain_port}/live"),
        producer,
        Duration::from_secs(15),
        Duration::from_secs(3),
        vec![root],
    )
    .unwrap();
    assert!(hears_audio(&mut consumer), "the redirected stream plays");
    done.send(()).unwrap();
    input.stop();
    redirect.join().unwrap();
    server.join().unwrap();
}

#[test]
fn input_refuses_an_untrusted_certificate_and_says_why() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tls, _) = server_config();
    let server = std::thread::spawn(move || {
        // The client stops in the handshake, so no request reaches the server.
        let mut stream = accept(&listener, tls);
        let mut bytes = [0; 4096];
        !matches!(stream.read(&mut bytes), Ok(n) if n > 0)
    });
    let (producer, mut consumer) = rtrb::RingBuffer::new(48_000);
    // No extra root: the self-signed certificate is not trusted.
    let input = StreamInput::open_with_timeouts(
        format!("https://127.0.0.1:{port}/live"),
        producer,
        Duration::from_secs(15),
        Duration::from_secs(3),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let error = loop {
        if let Some(e) = input.last_error() {
            break e;
        }
        assert!(std::time::Instant::now() < deadline, "no error reported");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(error.starts_with("TLS: certificate not trusted"), "{error}");
    assert!(consumer.pop().is_err(), "no audio from an untrusted server");
    input.stop();
    assert!(server.join().unwrap(), "no request was sent");
}

#[test]
fn input_refuses_a_url_that_is_not_http() {
    let (producer, _consumer) = rtrb::RingBuffer::new(16);
    let result = StreamInput::open("ftp://radio.example/live", producer);
    assert!(result.is_err());
}
