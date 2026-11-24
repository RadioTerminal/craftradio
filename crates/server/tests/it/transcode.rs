use crate::decode::{heard, stop};
use crate::framing::{OggSource, granule, is_bos, pages};
use crate::support::PATIENCE;
use craft_audio::mp3::{Config, Encoder};
use radio_server::{
    Decoded, Format, ListenerCap, Mount, Output, OutputFormat, Outputs, Received, Refusal,
    SourceDecoder, SourceGuard, Transcoder, spawn,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long a listener of an output may take, over a whole test, to hear what it should.
const EAR_PATIENCE: Duration = Duration::from_secs(20);
/// Packets of 20 ms in a second.
const SECOND: usize = 50;

/// A station with an Opus and an MP3 output.
struct Station {
    pushed: Arc<Mount>,
    outputs: Arc<Outputs>,
}

fn station() -> Station {
    let cap = ListenerCap::new(10);
    let configs = [
        Output {
            mount: "opus".into(),
            format: OutputFormat::Opus,
            kbps: 96,
        },
        Output {
            mount: "mp3".into(),
            format: OutputFormat::Mp3,
            kbps: 128,
        },
    ];
    Station {
        pushed: Mount::new(Arc::clone(&cap)),
        outputs: Outputs::new(&configs, &cap),
    }
}

impl Station {
    fn output(&self, name: &str) -> &Arc<Mount> {
        self.outputs.mount(name).expect("the output is configured")
    }

    fn spawn(&self) -> Transcoder {
        spawn(Arc::clone(&self.pushed), Arc::clone(&self.outputs))
    }

    fn outputs_live(&self) -> bool {
        self.outputs.each().all(|(_, mount)| mount.stats().live)
    }

    fn any_output_live(&self) -> bool {
        self.outputs.each().any(|(_, mount)| mount.stats().live)
    }

    /// Waits until the transcoder decodes nothing for a fifth of a second of pushing.
    fn settle(&self, pusher: &Pusher) {
        eventually("the transcoder went idle", || {
            let before = self.outputs.decoded_blocks();
            pusher.wait_packets(SECOND / 5);
            self.outputs.decoded_blocks() == before
        });
    }
}

/// Waits for `condition`, failing the test after [`PATIENCE`].
fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "{what}: not within {PATIENCE:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// `finish`, failing the test if it does not return within a second.
fn finish_promptly(transcoder: Transcoder) {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        transcoder.finish();
        let _ = done.send(());
    });
    finished
        .recv_timeout(Duration::from_secs(1))
        .expect("finish returned within a second");
}

/// What a source pushes.
#[derive(Clone, Copy)]
enum Source {
    Ogg,
    Mp3 { rate: u32 },
}

/// What the test asks the pushing thread to do next.
#[derive(Default)]
struct Requests {
    /// Start a new Ogg chain with this title.
    retitle: Option<String>,
    /// Write this many silent pages at once, without pause.
    burst: usize,
    /// Push silence instead of the tone from now on.
    quiet: bool,
}

/// Pushes a tone into a mount in real time from a thread, 20 ms at a time, until dropped;
/// dropping it ends the source.
struct Pusher {
    stop: Arc<AtomicBool>,
    packets: Arc<AtomicUsize>,
    requests: Arc<Mutex<Requests>>,
    thread: Option<JoinHandle<()>>,
}

