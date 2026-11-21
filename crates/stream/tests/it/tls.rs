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
    let handle = IcecastClient::start(cfg, consumer);
    std::thread::sleep(Duration::from_millis(500));
    handle.stop();
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while handle.status() != StreamStatus::Stopped {
        assert!(
            std::time::Instant::now() < deadline,
            "handshake delayed stop"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    server.join().unwrap();
}
