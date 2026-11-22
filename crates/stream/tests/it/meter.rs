use radio_stream::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{config, request_header};

/// Pushes a steady −6 dBFS signal into the ring for `secs`.
fn feed(mut producer: rtrb::Producer<f32>, secs: f32) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for _ in 0..(secs * 100.0) as usize {
            for _ in 0..960 {
                let _ = producer.push(0.5);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

#[test]
fn icecast_meter_stays_silent_while_not_connected() {
    // A port that refuses: bind, take the port, close.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let meter = Arc::new(LevelMeter::default());
    let (producer, consumer) = rtrb::RingBuffer::new(19200);
    let handle = IcecastClient::start_metered(config(port), consumer, meter.clone());
    let feeding = feed(producer, 1.0);
    feeding.join().unwrap();
    assert_eq!(meter.take(), [LEVEL_FLOOR_DB; 2]);
    handle.stop();
}

#[test]
fn icecast_meter_follows_what_it_sends_while_connected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let _ = request_header(&mut socket);
        socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        let mut buf = [0; 4096];
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && socket.read(&mut buf).is_ok_and(|n| n > 0) {}
    });
    let meter = Arc::new(LevelMeter::default());
    let (producer, consumer) = rtrb::RingBuffer::new(19200);
    let handle = IcecastClient::start_metered(config(port), consumer, meter.clone());
    let feeding = feed(producer, 2.0);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut loudest = LEVEL_FLOOR_DB;
    while Instant::now() < deadline && loudest < -10.0 {
        std::thread::sleep(Duration::from_millis(50));
        loudest = loudest.max(meter.take()[0]);
    }
    assert!(loudest > -7.0 && loudest < -5.0, "{loudest}");
    handle.stop();
    feeding.join().unwrap();
    server.join().unwrap();
}
