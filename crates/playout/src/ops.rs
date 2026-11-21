//! Operator intents from the engine, the operating modes (spec §6.15) and the answer to
//! each command once the mixer confirms it.

use crate::Reply;
use crate::alarm::Alarm;
use crate::core::{CUT_FADE, Core, Phase, Slot};
use crate::view::DeckRole;
use crate::work::Done;
use jiff::Timestamp;
use radio_audio::{Clip, Command as Cmd, CommandKind, DeckId, DeckState};
use radio_engine::{AudioIntent, Mode, Outcome};
use radio_model::{FixMode, Item, ItemId, Source};
use radio_scheduler::{Entry, Origin};
use std::time::{Duration, Instant};

/// What must be true before a command counts as applied to audio.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Check {
    /// Known at once.
    Now(bool),
    /// The item with this tag started on the deck.
    Started { deck: usize, tag: u64 },
    /// The mixer took the load with this tag.
    Loaded { deck: usize, tag: u64 },
    /// The mixer shows this state for the deck.
    State { deck: usize, state: DeckState },
    /// The mixer rendered past this frame, so it applied a command due there.
    Passed(u64),
    /// The item with this tag began to fade out, or left the deck.
    Fading { deck: usize, tag: u64 },
    /// The item with this tag left the deck.
    Gone { deck: usize, tag: u64 },
}

pub(crate) struct Pending {
    reply: Reply,
    outcome: Outcome,
    check: Check,
    deadline: Instant,
}

/// The cartwall clips in the mixer. The workers decode them.
pub(crate) struct Carts {
    loaded: Vec<Option<ItemId>>,
    asked: Vec<Option<ItemId>>,
}

impl Carts {
    pub(crate) fn new(slots: usize) -> Self {
        Self {
            loaded: vec![None; slots],
            asked: vec![None; slots],
        }
    }
}

impl Core {
    /// Answers a command now, or when its check passes.
    pub(crate) fn answer(&mut self, outcome: Outcome, mut checks: Vec<Check>, reply: Reply) {
        let Some(check) = checks.pop().filter(|_| outcome.audio_required) else {
            let _ = reply.send(Ok(outcome));
            return;
        };
        self.pending.push(Pending {
            reply,
            outcome,
            check,
            deadline: Instant::now()
                .checked_add(self.cfg.confirm_timeout)
                .unwrap_or_else(Instant::now),
        });
        self.expire_replies();
    }

    /// Sends the answers whose check passed or whose time ran out.
    pub(crate) fn expire_replies(&mut self) {
        let now = Instant::now();
        for mut p in std::mem::take(&mut self.pending) {
            let applied = match self.confirmed(p.check) {
                Some(ok) => ok,
                None if now >= p.deadline => false,
                None => {
                    self.pending.push(p);
                    continue;
                }
            };
            p.outcome.applied_to_audio = applied;
            let _ = p.reply.send(Ok(p.outcome));
        }
    }

    fn confirmed(&self, check: Check) -> Option<bool> {
        let slot = |deck: usize, tag: u64| {
            self.slots
                .get(deck)
                .and_then(Option::as_ref)
                .filter(|s| s.tag == tag)
        };
        let yes = match check {
            Check::Now(ok) => return Some(ok),
            Check::Started { deck, tag } => slot(deck, tag).is_some_and(|s| s.started.is_some()),
            Check::Loaded { deck, tag } => {
                slot(deck, tag).is_some_and(|s| !matches!(s.phase, Phase::Opening | Phase::Loading))
            }
            Check::State { deck, state } => self.mixer.deck_state(DeckId::ALL[deck]) == state,
            Check::Passed(at) => self.mixer.now() > at,
            Check::Fading { deck, tag } => slot(deck, tag).is_none_or(|s| s.fading),
            Check::Gone { deck, tag } => slot(deck, tag).is_none(),
        };
        yes.then_some(true)
    }

    /// Turns one intent into mixer commands, and returns what confirms it.
    pub(crate) fn intent(&mut self, intent: AudioIntent, now: Timestamp) -> Check {
        let deck = |d: u8| usize::from(d).min(3);
        match intent {
            AudioIntent::Load { deck: d, item } => self.op_load(deck(d), item, now),
            AudioIntent::Play { deck: d } => {
                self.leave_emergency(now);
                self.op_play(deck(d))
            }
            AudioIntent::Pause { deck: d } => self.op_pause(deck(d)),
            AudioIntent::Fade { deck: d } => self.op_fade(deck(d)),
            AudioIntent::Skip { deck: d } => self.op_skip(deck(d), now),
            AudioIntent::SetMode(_) => {
                self.leave_emergency(now);
                self.dirty = true;
                Check::Now(true)
            }
            AudioIntent::StopAfterCurrent => {
                self.hold = true;
                Check::Now(true)
            }
            AudioIntent::Next => {
                self.leave_emergency(now);
                self.hold = false;
                self.start_next(None)
            }
            AudioIntent::FireCart { page, slot } => self.op_cart(page, slot),
            AudioIntent::StopAllCarts => {
                let at = self.mixer.now();
                match self.mixer.send(Cmd::new(at, CommandKind::StopCarts)) {
                    Ok(()) => Check::Passed(at),
                    Err(_) => Check::Now(false),
                }
            }
        }
    }