impl Pusher {
    fn start(mount: &Arc<Mount>, source: Source) -> Pusher {
        let format = match source {
            Source::Ogg => Format::Ogg,
            Source::Mp3 { .. } => Format::Mp3,
        };
        let guard = mount
            .begin(format, "Morning Station".into(), Some(128))
            .expect("no other source is live");
        let stop = stop();
        let packets = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Requests::default()));
        let thread = {
            let (stop, packets, requests) = (
                Arc::clone(&stop),
                Arc::clone(&packets),
                Arc::clone(&requests),
            );
            thread::spawn(move || push(guard, source, &stop, &packets, &requests))
        };
        Pusher {
            stop,
            packets,
            requests,
            thread: Some(thread),
        }
    }

    fn packets(&self) -> usize {
        self.packets.load(Ordering::Acquire)
    }

    /// Waits until `more` packets more have been pushed.
    fn wait_packets(&self, more: usize) {
        let target = self.packets() + more;
        let deadline = Instant::now() + PATIENCE + Duration::from_millis(20 * more as u64);
        while self.packets() < target {
            assert!(Instant::now() < deadline, "the pusher fell behind");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn retitle(&self, title: &str) {
        self.requests.lock().unwrap().retitle = Some(title.into());
    }

    fn burst(&self, pages: usize) {
        self.requests.lock().unwrap().burst = pages;
    }

    /// Pushes silence instead of the tone from now on (Ogg only).
    fn quiet(&self) {
        self.requests.lock().unwrap().quiet = true;
    }

    /// Ends the source.
    fn end(self) {}
}

impl Drop for Pusher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn write(guard: &mut SourceGuard, bytes: &[u8]) {
    for piece in bytes.chunks(4096) {
        guard.write(piece);
    }
}

fn push(
    mut guard: SourceGuard,
    source: Source,
    stop: &AtomicBool,
    packets: &AtomicUsize,
    requests: &Mutex<Requests>,
) {
    let mut ogg = OggSource::new("Cloud Signal", Some("Echo"));
    let mut mp3 = match source {
        Source::Mp3 { rate } => Some((Encoder::new(Config::cbr(rate, 2, 128)).unwrap(), rate)),
        Source::Ogg => None,
    };
    let mut clock = Instant::now();
    let mut ticks = 0u32;
    let mut frames = 0usize;
    let mut quiet = false;
    while !stop.load(Ordering::Acquire) {
        let Requests {
            retitle,
            burst,
            quiet: hush,
        } = std::mem::take(&mut *requests.lock().unwrap());
        quiet |= hush;
        if let Some(title) = retitle {
            ogg.restart(&title, None);
        }
        if burst > 0 {
            ogg.silence(burst * 20);
            write(&mut guard, &ogg.take());
            packets.fetch_add(burst, Ordering::AcqRel);
            // Real time again from here.
            clock = Instant::now();
            ticks = 0;
        }
        let bytes = match &mut mp3 {
            None if quiet => {
                ogg.silence(20);
                ogg.take()
            }
            None => {
                ogg.audio(20);
                ogg.take()
            }
            Some((encoder, rate)) => {
                let count = *rate as usize / SECOND;
                let tone: Vec<f32> = (frames..frames + count)
                    .map(|i| (i as f32 / *rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.2)
                    .collect();
                frames += count;
                encoder.encode(&[&tone, &tone])
            }
        };
        write(&mut guard, &bytes);
        packets.fetch_add(1, Ordering::AcqRel);
        ticks += 1;
        let next = clock + Duration::from_millis(20) * ticks;
        thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}

/// Sets a flag after a time, unless dropped first; its thread ends when it is dropped.
struct Deadline {
    flag: Arc<AtomicBool>,
    cancel: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Deadline {
    fn after(limit: Duration) -> Deadline {
        let flag = stop();
        let (cancel, cancelled) = mpsc::channel::<()>();
        let shared = Arc::clone(&flag);
        let thread = thread::spawn(move || {
            if cancelled.recv_timeout(limit) == Err(RecvTimeoutError::Timeout) {
                shared.store(true, Ordering::Release);
            }
        });
        Deadline {
            flag,
            cancel: Some(cancel),
            thread: Some(thread),
        }
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        drop(self.cancel.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A listener of an output, decoding what it receives.
struct Ear {
    decoder: SourceDecoder,
    samples: Vec<f32>,
    _deadline: Deadline,
}

impl Ear {
    fn join(mount: &Arc<Mount>) -> Ear {
        let listener = mount.listen().expect("the output is live");
        let deadline = Deadline::after(EAR_PATIENCE);
        let decoder =
            SourceDecoder::open(listener, Arc::clone(&deadline.flag)).expect("the output opens");
        Ear {
            decoder,
            samples: Vec::new(),
            _deadline: deadline,
        }
    }

    /// The next packet's samples, or how the stream stopped.
    fn next(&mut self) -> Result<&[f32], Decoded> {
        self.samples.clear();
        match self
            .decoder
            .next(&mut self.samples)
            .expect("the output decodes")
        {
            Decoded::Audio => Ok(&self.samples),
            other => Err(other),
        }
    }

    /// Decodes until the tone is heard; false if the stream stopped first.
    fn hears(&mut self) -> bool {
        loop {
            match self.next() {
                Ok(samples) if heard(samples) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    /// Decodes until `packets` packets with the tone in them; false if the stream stopped
    /// first.
    fn hears_for(&mut self, packets: usize) -> bool {
        let mut heard_packets = 0;
        while heard_packets < packets {
            match self.next() {
                Ok(samples) if heard(samples) => heard_packets += 1,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        true
    }

    /// Decodes, keeping up with the stream, until `condition` holds.
    fn keep_listening_until(&mut self, what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !condition() {
            assert!(Instant::now() < deadline, "{what}: not within {PATIENCE:?}");
            assert!(self.next().is_ok(), "{what}: the output stopped first");
        }
    }

    /// Decodes until the stream stops, and says how.
    fn end(&mut self) -> Decoded {
        loop {
            if let Err(end) = self.next() {
                return end;
            }
        }
    }
}

#[test]
fn an_output_nobody_listens_to_is_not_decoded() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    station.settle(&pusher);

    let before = station.outputs.decoded_blocks();
    pusher.wait_packets(2 * SECOND);
    assert_eq!(
        station.outputs.decoded_blocks(),
        before,
        "two seconds of a source nobody hears were decoded"
    );

    let _ear = Ear::join(station.output("opus"));
    eventually("a listener's output is decoded", || {
        station.outputs.decoded_blocks() > before
    });
    finish_promptly(transcoder);
}

#[test]
fn a_listener_on_an_output_hears_the_source() {
    for (source, output) in [(Source::Ogg, "mp3"), (Source::Mp3 { rate: 48_000 }, "opus")] {
        let station = station();
        let _pusher = Pusher::start(&station.pushed, source);
        let transcoder = station.spawn();
        eventually("the outputs went live", || station.outputs_live());
        let mut ear = Ear::join(station.output(output));
        assert!(ear.hears(), "the {output} output plays the tone");
        finish_promptly(transcoder);
    }
}

#[test]
fn every_join_after_a_pause_hears_audio() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    for round in 0..5 {
        for output in ["opus", "mp3"] {
            let mut ear = Ear::join(station.output(output));
            assert!(ear.hears(), "join {round} on {output} hears the tone");
            drop(ear);
            station.settle(&pusher);
        }
    }
    // And again at once, before the transcoder may have seen the output empty.
    for round in 0..5 {
        for output in ["opus", "mp3"] {
            let mut ear = Ear::join(station.output(output));
            assert!(ear.hears(), "quick join {round} on {output} hears the tone");
        }
    }
    finish_promptly(transcoder);
}

#[test]
fn a_pause_drops_what_was_decoded_before_it() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    let opus = station.output("opus");
    let mut ear = Ear::join(opus);
    // A second of audio, so the chain before the pause is well under way.
    assert!(ear.hears_for(SECOND));
    drop(ear);
    // The tone goes on during the pause, then the source goes quiet.
    station.settle(&pusher);
    pusher.quiet();
    pusher.wait_packets(SECOND);

    let pages = opus.listen().unwrap();
    let mut ear = Ear::join(opus);
    for _ in 0..SECOND / 2 {
        let samples = ear.next().expect("the output plays");
        assert!(
            !heard(samples),
            "audio from before the pause was played after it"
        );
    }
    // Header pages carry no position; the first page of audio a listener joining in a
    // pause gets is the start of a chain, not the tail of the one before the pause.
    let first_audio = loop {
        match pages.recv(PATIENCE) {
            Received::Chunk(page) if granule(&page) > 0 => break granule(&page),
            Received::Chunk(_) => {}
            other => panic!("the output stopped: {other:?}"),
        }
    };
    assert!(
        first_audio <= 960,
        "the first audio page is at sample {first_audio} of its chain, not its first packet"
    );
    finish_promptly(transcoder);
}

#[test]
fn a_listener_joining_after_a_pause_gets_audio_before_any_chain_ends() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    let opus = station.output("opus");
    for round in 0..3 {
        let mut ear = Ear::join(opus);
        assert!(ear.hears(), "join {round} hears the tone");
        drop(ear);
        station.settle(&pusher);

        let seen = first_pages(opus);
        assert!(
            seen[0].0,
            "join {round} starts at a chain's start: {seen:?}"
        );
        assert!(
            !seen.iter().any(|&(_, ends, _)| ends),
            "join {round}: a chain ended before or between its first packets: {seen:?}"
        );
        assert_eq!(
            seen.iter().filter(|&&(begins, _, _)| begins).count(),
            1,
            "join {round} gets one chain: {seen:?}"
        );
    }
    finish_promptly(transcoder);
}

/// A new listener's pages, each as (begins a chain, ends a chain, position), up to its
/// tenth page of audio.
fn first_pages(opus: &Arc<Mount>) -> Vec<(bool, bool, u64)> {
    let listener = opus.listen().unwrap();
    let mut seen = Vec::new();
    while seen.iter().filter(|&&(_, _, at)| at > 0).count() < 10 {
        match listener.recv(PATIENCE) {
            Received::Chunk(chunk) => seen.extend(
                pages(&chunk)
                    .into_iter()
                    .map(|page| (is_bos(page), page[5] & 0x04 != 0, granule(page))),
            ),
            other => panic!("the output stopped: {other:?}"),
        }
    }
    seen
}

#[test]
fn a_listener_joining_after_a_title_change_in_a_pause_gets_one_chain() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    let opus = station.output("opus");
    let mut ear = Ear::join(opus);
    assert!(ear.hears());
    drop(ear);
    station.settle(&pusher);

    pusher.retitle("Paper Harbour");
    eventually("the source took the new title", || {
        station.pushed.stats().title == "Paper Harbour"
    });
    // Several of the idle transcoder's looks at the source.
    pusher.wait_packets(SECOND / 5);

    let seen = first_pages(opus);
    assert!(seen[0].0, "the join starts at a chain's start: {seen:?}");
    assert!(
        !seen.iter().any(|&(_, ends, _)| ends),
        "a chain ended before or between the first packets: {seen:?}"
    );
    assert_eq!(
        seen.iter().filter(|&&(begins, _, _)| begins).count(),
        1,
        "the join gets one chain: {seen:?}"
    );
    assert_eq!(
        opus.stats().title,
        "Paper Harbour",
        "the chain has the new title"
    );
    finish_promptly(transcoder);
}

#[test]
fn a_title_change_reaches_an_opus_output_and_audio_goes_on() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    let opus = station.output("opus");
    let mut ear = Ear::join(opus);
    assert!(ear.hears());
    assert_eq!(
        opus.stats().title,
        "Cloud Signal",
        "the output starts with the source's title"
    );
    assert_eq!(opus.stats().artist, "Echo");

    pusher.retitle("Paper Harbour");
    ear.keep_listening_until("the output took the new title", || {
        opus.stats().title == "Paper Harbour"
    });
    assert_eq!(opus.stats().artist, "", "the new title has no artist");
    // Kept up to the change, so a second of tone from now on was encoded after it.
    assert!(ear.hears_for(SECOND), "the output goes on after the change");
    finish_promptly(transcoder);
}

#[test]
fn a_source_that_is_not_48_khz_stereo_turns_outputs_off() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Mp3 { rate: 44_100 });
    let listener = station.pushed.listen().unwrap();
    let transcoder = station.spawn();
    eventually("the outputs were refused", || {
        assert!(!station.any_output_live(), "an output went live");
        station.outputs.refusal().is_some()
    });
    assert_eq!(
        station.outputs.refusal(),
        Some(Refusal::Unsupported {
            rate: 44_100,
            channels: 2
        })
    );
    pusher.wait_packets(SECOND / 2);
    assert!(!station.any_output_live(), "an output went live");
    assert!(
        matches!(listener.recv(PATIENCE), Received::Chunk(_)),
        "the pushed stream is still served"
    );

    pusher.end();
    eventually("the refusal was cleared", || {
        station.outputs.refusal().is_none()
    });
    finish_promptly(transcoder);
}

#[test]
fn the_source_ending_ends_every_output() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    let listeners: Vec<_> = ["opus", "mp3"]
        .map(|name| station.output(name).listen().unwrap())
        .into();
    for listener in &listeners {
        assert!(matches!(listener.recv(PATIENCE), Received::Chunk(_)));
    }

    pusher.end();
    for listener in &listeners {
        let deadline = Instant::now() + PATIENCE;
        loop {
            assert!(Instant::now() < deadline, "the output did not end");
            match listener.recv(PATIENCE) {
                Received::Chunk(_) => {}
                Received::Timeout => panic!("the output went quiet without ending"),
                Received::Ended => break,
            }
        }
        assert!(!listener.fell_behind(), "ended, not dropped");
    }
    assert!(!station.any_output_live());
    // The thread has ended on its own; finish is still safe.
    finish_promptly(transcoder);
}

#[test]
fn the_source_ending_while_nobody_listens_ends_every_output() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    station.settle(&pusher);

    pusher.end();
    eventually("the outputs ended", || !station.any_output_live());
    finish_promptly(transcoder);
}

#[test]
fn a_new_source_in_another_format_is_heard_on_the_outputs() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    let mut old = Ear::join(station.output("mp3"));
    assert!(old.hears());
    finish_promptly(transcoder);
    assert!(!station.any_output_live(), "finish ended every output");
    pusher.end();
    assert_eq!(old.end(), Decoded::Ended);

    let _pusher = Pusher::start(&station.pushed, Source::Mp3 { rate: 48_000 });
    let transcoder = station.spawn();
    eventually("the outputs went live again", || station.outputs_live());
    for output in ["opus", "mp3"] {
        let mut ear = Ear::join(station.output(output));
        assert!(ear.hears(), "the {output} output plays the new source");
    }
    finish_promptly(transcoder);
}

#[test]
fn outputs_recover_from_a_burst() {
    let station = station();
    let pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the outputs went live", || station.outputs_live());
    let opus = station.output("opus");
    let mut ear = Ear::join(opus);
    assert!(ear.hears());

    // Silent pages, so tone heard from now on was pushed just before the burst (a few
    // packets at most, since the listener kept up until now) or after it.
    let before = pusher.packets();
    pusher.burst(2000);
    eventually("the burst was pushed", || pusher.packets() > before + 2000);
    pusher.wait_packets(SECOND);
    if !ear.hears_for(SECOND) {
        // The listener was dropped for falling behind; a new one must hear the source.
        drop(ear);
        let mut late = Ear::join(opus);
        assert!(
            late.hears_for(SECOND),
            "a listener after the burst hears the tone"
        );
    }
    let decoded = station.outputs.decoded_blocks();
    eventually("the output is still decoded", || {
        station.outputs.decoded_blocks() > decoded
    });
    finish_promptly(transcoder);
}

#[test]
fn finish_returns_while_the_source_is_silent() {
    let station = station();
    let _guard = station
        .pushed
        .begin(Format::Ogg, "Morning Station".into(), None)
        .unwrap();
    let transcoder = station.spawn();
    // Gives the thread time to start waiting on the source; what the test proves is the
    // deadline in `finish_promptly`.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        station.outputs.refusal(),
        None,
        "a silent source is not refused"
    );
    finish_promptly(transcoder);
    assert!(!station.any_output_live());
}

#[test]
fn an_output_that_cannot_begin_is_marked_failed_and_the_others_play() {
    let station = station();
    // Something else holds the MP3 output's mount, so the transcoder can't begin it.
    let held = station
        .output("mp3")
        .begin(Format::Mp3, "Morning Station".into(), None)
        .unwrap();
    let _pusher = Pusher::start(&station.pushed, Source::Ogg);
    let transcoder = station.spawn();
    eventually("the MP3 output was marked failed", || {
        station.outputs.failed("mp3")
    });
    assert!(
        !station.outputs.failed("opus"),
        "the other output failed too"
    );
    assert_eq!(
        station.outputs.refusal(),
        None,
        "a failure is not a refusal"
    );
    eventually("the Opus output went live", || {
        station.output("opus").stats().live
    });
    let mut ear = Ear::join(station.output("opus"));
    assert!(ear.hears(), "the other output plays");

    finish_promptly(transcoder);
    assert!(
        !station.outputs.failed("mp3"),
        "the failure ends with the source's transcoder"
    );
    drop(held);
}
