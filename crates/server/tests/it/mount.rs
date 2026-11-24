use crate::framing::{OggSource, granule, is_bos, pages};
use radio_server::{Format, ListenError, Listener, ListenerCap, Mount, Received};
use radio_stream::OggMuxer;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_millis(200);

/// Everything a listener has queued, and whether the mount ended it.
fn drain(listener: &Listener) -> (Vec<Vec<u8>>, bool) {
    let mut chunks = Vec::new();
    loop {
        match listener.recv(WAIT) {
            Received::Chunk(chunk) => chunks.push(chunk.to_vec()),
            Received::Timeout => return (chunks, false),
            Received::Ended => return (chunks, true),
        }
    }
}

/// Ogg pages with small made-up packets: framed like audio, but quick to make.
fn quick_pages(count: usize) -> Vec<Vec<u8>> {
    let sink = crate::framing::Sink::default();
    let mut muxer = OggMuxer::new(sink.clone());
    muxer.start("Cloud Signal", None, 312).unwrap();
    for n in 0..count {
        muxer.packet(vec![(n % 251) as u8; 12], false).unwrap();
    }
    let bytes = sink.take();
    pages(&bytes).iter().map(|p| p.to_vec()).collect()
}

fn mp3_frames(count: usize) -> Vec<u8> {
    // MPEG-1 Layer III, 128 kbit/s, 48 kHz: 384 bytes a frame.
    let mut frame = vec![0xFF, 0xFB, 0x94, 0xC4];
    frame.resize(384, 0);
    frame.repeat(count)
}

#[test]
fn a_listener_who_joins_late_gets_a_decodable_stream() {
    let mount = Mount::new(ListenerCap::new(10));
    let mut guard = mount.begin(Format::Ogg, "Morning Station".into()).unwrap();
    let mut source = OggSource::new("Cloud Signal", Some("Echo"));
    source.audio(1000);
    // A title change before the join: the listener needs the new stream's headers.
    source.restart("Dawn", None);
    source.audio(1000);
    let before = source.take();
    let before_pages = pages(&before);
    let last_bos = before_pages.iter().rposition(|p| is_bos(p)).unwrap();
    assert!(last_bos > 0, "the stream has a second logical stream");
    let header_pages: Vec<Vec<u8>> = before_pages[last_bos..]
        .iter()
        .take_while(|p| granule(p) == 0)
        .map(|p| p.to_vec())
        .collect();
    // The join comes in the middle of a page, as a TCP read would cut it.
    let cut = before.len() - 10;
    for read in before[..cut].chunks(4096) {
        guard.write(read);
    }
    let last_before_join = granule(pages(&before[..cut]).last().unwrap());

    let listener = mount.listen().unwrap();
    assert_eq!(listener.format(), Format::Ogg);
    guard.write(&before[cut..]);
    source.audio(1000);
    guard.write(&source.take());

    let (chunks, ended) = drain(&listener);
    assert!(!ended);
    assert!(is_bos(&chunks[0]), "the first chunk is the BOS page");
    assert_eq!(chunks[..header_pages.len()], header_pages[..]);
    let audio = &chunks[header_pages.len()..];
    assert!(audio.iter().all(|c| c.starts_with(b"OggS")));
    assert!(granule(&audio[0]) > last_before_join);
    assert!(audio.windows(2).all(|w| granule(&w[1]) > granule(&w[0])));
}

#[test]
fn a_second_source_is_refused_until_the_first_ends() {
    let mount = Mount::new(ListenerCap::new(10));
    let first = mount.begin(Format::Ogg, "Morning Station".into());
    assert!(first.is_some());
    assert!(mount.begin(Format::Ogg, "Morning Station".into()).is_none());
    assert!(mount.begin(Format::Mp3, "Morning Station".into()).is_none());
    drop(first);
    assert!(mount.begin(Format::Mp3, "Morning Station".into()).is_some());
}

