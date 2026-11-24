//! ICY titles for MP3 listeners: what the writer interleaves, read back with the app's own
//! parser, and what a listener who asks for titles (or doesn't) is sent.

use crate::server::{EAR_PATIENCE, MORNING_KEY, Tone, on_air, set_title, with_outputs};
use crate::support::{Head, PATIENCE, client, read_head};
use radio_server::{IcyWriter, METAINT};
use radio_stream::{IcyState, parse_icy};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Audio that is not all one byte, so a misplaced block would show.
fn audio(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// Every title `parse_icy` finds in `bytes`, in order, and the audio left.
fn parsed(bytes: &[u8], metaint: usize, piece: usize) -> (Vec<u8>, Vec<String>) {
    let mut state = IcyState::default();
    let mut audio = Vec::new();
    let mut titles = Vec::new();
    for chunk in bytes.chunks(piece) {
        let (out, title) = parse_icy(chunk, metaint, &mut state);
        audio.extend(out);
        titles.extend(title);
    }
    (audio, titles)
}

#[test]
fn the_apps_parser_reads_the_title_and_the_audio_back() {
    let mut writer = IcyWriter::new();
    let first = audio(METAINT * 2 + 700);
    let second = audio(METAINT * 2 + 300);
    let mut bytes = Vec::new();
    // Chunks of uneven sizes, so blocks fall inside chunks and between them.
    for chunk in first.chunks(4093) {
        writer.write(chunk, "Echo - Cloud Signal", &mut bytes);
    }
    for chunk in second.chunks(1531) {
        writer.write(chunk, "Echo - Paper Lanterns", &mut bytes);
    }

    // Read back in pieces of another size, so a block spans reads too.
    let (heard, titles) = parsed(&bytes, METAINT, 1000);
    assert_eq!(
        heard,
        [first, second].concat(),
        "the audio comes out as it went in"
    );
    assert_eq!(titles, ["Echo - Cloud Signal", "Echo - Paper Lanterns"]);
}

#[test]
fn an_unchanged_title_is_sent_once() {
    let mut writer = IcyWriter::new();
    let mut bytes = Vec::new();
    for chunk in audio(METAINT * 5).chunks(4096) {
        writer.write(chunk, "Cloud Signal", &mut bytes);
    }
    let (heard, titles) = parsed(&bytes, METAINT, 4096);
    assert_eq!(heard.len(), METAINT * 5);
    assert_eq!(titles, ["Cloud Signal"]);
    let first_block = usize::from(bytes[METAINT]) * 16;
    assert_eq!(
        bytes.len(),
        METAINT * 5 + 1 + first_block + 4,
        "five blocks: the title, then four empty ones"
    );
}

#[test]
fn a_title_too_long_for_a_block_is_cut_at_a_character() {
    // Two bytes a character in UTF-8: 3000 of them make 6000 bytes, more than a block holds.
    let long: String = "é".repeat(3000);
    let mut writer = IcyWriter::new();
    let mut bytes = Vec::new();
    writer.write(&audio(METAINT), &long, &mut bytes);
    assert!(
        bytes.len() - METAINT - 1 <= 255 * 16,
        "a block is at most 4080 bytes"
    );
    let (heard, titles) = parsed(&bytes, METAINT, 4096);
    assert_eq!(heard, audio(METAINT));
    let [title] = titles.as_slice() else {
        panic!("one title: {titles:?}");
    };
    assert!(!title.contains('\u{FFFD}'), "cut inside a character");
    assert!(!title.is_empty() && long.starts_with(title.as_str()));
    assert!(title.len() < long.len());
}

/// MPEG-1/2 Layer III frame length from a header at the start of `bytes`, if it is one.
fn mp3_frame_len(bytes: &[u8]) -> Option<usize> {
    let [a, b, c, ..] = *bytes else {
        return None;
    };
    if a != 0xFF || b & 0xE0 != 0xE0 || (b >> 1) & 0x3 != 0x1 {
        return None;
    }
    let mpeg1 = (b >> 3) & 0x3 == 0x3;
    let kbps: &[u32] = if mpeg1 {
        &[
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
    } else {
        &[0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
    };
    let kbps = *kbps.get(usize::from(c >> 4))?;
    let rate = match ((b >> 3) & 0x3, (c >> 2) & 0x3) {
        (3, 0) => 44_100,
        (3, 1) => 48_000,
        (3, 2) => 32_000,
        (2, 0) => 22_050,
        (2, 1) => 24_000,
        (2, 2) => 16_000,
        _ => return None,
    };
    if kbps == 0 {
        return None;
    }
    let per_frame = if mpeg1 { 144 } else { 72 };
    Some((per_frame * kbps * 1000 / rate + u32::from((c >> 1) & 1)) as usize)
}

/// Whether `bytes` is MP3 frames back to back from its first byte, at least `frames` of
/// them.
fn frames_from_the_start(bytes: &[u8], frames: usize) -> bool {
    let mut at = 0;
    let mut count = 0;
    while let Some(len) = mp3_frame_len(&bytes[at..]) {
        count += 1;
        at += len;
        if at >= bytes.len() {
            break;
        }
    }
    count >= frames
}

/// `GET /<slug>/<mount>`, asking for ICY titles when `icy` is set.
fn listen(addr: SocketAddr, slug: &str, mount: &str, icy: bool) -> (TcpStream, Head) {
    let mut stream = client(addr);
    let ask = if icy { "Icy-MetaData: 1\r\n" } else { "" };
    write!(
        stream,
        "GET /{slug}/{mount} HTTP/1.1\r\nHost: radio.example.test\r\n{ask}\r\n"
    )
    .unwrap();
    let head = read_head(&mut stream);
    (stream, head)
}

/// Reads at least `len` bytes, within `limit`.
fn read_at_least(stream: &mut TcpStream, len: usize, limit: Duration) -> Vec<u8> {
    let deadline = Instant::now() + limit;
    let mut out = Vec::new();
    let mut buf = [0; 4096];
    while out.len() < len {
        assert!(Instant::now() < deadline, "only {} bytes came", out.len());
        match stream.read(&mut buf) {
            Ok(0) => panic!("the stream ended after {} bytes", out.len()),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => panic!("reading: {e}"),
        }
    }
    out
}

/// Reads an ICY stream until a block carries `title`; returns the audio up to there.
fn read_until_title(stream: &mut TcpStream, metaint: usize, title: &str) -> Vec<u8> {
    let deadline = Instant::now() + EAR_PATIENCE;
    let mut state = IcyState::default();
    let mut audio = Vec::new();
    let mut seen = Vec::new();
    let mut buf = [0; 4096];
    loop {
        assert!(
            Instant::now() < deadline,
            "no block carried {title:?}; saw {seen:?} in {} bytes of audio",
            audio.len()
        );
        match stream.read(&mut buf) {
            Ok(0) => panic!("the stream ended; saw {seen:?}"),
            Ok(n) => {
                let (out, found) = parse_icy(&buf[..n], metaint, &mut state);
                audio.extend(out);
                if found.as_deref() == Some(title) {
                    return audio;
                }
                seen.extend(found);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => panic!("reading: {e}"),
        }
    }
}

fn metaint_of(head: &Head) -> usize {
    head.header("icy-metaint")
        .expect("an icy-metaint header")
        .parse()
        .expect("icy-metaint is a number")
}

#[test]
fn a_listener_who_asks_gets_titles() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    // The source sends Ogg/Opus titled "Cloud Signal" by "Echo".
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    let (mut stream, head) = listen(addr, "morning", "mp3", true);
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Type"), Some("audio/mpeg"));
    // The name the source gave, from its Ice-Name.
    assert_eq!(head.header("icy-name"), Some("Cloud Signal FM"));
    let metaint = metaint_of(&head);
    let audio = read_until_title(&mut stream, metaint, "Echo - Cloud Signal");
    assert!(
        frames_from_the_start(&audio, 10),
        "with the blocks taken out, the audio is MP3 frames"
    );
}

#[test]
fn the_pushed_mp3_mount_gives_titles_too() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Mp3 { rate: 48_000 });
    let (_, reply) = set_title(addr, "morning", "Cloud Signal", Some(MORNING_KEY));
    assert_eq!(reply.status, 200);
    let (mut stream, head) = listen(addr, "morning", "stream", true);
    assert_eq!(head.status, 200);
    assert_eq!(head.header("icy-name"), Some("Cloud Signal FM"));
    let metaint = metaint_of(&head);
    read_until_title(&mut stream, metaint, "Cloud Signal");
}

#[test]
fn a_listener_who_does_not_ask_gets_plain_mp3() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    let (mut stream, head) = listen(addr, "morning", "mp3", false);
    assert_eq!(head.status, 200);
    assert_eq!(head.header("icy-metaint"), None);
    assert_eq!(head.header("icy-name"), None);
    // Past where a first block would be. A frame is at most 418 bytes at this output's
    // 128 kbit/s (at 44.1 kHz, padded), so the chain must run through all of it: a block
    // at METAINT would break it about halfway.
    let body = read_at_least(&mut stream, METAINT * 2, EAR_PATIENCE);
    assert!(
        frames_from_the_start(&body, METAINT * 2 / 418),
        "the body is MP3 frames back to back from its first byte"
    );
}

#[test]
fn an_opus_mount_ignores_the_request() {
    let server = with_outputs(10);
    let addr = server.local_addr();
    let _pusher = on_air(addr, "morning", MORNING_KEY, Tone::Ogg);
    for mount in ["opus", "stream"] {
        let (mut stream, head) = listen(addr, "morning", mount, true);
        assert_eq!(head.status, 200, "/morning/{mount}");
        assert_eq!(head.header("Content-Type"), Some("audio/ogg"));
        assert_eq!(head.header("icy-metaint"), None, "/morning/{mount}");
        let body = read_at_least(&mut stream, 4, PATIENCE);
        assert!(body.starts_with(b"OggS"), "/morning/{mount}");
    }
}
