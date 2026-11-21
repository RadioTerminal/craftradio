//! Alarms: raised on the control thread, sent to subscribers and to an optional webhook.

use jiff::Timestamp;
use radio_model::ItemId;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::time::Duration;

/// Something an operator should know about (spec §6.14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Alarm {
    /// Playout could not load an item (missing or broken file). The plan skips it before air.
    ItemSkipped {
        item: ItemId,
        title: String,
        reason: String,
    },
    /// Program went silent; playout skipped to the next item.
    DeadAir,
    /// Skipping did not help: the emergency fallback folder is playing.
    EmergencyFallback { reason: String },
    /// A keep-alive stream on air has no audio and reconnects. A bed plays under it.
    StreamReconnecting { item: ItemId },
    /// A stream without keep-alive had no audio past the timeout. Playout skipped it.
    StreamDown { item: ItemId },
    /// A file deck ran out of decoded audio while on air.
    DeckUnderrun { deck: u8 },
    /// The wall clock and the sample clock stepped apart; the estimates re-anchored.
    ClockStep { by_ms: i64 },
    /// A hard event is near, and each deck holds an item on air, so playout could not
    /// load the event.
    HardEventNotLoaded { item: ItemId },
    /// A hard event passed its target and its tolerance without a deck, so it did not air.
    HardEventMissed { item: ItemId, target: Timestamp },
    /// Playout took an operator's loaded item off its deck to make room for a hard event.
    OperatorItemEvicted { deck: u8, item: ItemId },
    /// A hard event is about to air, and a station change moves or deletes its rule. The
    /// event still airs, and the change applies after it: the occurrence it moved to does
    /// not air.
    HardEventLocked { item: ItemId, target: Timestamp },
    /// An operator removed the item of a hard event that was about to air, so it does not
    /// air.
    HardEventCancelled { item: ItemId, target: Timestamp },
    /// Playout could not use the local output as asked. It runs on the fallback.
    OutputFallback { reason: String },
    /// Writing the as-played log failed.
    LogFailed { reason: String },
    /// An item or command needs something this build does not do yet.
    Unsupported { what: String },
}

/// One raised alarm, as the snapshot shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RaisedAlarm {
    pub at: Timestamp,
    pub alarm: Alarm,
    /// When the condition went away, for alarms that describe a lasting condition.
    pub cleared: Option<Timestamp>,
}

const RECENT: usize = 50;

pub(crate) struct Alarms {
    subscribers: Vec<SyncSender<RaisedAlarm>>,
    /// Alarms that a full subscriber channel could not take.
    pub(crate) dropped: u64,
    webhook: Option<SyncSender<String>>,
    pub(crate) recent: VecDeque<RaisedAlarm>,
}

impl Alarms {
    pub(crate) fn new(webhook: Option<String>) -> Self {
        Self {
            subscribers: Vec::new(),
            dropped: 0,
            webhook: webhook.and_then(spawn_webhook),
            recent: VecDeque::new(),
        }
    }

    pub(crate) fn subscribe(&mut self, tx: SyncSender<RaisedAlarm>) {
        self.subscribers.push(tx);
    }

    /// Raises `alarm` if the same condition is not raised and still uncleared.
    pub(crate) fn raise_once(&mut self, at: Timestamp, alarm: Alarm) {
        let active = self
            .recent
            .iter()
            .any(|r| r.alarm == alarm && r.cleared.is_none());
        if !active {
            self.raise(at, alarm);
        }
    }

    /// Marks each alarm that matches `pred` and is not cleared yet as cleared.
    pub(crate) fn clear(&mut self, at: Timestamp, pred: impl Fn(&Alarm) -> bool) {
        for r in self.recent.iter_mut().filter(|r| r.cleared.is_none()) {
            if pred(&r.alarm) {
                r.cleared = Some(at);
            }
        }
    }

    pub(crate) fn raise(&mut self, at: Timestamp, alarm: Alarm) {
        let raised = RaisedAlarm {
            at,
            alarm,
            cleared: None,
        };
        let mut dropped = 0;
        self.subscribers
            .retain(|s| match s.try_send(raised.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    dropped += 1;
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            });
        self.dropped += dropped;
        if let Some(tx) = &self.webhook
            && let Ok(body) = serde_json::to_string(&raised)
            && let Err(TrySendError::Disconnected(_)) = tx.try_send(body)
        {
            self.webhook = None;
        }
        if self.recent.len() >= RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(raised);
    }
}

/// A thread that POSTs each alarm as JSON. Best effort: a slow or failing hook drops alarms,
/// and playout never waits for it.
fn spawn_webhook(url: String) -> Option<SyncSender<String>> {
    let (tx, rx) = sync_channel::<String>(64);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    std::thread::Builder::new()
        .name("playout-webhook".into())
        .spawn(move || {
            for body in rx {
                let _ = agent
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .send(body.as_str());
            }
        })
        .ok()?;
    Some(tx)
}