#[test]
fn a_listener_who_stops_reading_is_dropped_and_others_keep_up() {
    let mount = Mount::new(ListenerCap::new(10));
    let mut guard = mount.begin(Format::Ogg, "Morning Station".into()).unwrap();
    let all = quick_pages(1000);
    let headers = 2;
    for page in &all[..headers] {
        guard.write(page);
    }
    let stalled = mount.listen().unwrap();
    let reader = mount.listen().unwrap();
    assert_eq!(mount.stats().listeners, 2);

    let mut received = Vec::new();
    let mut writing = Duration::ZERO;
    for batch in all[headers..].chunks(100) {
        for page in batch {
            let start = Instant::now();
            guard.write(page);
            writing += start.elapsed();
        }
        // The reader keeps up; the stalled listener never reads.
        loop {
            match reader.recv(Duration::ZERO) {
                Received::Chunk(chunk) => received.push(chunk.to_vec()),
                Received::Timeout => break,
                Received::Ended => panic!("the reader was dropped"),
            }
        }
    }
    assert!(writing < Duration::from_secs(2), "writing took {writing:?}");
    assert_eq!(received, all);
    assert_eq!(mount.stats().listeners, 1);
    drop(stalled);
    assert_eq!(mount.stats().listeners, 1);
}

#[test]
fn a_listener_dropped_for_falling_behind_ends_at_once() {
    let mount = Mount::new(ListenerCap::new(10));
    let mut guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let stalled = mount.listen().unwrap();
    // Far more frames than its queue holds, a few at a time, as a source sends them.
    for _ in 0..200 {
        guard.write(&mp3_frames(4));
    }
    assert_eq!(
        mount.stats().listeners,
        0,
        "the stalled listener was dropped"
    );
    // Its queue is full, but none of it is handed out: a slow client is not sent it.
    assert_eq!(stalled.recv(Duration::ZERO), Received::Ended);
}

#[test]
fn the_cap_refuses_one_listener_too_many() {
    let mount = Mount::new(ListenerCap::new(2));
    assert!(matches!(mount.listen(), Err(ListenError::NoSource)));
    let _guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let first = mount.listen().unwrap();
    let _second = mount.listen().unwrap();
    assert!(matches!(mount.listen(), Err(ListenError::Full)));
    drop(first);
    assert!(mount.listen().is_ok());
}

#[test]
fn a_tap_takes_no_listener_place() {
    let mount = Mount::new(ListenerCap::new(1));
    assert!(mount.tap().is_none(), "no tap without a source");
    let mut guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let _listener = mount.listen().unwrap();
    assert!(matches!(mount.listen(), Err(ListenError::Full)));
    let tap = mount.tap().expect("a mount at its limit still gives a tap");
    assert_eq!(mount.stats().listeners, 1, "the tap is not counted");
    // The tap hears the stream like a listener.
    guard.write(&mp3_frames(2));
    assert!(matches!(tap.recv(WAIT), Received::Chunk(_)));
    drop(tap);
    assert_eq!(mount.stats().listeners, 1);
}

#[test]
fn mounts_of_one_station_share_the_limit() {
    let cap = ListenerCap::new(2);
    let stream = Mount::new(cap.clone());
    let output = Mount::new(cap);
    let _stream_source = stream.begin(Format::Ogg, "Morning Station".into()).unwrap();
    let _output_source = output.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let on_stream = stream.listen().unwrap();
    let _on_output = output.listen().unwrap();
    assert!(matches!(stream.listen(), Err(ListenError::Full)));
    assert!(matches!(output.listen(), Err(ListenError::Full)));
    let _tap = stream.tap().expect("a tap takes no place");
    assert_eq!(stream.stats().listeners, 1, "stats stay per mount");
    assert_eq!(output.stats().listeners, 1, "stats stay per mount");

    drop(on_stream);
    let _admitted = output.listen().expect("the freed place is anyone's");
    assert!(matches!(stream.listen(), Err(ListenError::Full)));
}

#[test]
fn a_listener_dropped_for_falling_behind_gives_its_place_back() {
    let mount = Mount::new(ListenerCap::new(1));
    let mut guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let stalled = mount.listen().unwrap();
    for _ in 0..200 {
        guard.write(&mp3_frames(4));
    }
    assert!(stalled.fell_behind());
    // Its place is held until its connection lets go of it.
    assert!(matches!(mount.listen(), Err(ListenError::Full)));
    drop(stalled);
    let _next = mount.listen().expect("the place came back");
    assert!(
        matches!(mount.listen(), Err(ListenError::Full)),
        "the place came back once"
    );
}

