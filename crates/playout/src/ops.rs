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
            AudioIntent::Play {
                deck: d,
                expect_item,
                ..
            } if !self.holds(deck(d), expect_item) => Check::Now(false),
            AudioIntent::Pause {
                deck: d,
                expect_item,
            }
            | AudioIntent::Fade {
                deck: d,
                expect_item,
                ..
            }
            | AudioIntent::Skip {
                deck: d,
                expect_item,
            } if !self.holds(deck(d), expect_item) => Check::Now(false),
            AudioIntent::Play {
                deck: d, fade_in, ..
            } => {
                self.leave_emergency(now);
                self.op_play(deck(d), fade_in, now)
            }
            AudioIntent::Pause { deck: d, .. } => self.op_pause(deck(d)),
            AudioIntent::Fade { deck: d, over, .. } => self.op_fade(deck(d), over),
            AudioIntent::Skip { deck: d, .. } => self.op_skip(deck(d), now),
            AudioIntent::SetMode(_) => {
                self.leave_emergency(now);
                self.dirty = true;
                Check::Now(true)
            }
            AudioIntent::StopAfterCurrent => {
                self.hold = true;
                Check::Now(true)
            }
            AudioIntent::Continue => {
                self.hold = false;
                self.dirty = true;
                Check::Now(true)
            }
            AudioIntent::SetStrip {
                strip,
                gain_db,
                mute,
                pfl,
            } => {
                let Some(strip) = strip_of(&strip) else {
                    return Check::Now(false);
                };
                self.mixer
                    .set_pfl(strip, pfl && !self.cue_device().is_empty());
                let db = if mute { f32::NEG_INFINITY } else { gain_db };
                let at = self.mixer.now();
                let cmd = Cmd::new(at, CommandKind::SetGain { strip, db });
                Check::Now(self.mixer.send(cmd).is_ok())
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

    /// Tells if deck `i` holds `expect`, the item that a command meant. With no
    /// expectation, any item matches. A stale command does nothing.
    fn holds(&self, i: usize, expect: Option<ItemId>) -> bool {
        expect.is_none_or(|id| self.slots[i].as_ref().is_some_and(|s| s.item.id == id))
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
    pub(crate) fn unload(&mut self, i: usize) {
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
        let fixed_entry = self
            .timeline
            .entries
            .iter()
            .find(|e| e.item == id && e.fixed.as_ref().is_some_and(|f| f.target >= now))
            .cloned();
        if let Some(entry) = &fixed_entry {
            let duplicates: Vec<_> = (0..4)
                .filter(|&k| k != i)
                .filter(|&k| {
                    self.slots[k].as_ref().is_some_and(|s| {
                        s.not_started()
                            && s.entry.item == entry.item
                            && s.entry.fixed == entry.fixed
                    })
                })
                .collect();
            for k in duplicates {
                self.unload(k);
            }
        }
        let entry = fixed_entry.unwrap_or(Entry {
            playlist,
            item: item.id,
            start: now,
            len: radio_timing::item_len(&item),
            origin: Origin::Manual,
            locked: true,
            fixed: None,
            trim: Duration::ZERO,
        });
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

    /// Plays deck `i`. A deck that has not started fades in over `fade_in`. The item on air
    /// fades out at the same time: a crossfade. The plan goes on from this deck.
    fn op_play(&mut self, i: usize, fade_in: Duration, now: Timestamp) -> Check {
        self.hold = false;
        let Some(s) = &self.slots[i] else {
            return Check::Now(false);
        };
        if s.role == DeckRole::Plan
            && s.is_hard()
            && s.entry.fixed.as_ref().is_some_and(|f| now < f.target)
        {
            self.alarms.raise_once(
                now,
                Alarm::Unsupported {
                    what: "This hard event waits for its target time".into(),
                },
            );
            return Check::Now(false);
        }
        let tag = s.tag;
        match s.phase {
            Phase::Opening | Phase::Loading | Phase::Ready => {
                let at = self.mixer.now();
                self.crossfade_from_air(i, at, fade_in);
                self.skip_over(i);
                if let Some(s) = &mut self.slots[i] {
                    // The deck starts now, so the plan goes on from it.
                    if !s.is_hard() {
                        s.entry.start = now;
                    }
                    // A file that a worker still opens takes the fade with its load.
                    s.options.fade_in = s.options.fade_in.max(fade_in);
                }
                self.play_fading_at(i, at, fade_in);
                self.dirty = true;
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

    /// Fades out each item on air but deck `i`, from frame `at`. The fade has the length of
    /// the fade-in of deck `i`. With no fade-in, it is the early fade of the item. A paused
    /// item ends at once. Thus it does not hold back the plan.
    fn crossfade_from_air(&mut self, i: usize, at: u64, fade_in: Duration) {
        for k in (0..4).filter(|&k| k != i) {
            let fade = self.slots[k]
                .as_ref()
                .filter(|s| !s.not_started() && s.role != DeckRole::Bed)
                .filter(|s| !s.end_sent || s.phase == Phase::Paused)
                .map(|s| {
                    if fade_in.is_zero() {
                        Self::early_fade(s)
                    } else {
                        fade_in
                    }
                });
            if let Some(over) = fade {
                self.fade_at(k, at, over, true);
            }
        }
    }

    /// Deck `i` plays before its turn. The plan items that waited before it leave their
    /// decks. They return to the rotation and play at a later turn. Hard events and play-next
    /// items stay.
    fn skip_over(&mut self, i: usize) {
        let Some(start) = self.slots[i].as_ref().map(|s| s.entry.start) else {
            return;
        };
        let queue = self.engine.state().queue.clone();
        for k in (0..4).filter(|&k| k != i) {
            let skipped = self.slots[k].as_ref().is_some_and(|s| {
                s.role == DeckRole::Plan
                    && s.not_started()
                    && !s.is_hard()
                    && !queue.contains(&s.item.id)
                    && s.entry.start <= start
            });
            if skipped {
                self.unload(k);
            }
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

    fn op_fade(&mut self, i: usize, over: Duration) -> Check {
        let Some(s) = &self.slots[i] else {
            return Check::Now(false);
        };
        let tag = s.tag;
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
            // An operator skip is the one way to cancel a queued hard event.
            if s.is_hard() {
                let entry = s.entry.clone();
                self.cancel_hard(&entry);
            }
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
        let now = self.clock.now();
        let next = (0..4)
            .filter(|&i| Some(i) != except)
            .filter_map(|i| self.slots[i].as_ref().map(|s| (i, s)))
            .filter(|(_, s)| s.role == DeckRole::Plan && s.not_started())
            .filter(|(_, s)| !s.is_hard() || s.entry.fixed.as_ref().is_none_or(|f| now >= f.target))
            .min_by_key(|(_, s)| s.entry.start)
            .map(|(i, s)| (i, s.tag));
        let Some((j, tag)) = next else {
            if self
                .slots
                .iter()
                .flatten()
                .any(|s| s.is_hard() && s.not_started())
            {
                self.alarms.raise_once(
                    now,
                    Alarm::Unsupported {
                        what: "The next hard event waits for its target time".into(),
                    },
                );
            }
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
    /// Sends the stored fader and mute of each mixer strip to the mixer.
    pub(crate) fn cue_device(&self) -> &str {
        self.engine
            .state()
            .settings("audio")
            .and_then(|s| s.get("pfl"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
    }

    pub(crate) fn sync_strips(&mut self) {
        let strips: Vec<(String, f32, bool, bool)> = self
            .engine
            .state()
            .strips
            .iter()
            .map(|(k, s)| {
                (
                    k.clone(),
                    s.gain_db,
                    s.mute || s.route == radio_engine::Route::CueOnly,
                    s.pfl || s.route != radio_engine::Route::Out,
                )
            })
            .collect();
        for (name, gain, mute, pfl) in strips {
            if let Some(strip) = strip_of(&name) {
                self.mixer
                    .set_pfl(strip, pfl && !self.cue_device().is_empty());
                let db = if mute { f32::NEG_INFINITY } else { gain };
                let cmd = Cmd::new(self.mixer.now(), CommandKind::SetGain { strip, db });
                let _ = self.mixer.send(cmd);
            }
        }
    }

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

/// The mixer strip of a strip name.
fn strip_of(name: &str) -> Option<radio_audio::Strip> {
    use radio_audio::{DeckId, Strip};
    Some(match name {
        "A" => Strip::Deck(DeckId::A),
        "B" => Strip::Deck(DeckId::B),
        "C" => Strip::Deck(DeckId::C),
        "D" => Strip::Deck(DeckId::D),
        "Cart" => Strip::Cart,
        "Live" => Strip::Live,
        "Out" => Strip::Out,
        _ => return None,
    })
}
