//! New Icecast settings while the source runs: it moves, closes or waits as told.

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

/// What a test server saw of a source.
#[derive(Debug, PartialEq)]
enum Seen {
    /// The request line and mount of a source it accepted.
    Arrived(String),
    Left,
}

/// A server that accepts each source, says 200, reads it until it leaves and reports both.
fn server() -> (u16, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mut socket in listener.incoming().flatten() {
            let header = request_header(&mut socket);
            let line = header.lines().next().unwrap_or_default().to_owned();
            if socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").is_err() {
                continue;
            }
            if tx.send(Seen::Arrived(line)).is_err() {
                return;
            }
            socket.set_read_timeout(None).unwrap();
            let mut bytes = [0; 4096];
            while matches!(socket.read(&mut bytes), Ok(n) if n > 0) {}
            if tx.send(Seen::Left).is_err() {
                return;
            }
        }
    });
    (port, rx)
}

/// A port where a server listens but that nobody accepts on, to see whether a source dials.
fn silent_listener() -> (u16, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    (listener.local_addr().unwrap().port(), listener)
}

fn dialled(listener: &TcpListener) -> bool {
    !matches!(
        listener
            .accept()
            .map(|_: (TcpStream, _)| ())
            .map_err(|e| e.kind()),
        Err(ErrorKind::WouldBlock)
    )
}

#[test]
fn reconfigure_moves_the_source_to_the_new_mount() {
    let (port_a, seen_a) = server();
    let (port_b, seen_b) = server();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered(config(port_a), consumer, Arc::default());
    assert!(matches!(
        seen_a.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));

    let mut moved = config(port_b);
    moved.mount = "/moved".into();
    handle.reconfigure(Some(moved));
    assert_eq!(seen_a.recv_timeout(PATIENCE).unwrap(), Seen::Left);
    match seen_b.recv_timeout(PATIENCE).unwrap() {
        Seen::Arrived(line) => assert!(line.contains(" /moved "), "the new mount: {line}"),
        other => panic!("server B saw {other:?}"),
    }
    wait_for(&handle, "connected to B", |s| {
        matches!(s, StreamStatus::Connected { .. })
    });
    handle.stop();
}

#[test]
fn reconfigure_to_none_closes_the_source() {
    let (port, seen) = server();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered(config(port), consumer, Arc::default());
    assert!(matches!(
        seen.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));

    handle.reconfigure(None);
    assert_eq!(seen.recv_timeout(PATIENCE).unwrap(), Seen::Left);
    wait_for(&handle, "stopped", |s| *s == StreamStatus::Stopped);
    // It does not come back by itself.
    assert!(seen.recv_timeout(Duration::from_millis(1500)).is_err());
    assert_eq!(handle.status(), StreamStatus::Stopped);
    handle.stop();
}

#[test]
fn a_source_started_without_settings_waits_and_connects_once_given_them() {
    let (port, seen) = server();
    // A tap that fills in a fraction of a second, to see that the waiting source drains it.
    let (mut producer, consumer) = rtrb::RingBuffer::new(9600);
    let handle = IcecastClient::start_metered_with(None, true, consumer, Arc::default());
    assert_eq!(handle.status(), StreamStatus::Stopped);
    let mut refused = 0;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        for _ in 0..960 {
            refused += usize::from(producer.push(0.0).is_err());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(refused, 0, "the tap filled up while the source waited");
    assert_eq!(handle.status(), StreamStatus::Stopped);

    handle.reconfigure(Some(config(port)));
    assert!(matches!(
        seen.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));
    wait_for(&handle, "connected", |s| {
        matches!(s, StreamStatus::Connected { .. })
    });
    handle.stop();
}

#[test]
fn off_air_still_wins_after_reconfigure() {
    // Off air with no settings: new settings do not dial.
    let (port, listener) = silent_listener();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered_with(None, false, consumer, Arc::default());
    handle.reconfigure(Some(config(port)));
    std::thread::sleep(Duration::from_secs(1));
    assert!(!dialled(&listener), "new settings dialled while off air");
    assert_eq!(handle.status(), StreamStatus::OffAir);
    handle.stop();

    // On air and connected: new settings and off air sent at once never reach the new server.
    let (port_a, seen_a) = server();
    let (port_b, listener_b) = silent_listener();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered(config(port_a), consumer, Arc::default());
    assert!(matches!(
        seen_a.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));
    handle.reconfigure(Some(config(port_b)));
    handle.set_on_air(false);
    assert_eq!(seen_a.recv_timeout(PATIENCE).unwrap(), Seen::Left);
    wait_for(&handle, "off air", |s| *s == StreamStatus::OffAir);
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !dialled(&listener_b),
        "off air sent after the settings dialled"
    );
    handle.stop();
}

#[test]
fn the_same_settings_again_keep_the_connection() {
    let (port, seen) = server();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered(config(port), consumer, Arc::default());
    assert!(matches!(
        seen.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));
    handle.reconfigure(Some(config(port)));
    assert!(
        seen.recv_timeout(Duration::from_millis(1500)).is_err(),
        "the same settings closed the connection"
    );
    handle.stop();
}

#[test]
fn new_settings_while_retrying_dial_at_once() {
    // A port that refuses: bind, take the port, close.
    let refused = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (port, seen) = server();
    let (_producer, consumer) = rtrb::RingBuffer::new(4096);
    let handle = IcecastClient::start_metered(config(refused), consumer, Arc::default());
    wait_for(&handle, "retrying", |s| {
        matches!(s, StreamStatus::Retrying { .. })
    });
    // Let the back-off grow past what the test waits for the new server.
    std::thread::sleep(Duration::from_millis(3500));
    let asked = Instant::now();
    handle.reconfigure(Some(config(port)));
    assert!(matches!(
        seen.recv_timeout(PATIENCE).unwrap(),
        Seen::Arrived(_)
    ));
    assert!(
        asked.elapsed() < Duration::from_millis(1500),
        "new settings waited for the retry delay: {:?}",
        asked.elapsed()
    );
    handle.stop();
}