#[test]
fn a_source_ending_gives_every_place_back() {
    let cap = ListenerCap::new(2);
    let mount = Mount::new(cap.clone());
    let other = Mount::new(cap);
    let guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let listeners = [mount.listen().unwrap(), mount.listen().unwrap()];
    drop(guard);
    let _other_source = other.begin(Format::Mp3, "Morning Station".into()).unwrap();
    for listener in &listeners {
        assert_eq!(listener.recv(WAIT), Received::Ended);
    }
    // The places are held until the listeners' connections let go of them.
    assert!(matches!(other.listen(), Err(ListenError::Full)));
    drop(listeners);
    let _first = other.listen().expect("a place came back");
    let _second = other.listen().expect("every place came back");
    assert!(
        matches!(other.listen(), Err(ListenError::Full)),
        "each place came back once"
    );
}

#[test]
fn ending_the_source_ends_every_listener() {
    let mount = Mount::new(ListenerCap::new(10));
    let mut guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let listeners = [mount.listen().unwrap(), mount.listen().unwrap()];
    guard.write(&mp3_frames(5));
    drop(guard);
    for listener in &listeners {
        let (chunks, ended) = drain(listener);
        assert!(ended, "recv reports the end, not a timeout");
        assert_eq!(chunks.len(), 5, "what was queued still arrives");
    }
    let stats = mount.stats();
    assert!(!stats.live);
    assert_eq!(stats.listeners, 0);
    assert!(matches!(mount.listen(), Err(ListenError::NoSource)));
}

#[test]
fn a_listener_of_an_ended_source_never_hears_the_next_one() {
    let mount = Mount::new(ListenerCap::new(10));
    let mut guard = mount.begin(Format::Ogg, "Morning Station".into()).unwrap();
    let mut source = OggSource::new("Cloud Signal", None);
    source.audio(200);
    guard.write(&source.take());
    let old = mount.listen().unwrap();
    source.audio(200);
    guard.write(&source.take());
    drop(guard);

    let mut guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    guard.write(&mp3_frames(10));
    let new = mount.listen().unwrap();
    assert_eq!(new.format(), Format::Mp3);
    guard.write(&mp3_frames(10));

    let (chunks, ended) = drain(&old);
    assert!(ended);
    assert!(!chunks.is_empty());
    assert!(chunks.iter().all(|c| c.starts_with(b"OggS")));
    assert_eq!(old.format(), Format::Ogg);

    // The new listener gets no Ogg header pages from the old source.
    let (chunks, _) = drain(&new);
    assert!(!chunks.is_empty());
    assert!(chunks.iter().all(|c| c[0] == 0xFF && c[1] & 0xE0 == 0xE0));
}

#[test]
fn stats_follow_the_ogg_tags_and_the_mp3_title() {
    let mount = Mount::new(ListenerCap::new(10));
    let mut guard = mount.begin(Format::Ogg, "Morning Station".into()).unwrap();
    let mut source = OggSource::new("Cloud Signal", Some("Echo"));
    source.audio(100);
    guard.write(&source.take());
    let _listener = mount.listen().unwrap();
    let stats = mount.stats();
    assert!(stats.live);
    assert_eq!(stats.format, Some(Format::Ogg));
    assert_eq!(stats.listeners, 1);
    assert_eq!(stats.name, "Morning Station");
    assert_eq!(
        (stats.title.as_str(), stats.artist.as_str()),
        ("Cloud Signal", "Echo")
    );

    source.restart("Dawn", None);
    source.audio(100);
    guard.write(&source.take());
    let stats = mount.stats();
    assert_eq!((stats.title.as_str(), stats.artist.as_str()), ("Dawn", ""));
    drop(guard);

    let stats = mount.stats();
    assert!(!stats.live);
    assert_eq!(stats.format, None);
    assert_eq!(stats.title, "");

    let mut guard = mount.begin(Format::Mp3, "Night Shift".into()).unwrap();
    guard.write(&mp3_frames(3));
    assert_eq!(
        mount.stats().title,
        "",
        "a new source starts without the old title"
    );
    mount.set_title("Cloud Signal".into());
    let stats = mount.stats();
    assert_eq!(stats.format, Some(Format::Mp3));
    assert_eq!(stats.name, "Night Shift");
    assert_eq!(stats.title, "Cloud Signal");
}
