//! Internet streams on decks (spec §6.4), keep-alive beds, and the stream title on Icecast.

use crate::alarm::Alarm;
use crate::core::{CUT_FADE, Core, Phase, since};
use crate::view::{DeckRole, StreamState};
use jiff::Timestamp;
use radio_audio::{CHANNELS, DeckId, LoadOptions, SAMPLE_RATE};
use radio_engine::Mode;
use radio_model::{Item, Source, stream_title};
use radio_scheduler::{Entry, Origin};
use radio_stream::{StreamInput, StreamInputHandle};
use std::time::Duration;

/// The ring of a stream deck holds this much audio.
const STREAM_RING: Duration = Duration::from_secs(5);
/// A read that brings no audio for this long counts as a stall, and the input reconnects.
const STALL: Duration = Duration::from_secs(3);

/// An internet stream on a deck.
pub(crate) struct StreamSlot {
    handle: StreamInputHandle,
    pub(crate) url: String,
    pub(crate) keep_alive: bool,
    /// The deck ran dry while on air at this time.
    pub(crate) down_since: Option<Timestamp>,
    /// The deck of the bed under a keep-alive stream that has no audio.
    pub(crate) bed: Option<(usize, u64)>,
}

impl Core {
    /// Connects a stream and puts its ring on deck `deck`.
    pub(crate) fn load_stream(
        &mut self,
        deck: DeckId,
        url: &str,
        keep_alive: bool,
        options: LoadOptions,
    ) -> Result<StreamSlot, String> {
        let frames = STREAM_RING.as_secs() as usize * SAMPLE_RATE as usize;
        let (tx, rx) = rtrb::RingBuffer::new(frames * CHANNELS);
        let handle = StreamInput::open_with_timeouts(url, tx, self.cfg.stream_timeout, STALL)
            .map_err(|e| e.to_string())?;
        self.mixer
            .load_ring(deck, rx, options)
            .map_err(|e| e.to_string())?;
        Ok(StreamSlot {
            handle,
            url: url.to_string(),
            keep_alive,
            down_since: None,
            bed: None,
        })
    }

    /// Watches each stream on air. A keep-alive stream without audio keeps its place
    /// and gets a bed under it. Other streams end after the stream timeout.
    pub(crate) fn check_streams(&mut self, now: Timestamp) {
        for i in 0..4 {
            let Some(s) = &self.slots[i] else { continue };
            let Some(st) = &s.stream else { continue };
            // A stream that ends already needs no more checks.
            if s.phase != Phase::Playing || s.end_sent {
                continue;
            }
            let item = s.item.id;
            let reason = st.handle.last_error();
            let down = self.mixer.underrun(DeckId::ALL[i]);
            let (keep_alive, down_since, bed) = (st.keep_alive, st.down_since, st.bed);
            if !down {
                if down_since.is_some() {
                    self.set_stream(i, None, None);
                    self.alarms.clear(
                        now,
                        |a| matches!(a, Alarm::StreamReconnecting { item: x, .. } if *x == item),
                    );
                    if let Some((deck, tag)) = bed {
                        self.end_bed(deck, tag);
                    }
                }
                continue;
            }
            let since_down = down_since.unwrap_or(now);
            if down_since.is_none() {
                self.set_stream(i, Some(now), bed);
            }
            if keep_alive {
                // The reason often comes after the alarm. A new reason replaces the alarm.
                let same = |a: &Alarm| matches!(a, Alarm::StreamReconnecting { item: x, .. } if *x == item);
                let alarm = Alarm::StreamReconnecting { item, reason };
                let active = self
                    .alarms
                    .recent
                    .iter()
                    .find(|r| r.cleared.is_none() && same(&r.alarm))
                    .map(|r| r.alarm.clone());
                let newer = matches!(
                    &alarm,
                    Alarm::StreamReconnecting {
                        reason: Some(_),
                        ..
                    }
                );
                if active.is_none() || (newer && active.as_ref() != Some(&alarm)) {
                    self.alarms.clear(now, same);
                    self.alarms.raise(now, alarm);
                }
                let bed_on = bed
                    .is_some_and(|(d, tag)| self.slots[d].as_ref().is_some_and(|b| b.tag == tag));
                if !bed_on {
                    let bed = self.start_bed(now);
                    self.set_stream(i, Some(since_down), bed);
                }
            } else if since(since_down, now) >= self.cfg.stream_timeout {
                self.alarms.raise(now, Alarm::StreamDown { item, reason });
                let at = self.mixer.now();
                self.fade_at(i, at, Duration::ZERO, true);
                if !self.hold && self.engine.state().mode == Mode::Auto {
                    let _ = self.start_next(Some(i));
                }
            }
        }
    }

    fn set_stream(&mut self, i: usize, down: Option<Timestamp>, bed: Option<(usize, u64)>) {
        if let Some(st) = self.slots[i].as_mut().and_then(|s| s.stream.as_mut()) {
            st.down_since = down;
            st.bed = bed;
        }
    }

    /// Ends the bed on deck `deck` if it still holds the load with `tag`.
    pub(crate) fn end_bed(&mut self, deck: usize, tag: u64) {
        if self.slots[deck].as_ref().is_some_and(|b| b.tag == tag) {
            let at = self.mixer.now();
            self.fade_at(deck, at, CUT_FADE, true);
        }
    }

    /// Plays the next file of the first fill pool on a free deck, and returns that deck.
    fn start_bed(&mut self, now: Timestamp) -> Option<(usize, u64)> {
        let beds: Vec<Item> = self
            .engine
            .state()
            .station
            .playlists
            .iter()
            .filter(|p| p.enabled && p.is_fill_pool)
            .flat_map(|p| p.items.iter())
            .filter(|i| matches!(i.source, Source::File { .. }))
            .cloned()
            .collect();
        if beds.is_empty() {
            return None;
        }
        let item = beds[self.next_bed % beds.len()].clone();
        self.next_bed += 1;
        let i = self.free_deck()?;
        let entry = Entry {
            playlist: radio_model::PlaylistId(0),
            item: item.id,
            start: now,
            len: radio_timing::item_len(&item),
            origin: Origin::Fill,
            locked: true,
            fixed: None,
            trim: Duration::ZERO,
        };
        let tag = self
            .load(i, entry, item, DeckRole::Bed, Duration::ZERO)
            .ok()?;
        let at = self.mixer.now();
        self.play_at(i, at);
        Some((i, tag))
    }

    /// Sends the stream title for an item that just started.
    pub(crate) fn send_title(&mut self, item: &Item) {
        let Some(ice) = &self.icecast else { return };
        let mut shown = item.clone();
        if shown.meta.title.is_empty() {
            shown.meta.title.clone_from(&item.title);
            shown.meta.artist = item.artist.clone().unwrap_or_default();
        }
        let title = stream_title(&shown, &self.title, &self.cfg.station_name);
        ice.set_title(title.clone());
        self.title = title;
    }

    pub(crate) fn stream_states(&self) -> Vec<StreamState> {
        (0..4)
            .filter_map(|i| {
                let s = self.slots[i].as_ref()?;
                let st = s.stream.as_ref()?;
                Some(StreamState {
                    deck: i as u8,
                    item: s.item.id,
                    url: st.url.clone(),
                    keep_alive: st.keep_alive,
                    up: s.phase == Phase::Playing && st.down_since.is_none(),
                    down_since: st.down_since,
                    icy_title: st.handle.icy_title(),
                })
            })
            .collect()
    }
}
