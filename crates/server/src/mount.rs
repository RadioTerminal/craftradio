//! A mount: one source's stream, handed to every listener.
//!
//! One source thread writes through a [`SourceGuard`]; each listener thread reads its own
//! bounded queue through a [`Listener`]. Writing never waits for a listener: a listener whose
//! queue is full is dropped, and hears nothing more, not even what is already queued.

use crate::framing::{Format, Framer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// Chunks (pages or frames) a listener may fall behind before it is dropped.
const QUEUE: usize = 256;
/// The longest title or artist a mount keeps, in characters, whether it came from a
/// title update or from Ogg tags; the rest is cut off, so a key holder cannot bloat the
/// public stats.
const MAX_TITLE_CHARS: usize = 512;

/// `text` cut to [`MAX_TITLE_CHARS`] characters.
fn capped(text: String) -> String {
    match text.char_indices().nth(MAX_TITLE_CHARS) {
        Some((end, _)) => text.get(..end).unwrap_or_default().to_owned(),
        None => text,
    }
}

/// Why a listener could not join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ListenError {
    #[error("no source is live on this mount")]
    NoSource,
    #[error("the mount has as many listeners as it allows")]
    Full,
}

/// What [`Listener::recv`] got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// A whole page or frame.
    Chunk(Arc<[u8]>),
    /// Nothing arrived in time; the source is still live.
    Timeout,
    /// The source ended (once what was queued has been received), or this listener fell
    /// too far behind and was dropped (at once).
    Ended,
}

/// What a mount is doing, for status pages and logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountStats {
    pub live: bool,
    pub format: Option<Format>,
    pub listeners: usize,
    pub name: String,
    pub title: String,
    pub artist: String,
}

/// One stream and its listeners.
#[derive(Debug)]
pub struct Mount {
    max_listeners: usize,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    live: Option<Live>,
    next_id: u64,
    name: String,
    title: String,
    artist: String,
}

/// The source that is pushing now.
#[derive(Debug)]
struct Live {
    format: Format,
    /// Sent to each new listener before the stream.
    headers: Vec<Arc<[u8]>>,
    listeners: Vec<Slot>,
}

#[derive(Debug)]
struct Slot {
    id: u64,
    queue: SyncSender<Arc<[u8]>>,
    /// Set when the listener is dropped for falling behind; shared with its [`Listener`].
    dropped: Arc<AtomicBool>,
}

impl Mount {
    pub fn new(max_listeners: usize) -> Arc<Mount> {
        Arc::new(Mount {
            max_listeners,
            state: Mutex::default(),
        })
    }

    /// The state, even after a thread panicked while holding it: every change to it is a
    /// single assignment or `retain`, so it stays consistent, and refusing every later
    /// source and listener would take the station off the air.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a source. `None` while another source is live.
    pub fn begin(self: &Arc<Self>, format: Format, name: String) -> Option<SourceGuard> {
        let mut state = self.state();
        if state.live.is_some() {
            return None;
        }
        state.live = Some(Live {
            format,
            headers: Vec::new(),
            listeners: Vec::new(),
        });
        state.name = name;
        state.title.clear();
        state.artist.clear();
        let framer = Framer::new(format);
        Some(SourceGuard {
            mount: Arc::clone(self),
            seen: framer.version(),
            framer,
            chunks: Vec::new(),
        })
    }

    /// Joins the stream. A new listener first receives the header pages, then the stream
    /// from the next page or frame on.
    pub fn listen(self: &Arc<Self>) -> Result<Listener, ListenError> {
        let mut state = self.state();
        let State { live, next_id, .. } = &mut *state;
        let live = live.as_mut().ok_or(ListenError::NoSource)?;
        if live.listeners.len() >= self.max_listeners {
            return Err(ListenError::Full);
        }
        let id = *next_id;
        *next_id += 1;
        let (queue, receiver) = sync_channel(QUEUE);
        // The framer keeps far fewer header pages than the queue holds.
        for page in &live.headers {
            let _ = queue.try_send(Arc::clone(page));
        }
        let dropped = Arc::new(AtomicBool::new(false));
        live.listeners.push(Slot {
            id,
            queue,
            dropped: Arc::clone(&dropped),
        });
        Ok(Listener {
            mount: Arc::clone(self),
            id,
            format: live.format,
            queue: receiver,
            dropped,
        })
    }