    /// Tells if this queued slot can start by itself now. Hard events always can.
    pub(crate) fn may_start(&self, s: &Slot) -> bool {
        if s.entry
            .fixed
            .as_ref()
            .is_some_and(|f| f.mode == FixMode::Hard)
        {
            return true;
        }
        // A paused item holds back the item after it.
        let paused = self
            .slots
            .iter()
            .flatten()
            .any(|o| o.phase == Phase::Paused);
        !paused && !self.hold && !self.dead_air.emergency && self.engine.state().mode == Mode::Auto
    }

    /// The item with this id in the station, and the id of its playlist.
    fn station_item(&self, id: ItemId) -> Option<(Item, radio_model::PlaylistId)> {
        self.engine.state().station.playlists.iter().find_map(|p| {
            p.items
                .iter()
                .find(|i| i.id == id)
                .map(|i| (i.clone(), p.id))
        })
    }

    /// Takes a deck item off the deck at once.
    fn unload(&mut self, i: usize) {
        if let Some(s) = self.slots[i].take() {
            let cmd = Cmd::new(self.mixer.now(), CommandKind::Stop(DeckId::ALL[i]));
            let _ = self.mixer.send(cmd.for_item(s.tag));
            self.dirty = true;
        }
    }

    fn op_load(&mut self, i: usize, id: ItemId, now: Timestamp) -> Check {
        if self.slots[i].as_ref().is_some_and(|s| !s.not_started()) {
            return Check::Now(false);
        }
        let Some((item, playlist)) = self.station_item(id) else {
            return Check::Now(false);
        };
        self.unload(i);
        let entry = Entry {
            playlist,
            item: item.id,
            start: now,
            len: radio_timing::item_len(&item),
            origin: Origin::Manual,
            locked: true,
            fixed: None,
            trim: Duration::ZERO,
        };
        match self.load(i, entry, item.clone(), DeckRole::Operator, Duration::ZERO) {
            Ok(tag) => Check::Loaded { deck: i, tag },
            Err(reason) => {
                let alarm = Alarm::ItemSkipped {
                    item: item.id,
                    title: item.title,
                    reason,
                };
                self.alarms.raise(now, alarm);
                Check::Now(false)
            }
        }
    }

    fn op_play(&mut self, i: usize) -> Check {
        self.hold = false;
        let Some(s) = &self.slots[i] else {
            return Check::Now(false);
        };
        let tag = s.tag;
        match s.phase {
            Phase::Opening | Phase::Loading | Phase::Ready => {
                let at = self.mixer.now();
                self.play_at(i, at);
                Check::Started { deck: i, tag }
            }
            Phase::Cued(_) => Check::Started { deck: i, tag },
            Phase::Paused => {
                let cmd = Cmd::new(self.mixer.now(), CommandKind::Play(DeckId::ALL[i]));
                match self.mixer.send(cmd.for_item(tag)) {
                    Ok(()) => Check::State {
                        deck: i,
                        state: DeckState::Playing,
                    },
                    Err(_) => Check::Now(false),
                }
            }
            Phase::Playing => Check::Now(true),
        }
    }

    fn op_pause(&mut self, i: usize) -> Check {
        let Some(s) = &self.slots[i] else {
            return Check::Now(false);
        };
        if s.phase != Phase::Playing {
            return Check::Now(false);
        }
        let cmd = Cmd::new(self.mixer.now(), CommandKind::Pause(DeckId::ALL[i]));
        match self.mixer.send(cmd.for_item(s.tag)) {
            Ok(()) => Check::State {
                deck: i,
                state: DeckState::Paused,
            },
            Err(_) => Check::Now(false),
        }
    }

    /// The fade of an item that ends early: its own fade-out, or [`CUT_FADE`].
    fn early_fade(s: &Slot) -> Duration {
        let own = s
            .item
            .timing
            .as_ref()
            .map_or(Duration::ZERO, |t| t.fade_out);
        if own.is_zero() { CUT_FADE } else { own }
    }

    fn op_fade(&mut self, i: usize) -> Check {
        let Some(s) = &self.slots[i] else {
            return Check::Now(false);
        };
        let (over, tag) = (Self::early_fade(s), s.tag);
        let at = self.mixer.now();
        self.fade_at(i, at, over, true);
        Check::Fading { deck: i, tag }
    }

