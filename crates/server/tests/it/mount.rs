use crate::framing::{OggSource, granule, is_bos, pages};
use radio_server::{Format, ListenError, Listener, Mount, Received};
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
    let mount = Mount::new(10);
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
    let mount = Mount::new(10);
    let first = mount.begin(Format::Ogg, "Morning Station".into());
    assert!(first.is_some());
    assert!(mount.begin(Format::Ogg, "Morning Station".into()).is_none());
    assert!(mount.begin(Format::Mp3, "Morning Station".into()).is_none());
    drop(first);
    assert!(mount.begin(Format::Mp3, "Morning Station".into()).is_some());
}

#[test]
fn a_listener_who_stops_reading_is_dropped_and_others_keep_up() {
    let mount = Mount::new(10);
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
fn the_cap_refuses_one_listener_too_many() {
    let mount = Mount::new(2);
    assert!(matches!(mount.listen(), Err(ListenError::NoSource)));
    let _guard = mount.begin(Format::Mp3, "Morning Station".into()).unwrap();
    let first = mount.listen().unwrap();
    let _second = mount.listen().unwrap();
    assert!(matches!(mount.listen(), Err(ListenError::Full)));
    drop(first);
    assert!(mount.listen().is_ok());
}

#[test]
fn ending_the_source_ends_every_listener() {
    let mount = Mount::new(10);
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
    let mount = Mount::new(10);
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
    let mount = Mount::new(10);
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
