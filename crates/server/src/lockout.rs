//! Wrong-key lockout: five wrong keys from one address within a minute lock that address
//! out of every keyed endpoint for five minutes.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Wrong keys from one address, inside [`WINDOW`], that lock it.
const MAX_FAILURES: usize = 5;
/// How far back wrong keys count.
const WINDOW: Duration = Duration::from_secs(60);
/// How long a locked address stays locked.
const LOCK_FOR: Duration = Duration::from_secs(300);

#[derive(Debug, Default)]
struct Entry {
    /// Wrong keys inside the window, oldest first.
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

impl Entry {
    fn forget_before(&mut self, now: Instant) {
        while self
            .failures
            .front()
            .is_some_and(|&at| now.saturating_duration_since(at) >= WINDOW)
        {
            self.failures.pop_front();
        }
    }

    fn locked(&self, now: Instant) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }

    /// Nothing left to remember: no failure inside the window and no lock in force.
    fn idle(&self, now: Instant) -> bool {
        !self.locked(now)
            && self
                .failures
                .back()
                .is_none_or(|&at| now.saturating_duration_since(at) >= WINDOW)
    }
}

/// Counts wrong keys per address and says which addresses are locked out.
///
/// Time is passed in, so callers (and tests) choose the clock.
#[derive(Debug, Default)]
pub struct Lockout {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<IpAddr, Entry>,
    /// When idle entries were last dropped; sweeping once a window keeps `failed` cheap
    /// while many addresses fail at once.
    swept_at: Option<Instant>,
}

impl Inner {
    fn sweep(&mut self, now: Instant) {
        let due = self
            .swept_at
            .is_none_or(|at| now.saturating_duration_since(at) >= WINDOW);
        if due {
            self.entries.retain(|_, entry| !entry.idle(now));
            self.swept_at = Some(now);
        }
    }
}

impl Lockout {
    /// Whether `addr` is locked out at `now`.
    pub fn blocked(&self, addr: IpAddr, now: Instant) -> bool {
        self.inner()
            .entries
            .get(&addr)
            .is_some_and(|entry| entry.locked(now))
    }

    /// Records a wrong key from `addr` at `now`; the fifth inside a minute locks it.
    pub fn failed(&self, addr: IpAddr, now: Instant) {
        let mut inner = self.inner();
        inner.sweep(now);
        let entry = inner.entries.entry(addr).or_default();
        entry.forget_before(now);
        entry.failures.push_back(now);
        if entry.failures.len() >= MAX_FAILURES {
            entry.failures.clear();
            // An `Instant` this far ahead cannot be represented only on a broken clock;
            // then the address is not locked rather than the server panicking.
            entry.locked_until = now.checked_add(LOCK_FOR);
        }
    }

    /// How many addresses are remembered.
    #[doc(hidden)]
    pub fn tracked(&self) -> usize {
        self.inner().entries.len()
    }

    /// The map, even after a thread panicked while holding it: it only holds counts and
    /// times, which stay consistent, and losing it would lift every lock.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