    /// Sets the title of an MP3 source, whose stream carries none. An Ogg source's next
    /// tags replace it. Cut to 512 characters. Returns whether a source was live to take
    /// it.
    pub fn set_title(&self, title: String) -> bool {
        let mut state = self.state();
        let live = state.live.is_some();
        if live {
            state.title = capped(title);
        }
        live
    }

    pub fn stats(&self) -> MountStats {
        let state = self.state();
        MountStats {
            live: state.live.is_some(),
            format: state.live.as_ref().map(|live| live.format),
            listeners: state.live.as_ref().map_or(0, |live| live.listeners.len()),
            name: state.name.clone(),
            title: state.title.clone(),
            artist: state.artist.clone(),
        }
    }
}

/// The live source of a mount. Dropping it ends the stream for every listener.
#[derive(Debug)]
pub struct SourceGuard {
    mount: Arc<Mount>,
    framer: Framer,
    /// The framer's header version the mount has.
    seen: u64,
    chunks: Vec<Arc<[u8]>>,
}

impl SourceGuard {
    /// Passes the source's bytes on, a whole page or frame at a time. Never waits for a
    /// listener: one whose queue is full is dropped.
    /// Pass a source's reads in small pieces (4 KiB or less): one call that frames more
    /// chunks than a listener's queue holds (256) drops every listener.
    pub fn write(&mut self, bytes: &[u8]) {
        self.chunks.clear();
        self.framer.push(bytes, &mut self.chunks);
        let changed = self.framer.version() != self.seen;
        if self.chunks.is_empty() && !changed {
            return;
        }
        let mut state = self.mount.state();
        let State {
            live,
            title,
            artist,
            ..
        } = &mut *state;
        let Some(live) = live.as_mut() else {
            return;
        };
        let chunks = &self.chunks;
        live.listeners.retain(|slot| {
            let kept = chunks
                .iter()
                .all(|chunk| slot.queue.try_send(Arc::clone(chunk)).is_ok());
            if !kept {
                slot.dropped.store(true, Ordering::Release);
            }
            kept
        });
        // Under the same lock as the sends, so a listener joining now gets either these
        // chunks or the headers they set, never neither.
        if changed {
            self.seen = self.framer.version();
            live.headers = self.framer.headers();
            if let Some((tag_title, tag_artist)) = self.framer.tags() {
                *title = capped(tag_title);
                *artist = capped(tag_artist.unwrap_or_default());
            }
        }
    }
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        let mut state = self.mount.state();
        // Dropping the queues' senders ends every listener once it has read what is queued.
        state.live = None;
        state.title.clear();
        state.artist.clear();
    }
}

/// One listener's place on a mount. Dropping it frees the place.
#[derive(Debug)]
pub struct Listener {
    mount: Arc<Mount>,
    id: u64,
    format: Format,
    queue: Receiver<Arc<[u8]>>,
    /// Set by the mount when it drops this listener for falling behind.
    dropped: Arc<AtomicBool>,
}

impl Listener {
    /// The format of the source this listener joined.
    pub fn format(&self) -> Format {
        self.format
    }

    /// The next page or frame, waiting at most `timeout`. Once the mount has dropped this
    /// listener for falling behind, [`Received::Ended`] at once: what is still queued is
    /// not worth a slow client's time.
    pub fn recv(&self, timeout: Duration) -> Received {
        if self.dropped.load(Ordering::Acquire) {
            return Received::Ended;
        }
        match self.queue.recv_timeout(timeout) {
            Ok(chunk) => Received::Chunk(chunk),
            Err(RecvTimeoutError::Timeout) => Received::Timeout,
            Err(RecvTimeoutError::Disconnected) => Received::Ended,
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let mut state = self.mount.state();
        if let Some(live) = state.live.as_mut() {
            live.listeners.retain(|slot| slot.id != self.id);
        }
    }
}
