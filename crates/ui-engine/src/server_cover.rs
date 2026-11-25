//! The cover of what plays, sent to the paired station server, which shows it on its
//! listener page. Each item on air is sent once, from a lane of its own: a newer item drops
//! an upload that has not gone out yet, so the server ends with the latest. The key goes
//! only to the paired address, with the agent pairing uses.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use jiff::Timestamp;
use radio_stream::StreamStatus;

use crate::pairing::{PairError, PutError, agent, basic, check_address, describe, stored_or};
use crate::{EngineSource, SecretStore};

/// The largest cover the server takes (`PUT /api/station/cover`).
pub const SERVER_MAX_COVER: usize = 1 << 20;

/// Sends `cover` (its type and bytes) to `<address>/api/station/cover` with `key` as the
/// Basic password, or clears the server's cover with an empty body when there is none. An
/// address outside [`check_address`]'s rule is refused before anything is sent.
pub fn put_cover(address: &str, key: &str, cover: Option<(&str, &[u8])>) -> Result<(), PutError> {
    let address = &check_address(address).map_err(PutError::Refused)?;
    let agent = agent(address).map_err(PutError::Unreachable)?;
    let (mime, body) = cover.unwrap_or(("application/octet-stream", &[]));
    // A body of bytes goes with its Content-Length (0 to clear), which the server needs.
    let response = agent
        .put(format!("{address}/api/station/cover"))
        .header("Authorization", basic(key))
        .header("Content-Type", mime)
        .send(body)
        .map_err(|e| PutError::Unreachable(describe(&e)))?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 => Err(PutError::KeyRefused),
        429 => Err(PutError::Unreachable(
            "the server refuses keys from this computer for a while after wrong ones".into(),
        )),
        404 | 405 => Err(PutError::Refused(
            "this server takes no cover; update craftradio-server".into(),
        )),
        code => Err(PutError::Refused(format!(
            "the server refused the cover ({code})"
        ))),
    }
}

/// One cover to send: of the item `seq` names, from `file` (none clears the server's).
pub(crate) struct Upload {
    pub(crate) seq: u64,
    pub(crate) address: String,
    /// The keychain entry of the paired address itself.
    pub(crate) entry: String,
    pub(crate) file: Option<PathBuf>,
    pub(crate) store: Arc<dyn SecretStore + Sync>,
}

/// What the lane hands back for an upload that went out.
pub(crate) struct CoverAnswer {
    pub(crate) address: String,
    pub(crate) result: Result<(), PutError>,
}

#[derive(Default)]
struct Slot {
    next: Option<Upload>,
    /// The newest upload asked for: an older one about to go out is dropped.
    latest: u64,
    closed: bool,
}

/// The cover lane: one thread that sends the newest upload it was given.
pub(crate) struct Lane {
    shared: Arc<(Mutex<Slot>, Condvar)>,
}

impl Lane {
    pub(crate) fn start(done: Sender<crate::bg::Done>) -> Option<Lane> {
        let shared = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let theirs = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("ui-server-cover".into())
            .spawn(move || run(&theirs, &done))
            .ok()?;
        Some(Lane { shared })
    }

    /// Puts `upload` in place of any that has not gone out yet.
    pub(crate) fn send(&self, upload: Upload) {
        let (slot, wake) = &*self.shared;
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.latest = upload.seq;
        slot.next = Some(upload);
        wake.notify_one();
    }
}

impl Drop for Lane {
    fn drop(&mut self) {
        let (slot, wake) = &*self.shared;
        slot.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        wake.notify_one();
    }
}

fn run(shared: &(Mutex<Slot>, Condvar), done: &Sender<crate::bg::Done>) {
    let (slot, wake) = shared;
    loop {
        let upload = {
            let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if guard.closed {
                    return;
                }
                if let Some(upload) = guard.next.take() {
                    break upload;
                }
                guard = wake.wait(guard).unwrap_or_else(PoisonError::into_inner);
            }
        };
        // A picture that cannot be used clears the cover, so the last item's does not stay.
        let cover = upload.file.as_deref().and_then(|f| {
            radio_stream::server_cover(f, SERVER_MAX_COVER)
                .ok()
                .flatten()
        });
        // A newer item came while the file was read: its upload goes instead.
        if slot.lock().unwrap_or_else(PoisonError::into_inner).latest != upload.seq {
            continue;
        }
        let result = match stored_or(None, Some(&upload.entry), upload.store.as_ref()) {
            Ok(key) => put_cover(
                &upload.address,
                key.expose(),
                cover.as_ref().map(|c| (c.mime.as_str(), c.data.as_slice())),
            ),
            Err(PairError::KeyRefused) => Err(PutError::KeyRefused),
            Err(PairError::Refused(reason) | PairError::Unreachable(reason)) => {
                Err(PutError::Refused(reason))
            }
        };
        let answer = CoverAnswer {
            address: upload.address,
            result,
        };
        if done.send(crate::bg::Done::ServerCover(answer)).is_err() {
            return;
        }
    }
}

/// What the source last asked the lane to send, so each item goes once.
#[derive(Default)]
pub(crate) struct CoverSync {
    /// The paired address, the item's revision and when the push connected.
    sent_for: Option<(String, u64, Timestamp)>,
    seq: u64,
}

impl EngineSource {
    /// Sends the cover of the item on air to the paired server when the item changes, or
    /// when the push connects again (the server forgets the cover when its source ends).
    /// Nothing goes while unpaired, off air, with Icecast off or not connected, or while the
    /// push waits for the pairing or a new key.
    pub(crate) fn watch_cover(&mut self, outputs: &radio_playout::OutputStatus) {
        let Some(address) = crate::pairing::paired_address(&self.state) else {
            self.server_cover.sent_for = None;
            return;
        };
        if self.push_held() || !outputs.on_air || !outputs.icecast_enabled {
            return;
        }
        let Some(StreamStatus::Connected { since, .. }) = &outputs.icecast else {
            return;
        };
        let Some(now) = &outputs.now else {
            return;
        };
        let want = (address, now.rev, *since);
        if self.server_cover.sent_for.as_ref() == Some(&want) {
            return;
        }
        let Some(lane) = self.worker.as_ref().and_then(|w| w.covers.as_ref()) else {
            return;
        };
        let address = want.0.clone();
        // One try per item, sent or not.
        self.server_cover.sent_for = Some(want);
        if check_address(&address).is_err() {
            return;
        }
        self.server_cover.seq += 1;
        lane.send(Upload {
            seq: self.server_cover.seq,
            entry: crate::pairing::server_entry(&address),
            address,
            file: now.cover.clone(),
            store: Arc::clone(&self.secrets),
        });
    }

    /// The server's answer to a cover. A refused key latches as a refused pairing does, so
    /// neither the push nor the next cover walks the server's lockout; other failures wait
    /// for the next item.
    pub(crate) fn cover_answer(&mut self, answer: CoverAnswer) {
        if matches!(answer.result, Err(PutError::KeyRefused))
            && crate::pairing::paired_address(&self.state).as_deref()
                == Some(answer.address.as_str())
        {
            self.refuse_key();
        }
    }
}