    fn op_skip(&mut self, i: usize, _now: Timestamp) -> Check {
        let Some(s) = &self.slots[i] else {
            return Check::Now(false);
        };
        let on_air_plan = s.role == DeckRole::Plan && !s.not_started();
        let tag = s.tag;
        let at = self.mixer.now();
        if s.not_started() {
            self.unload(i);
        } else {
            self.fade_at(i, at, Duration::ZERO, true);
        }
        if on_air_plan && !self.hold && self.engine.state().mode == Mode::Auto {
            let _ = self.start_next(Some(i));
        }
        Check::Gone { deck: i, tag }
    }

    /// Starts the earliest queued plan entry now, and fades out what plays. `except` is
    /// a deck that ends already.
    pub(crate) fn start_next(&mut self, except: Option<usize>) -> Check {
        let next = (0..4)
            .filter(|&i| Some(i) != except)
            .filter_map(|i| self.slots[i].as_ref().map(|s| (i, s)))
            .filter(|(_, s)| s.role == DeckRole::Plan && s.not_started())
            .min_by_key(|(_, s)| s.entry.start)
            .map(|(i, s)| (i, s.tag));
        let Some((j, tag)) = next else {
            return Check::Now(false);
        };
        let at = self.mixer.now();
        for i in (0..4).filter(|&i| i != j && Some(i) != except) {
            let fade = self.slots[i]
                .as_ref()
                .filter(|s| !s.not_started() && !s.end_sent && s.role != DeckRole::Bed)
                .map(Self::early_fade);
            if let Some(over) = fade {
                self.fade_at(i, at, over, true);
            }
        }
        self.play_at(j, at);
        Check::Started { deck: j, tag }
    }

    /// Follows pause and continue in the mixer.
    pub(crate) fn poll_states(&mut self) {
        for i in 0..4 {
            let state = self.mixer.deck_state(DeckId::ALL[i]);
            if let Some(s) = &mut self.slots[i] {
                match (s.phase, state) {
                    (Phase::Playing, DeckState::Paused) => {
                        s.phase = Phase::Paused;
                        // A pause moves the planned end; the item now plays to its file end.
                        s.end_sent = true;
                    }
                    (Phase::Paused, DeckState::Playing) => s.phase = Phase::Playing,
                    _ => {}
                }
            }
        }
    }

    /// Takes the result of a worker that decoded the clip for cart slot `i`.
    pub(crate) fn on_cart(
        &mut self,
        i: usize,
        item: ItemId,
        result: Result<Clip, String>,
        now: Timestamp,
    ) {
        if self.carts.asked.get(i) == Some(&Some(item)) {
            match result {
                Ok(clip) => match self.mixer.set_cart(i, Some(clip)) {
                    Ok(()) => self.carts.loaded[i] = Some(item),
                    Err(_) => self.carts.asked[i] = None,
                },
                Err(reason) => {
                    let title = self.station_item(item).map(|(it, _)| it.title);
                    let title = title.unwrap_or_default();
                    self.alarms.raise(
                        now,
                        Alarm::ItemSkipped {
                            item,
                            title,
                            reason,
                        },
                    );
                }
            }
        }
    }

    /// Keeps the cartwall clips in the mixer in step with the engine's assignments.
    pub(crate) fn sync_carts(&mut self) {
        let wanted: Vec<ItemId> = self.engine.state().carts.iter().map(|c| c.item).collect();
        for i in 0..self.carts.loaded.len() {
            let want = wanted.get(i).copied();
            if self.carts.asked[i] == want {
                continue;
            }
            self.carts.asked[i] = want;
            self.carts.loaded[i] = None;
            let _ = self.mixer.set_cart(i, None);
            let path = want.and_then(|id| match self.station_item(id) {
                Some((
                    Item {
                        source: Source::File { path },
                        ..
                    },
                    _,
                )) => Some((id, path)),
                _ => None,
            });
            if let Some((id, path)) = path {
                self.work.run(move || {
                    Done::Cart(i, id, Clip::from_file(&path).map_err(|e| e.to_string()))
                });
            }
        }
    }

    fn op_cart(&mut self, page: u32, slot: u32) -> Check {
        let state = self.engine.state();
        let Some(i) = state
            .carts
            .iter()
            .position(|c| c.page == page && c.slot == slot)
        else {
            return Check::Now(false);
        };
        let item = state.carts[i].item;
        if self.carts.loaded.get(i) != Some(&Some(item)) {
            return Check::Now(false);
        }
        let at = self.mixer.now();
        match self
            .mixer
            .send(Cmd::new(at, CommandKind::FireCart { slot: i }))
        {
            Ok(()) => Check::Passed(at),
            Err(_) => Check::Now(false),
        }
    }
}
