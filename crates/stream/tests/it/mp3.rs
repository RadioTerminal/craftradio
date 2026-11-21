use super::*;

fn header(socket: &mut std::net::TcpStream) -> String {
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

#[test]
fn mp3_frames_and_exact_admin_title() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut source, _) = listener.accept().unwrap();
        let request = header(&mut source);
        assert!(request.starts_with("PUT /signal HTTP/1.0\r\n"));
        assert!(request.contains("Content-Type: audio/mpeg\r\n"));
        assert!(request.contains("Authorization: Basic c291cmNlOnRlc3Qtb25seQ=="));
        source.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        let mut sync = [0; 2];
        source.read_exact(&mut sync).unwrap();
        assert_eq!(sync[0], 0xff);
        assert_eq!(sync[1] & 0xe0, 0xe0);
        tx.send(()).unwrap();
        let (mut admin, _) = listener.accept().unwrap();
        let request = header(&mut admin);
        assert!(request.starts_with("GET /admin/metadata?mount=%2Fsignal&mode=updinfo&song=Cloud%20%26%20Relay%20%2B%20%2F%20%C5%BD%3F%23%25 HTTP/1.0\r\n"));
        assert!(request.contains("Authorization: Basic c291cmNlOnRlc3Qtb25seQ=="));
        admin.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        tx.send(()).unwrap();
    });
    let (mut producer, consumer) = rtrb::RingBuffer::new(19200);
    let mut cfg = config(port);
    cfg.format = StreamFormat::Mp3 { kbps: 128 };
    let handle = IcecastClient::start(cfg, consumer);
    let feed = std::thread::spawn(move || {
        for _ in 0..150 {
            for _ in 0..960 {
                let _ = producer.push(0.1);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    rx.recv_timeout(Duration::from_secs(6)).unwrap();
    handle.set_title("Cloud & Relay + / Ž?#%".into());
    rx.recv_timeout(Duration::from_secs(6)).unwrap();
    handle.stop();
    server.join().unwrap();
    feed.join().unwrap();
}

#[test]
fn rejected_metadata_keeps_source_and_strips_title_newlines() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (ready, wait_ready) = mpsc::channel();
    let (updated, wait_updated) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut source, _) = listener.accept().unwrap();
        header(&mut source);
        source.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        let mut audio = [0; 4096];
        assert!(source.read(&mut audio).unwrap() > 0);
        ready.send(()).unwrap();
        let (mut admin, _) = listener.accept().unwrap();
        let request = header(&mut admin);
        assert!(request.contains("song=CloudRelay HTTP/1.0\r\n"));
        assert!(!request.contains("%0D"));
        assert!(!request.contains("%0A"));
        admin
            .write_all(b"HTTP/1.0 401 Unauthorized\r\n\r\n")
            .unwrap();
        // Discard audio already queued before the admin response.
        source.set_nonblocking(true).unwrap();
        while source.read(&mut audio).is_ok_and(|n| n > 0) {}
        source.set_nonblocking(false).unwrap();
        assert!(source.read(&mut audio).unwrap() > 0);
        updated.send(()).unwrap();
        wait_release.recv_timeout(Duration::from_secs(3)).unwrap();
    });
    let (mut producer, consumer) = rtrb::RingBuffer::new(19200);
    let mut cfg = config(port);
    cfg.format = StreamFormat::Mp3 { kbps: 128 };
    let handle = IcecastClient::start(cfg, consumer);
    let feed = std::thread::spawn(move || {
        for _ in 0..200 {
            for _ in 0..960 {
                let _ = producer.push(0.1);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.set_title("Cloud\r\nRelay".into());
    wait_updated.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        handle.status(),
        StreamStatus::Connected {
            last_metadata_error: Some(_),
            ..
        }
    ));
    handle.stop();
    release.send(()).unwrap();
    server.join().unwrap();
    feed.join().unwrap();
}
