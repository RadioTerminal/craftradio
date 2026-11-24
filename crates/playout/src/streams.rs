//! Internet streams on decks (spec §6.4), keep-alive beds, and the stream title on Icecast.

use crate::WEBRTC_PART;
use crate::alarm::Alarm;
use crate::core::{CUT_FADE, Core, Phase, since};
use crate::view::{DeckRole, StreamState};
use jiff::Timestamp;
use radio_audio::{CHANNELS, DeckId, LoadOptions, SAMPLE_RATE};
use radio_engine::Mode;
use radio_model::{Item, PublishMode, Source, stream_title};
use radio_scheduler::{Entry, Origin};
use radio_stream::{Played, StreamInput, StreamInputHandle, WebRtcHandle, WebRtcServer};
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
        let i = self.spare_deck()?;
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

    /// The station name that hidden titles show: the setting, else the configured name.
    fn station_name(&self) -> String {
        self.engine
            .state()
            .station_name()
            .unwrap_or_else(|| self.cfg.station_name.clone())
    }

    /// Sends the stream title for an item that just started.
    pub(crate) fn send_title(&mut self, item: &Item) {
        if self.icecast.is_none() && self.cfg.webrtc.is_none() {
            return;
        }
        let mut shown = item.clone();
        if shown.meta.title.is_empty() {
            shown.meta.title.clone_from(&item.title);
            shown.meta.artist = item.artist.clone().unwrap_or_default();
        }
        let name = self.station_name();
        let title = stream_title(&shown, &self.title, &name);
        if let Some(ice) = &self.icecast {
            ice.set_title(title.clone());
        }
        // Listeners see the tags and the cover only when the item shows its own tags.
        // An item that hides its title stays out of the history too.
        let at = self.cfg.clock.now();
        let now = match shown.meta.publish {
            PublishMode::KeepPrevious => None,
            PublishMode::FromTags => Some(NowPlaying {
                title: shown.meta.title.clone(),
                artist: shown.meta.artist.clone(),
                file: cover_file(item),
                at,
                listed: true,
            }),
            PublishMode::Custom | PublishMode::Hide => Some(NowPlaying {
                title: title.clone(),
                artist: String::new(),
                file: cover_file(item),
                at,
                listed: shown.meta.publish == PublishMode::Custom,
            }),
        };
        if let Some(now) = now {
            if let Some(before) = self.webrtc_now.take()
                && before.listed
                && !before.title.is_empty()
            {
                self.webrtc_played.push_front(Played {
                    title: before.title,
                    artist: before.artist,
                    at: before.at,
                    file: before.file,
                });
            }
            let hour_ago = at
                .checked_sub(jiff::SignedDuration::from_hours(1))
                .unwrap_or(at);
            self.webrtc_played
                .retain(|p| p.at >= hour_ago && p.at <= at);
            self.webrtc_played.truncate(PLAYED_MAX);
            if let Some(w) = &self.webrtc {
                now.send(w, &self.webrtc_played);
            }
            self.webrtc_now = Some(now);
        }
        self.title = title;
    }

    /// Fills the listener history from the as-played log of the last hour. Hidden and deleted items stay out.
    pub(crate) fn seed_played(&mut self) {
        let now = self.cfg.clock.now();
        let hour_ago = now
            .checked_sub(jiff::SignedDuration::from_hours(1))
            .unwrap_or(now);
        let Ok(rows) = self.log.history(hour_ago, PLAYED_MAX as u32 * 4) else {
            return;
        };
        let name = self.station_name();
        let station = &self.engine.state().station;
        for row in rows {
            // A row without an end did not finish, or it is the item that resumes now.
            if row.end.is_none() || row.entry.start > now {
                continue;
            }
            let item = station
                .playlists
                .iter()
                .filter(|p| p.name == row.entry.playlist)
                .flat_map(|p| p.items.iter())
                .find(|i| i.title == row.entry.title && i.artist == row.entry.artist);
            let Some((title, artist)) = item.and_then(|i| listed_title(i, &name)) else {
                continue;
            };
            let file = item.and_then(cover_file);
            self.webrtc_played.push_front(Played {
                title,
                artist,
                at: row.entry.start,
                file,
            });
        }
        self.webrtc_played.truncate(PLAYED_MAX);
    }

    /// New Icecast settings, or none: the source moves to them, or closes and waits.
    pub(crate) fn set_icecast(&mut self, cfg: Option<radio_stream::IcecastConfig>) {
        self.icecast_set_up = cfg.is_some();
        if let Some(ice) = &self.icecast {
            ice.reconfigure(cfg.clone());
        }
        self.cfg.icecast = cfg;
    }

    /// Takes Icecast off air or puts it on air when the On air setting changed. WebRTC
    /// follows the same setting in [`Core::sync_webrtc`].
    pub(crate) fn sync_air(&mut self) {
        let on = on_air(self.engine.state());
        if on == self.on_air {
            return;
        }
        if let Some(ice) = &self.icecast {
            ice.set_on_air(on);
        }
        self.on_air = on;
    }

    /// Starts or stops the WebRTC server to match its setting. Off air stops it whatever
    /// the setting or the config says. A server that stops frees its ports on its own
    /// thread, and the next one starts after that.
    pub(crate) fn sync_webrtc(&mut self) {
        let Some(base) = &self.cfg.webrtc else { return };
        let settings = self.engine.state().settings(WEBRTC_PART);
        let on = self.on_air
            && (self.cfg.webrtc_always
                || settings
                    .and_then(|s| s.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false));
        let port = settings
            .and_then(|s| s.get("port"))
            .and_then(|v| v.as_u64())
            .and_then(|p| u16::try_from(p).ok())
            .filter(|_| !self.cfg.webrtc_always)
            .unwrap_or(base.bind.port());
        let bind = std::net::SocketAddr::new(base.bind.ip(), port);
        if self.webrtc.is_some() && (!on || self.webrtc_bind != Some(bind)) {
            if let Some(server) = self.webrtc.take() {
                self.webrtc_stopping = std::thread::Builder::new()
                    .name("webrtc-stop".into())
                    .spawn(move || drop(server))
                    .ok();
            }
            self.webrtc_bind = None;
            return;
        }
        if !on || self.webrtc.is_some() {
            return;
        }
        if self
            .webrtc_stopping
            .as_ref()
            .is_some_and(|t| !t.is_finished())
        {
            return;
        }
        self.webrtc_stopping = None;
        let Some(tap) = &self.webrtc_tap else { return };
        let mut cfg = base.clone();
        cfg.bind = bind;
        let server =
            WebRtcServer::start_metered(cfg, tap.clone(), self.output_meters.webrtc.clone());
        if let Some(now) = &self.webrtc_now {
            now.send(&server, &self.webrtc_played);
        }
        self.webrtc = Some(server);
        self.webrtc_bind = Some(bind);
    }

    /// Gives the running WebRTC server the station name and logo when they change, and
    /// raises an alarm for each picture it could not load.
    pub(crate) fn sync_brand(&mut self, now: Timestamp) {
        let Some(server) = &self.webrtc else {
            // The logo it had stays known, so a logo fixed while the server was off still
            // clears its alarm.
            if let Some((_, logo, _)) = self.brand_sent.take() {
                self.logo_before = logo;
            }
            return;
        };
        // Errors of earlier loads go out first, so a change below clears them in the same tick.
        for error in server.take_picture_errors() {
            self.alarms.raise_once(
                now,
                Alarm::PictureFailed {
                    path: error.path.to_string_lossy().into_owned(),
                    reason: error.reason,
                },
            );
        }
        let state = self.engine.state();
        let logo = state.station_logo();
        // The modified time makes a logo file that changes at the same path count as new. A
        // thread reads it; this one never touches the file system.
        let Some(modified) = self
            .logo_watch
            .get_or_insert_with(LogoWatch::start)
            .modified(logo.as_deref())
        else {
            return;
        };
        let brand = (state.station_name().unwrap_or_default(), logo, modified);
        if self.brand_sent.as_ref() != Some(&brand) {
            // The alarms of the logo that goes out of use go with it. A new name alone leaves
            // them: the same file is still the logo, and still does not load.
            let before = match self.brand_sent.take() {
                Some((_, logo, at)) if logo == brand.1 && at == brand.2 => None,
                Some((_, logo, _)) => logo,
                None => self.logo_before.take(),
            };
            if let Some(before) = before {
                let before = before.to_string_lossy().into_owned();
                self.alarms.clear(
                    now,
                    |a| matches!(a, Alarm::PictureFailed { path, .. } if *path == before),
                );
            }
            server.set_brand(brand.0.clone(), brand.1.clone());
            self.brand_sent = Some(brand);
        }
    }
}

/// The On air setting of `state`: on air unless it is stored as `false`.
pub(crate) fn on_air(state: &radio_engine::State) -> bool {
    let stored = state
        .settings(radio_model::AIR_PART)
        .and_then(|s| s.get(radio_model::ON_AIR_KEY))
        .and_then(|v| v.as_bool());
    radio_model::on_air(stored)
}

/// What the logo watch has been asked about and what it saw.
#[derive(Default)]
struct Watched {
    path: Option<std::path::PathBuf>,
    /// The modified time of `path`, once it was read for the path asked about.
    seen: Option<Option<std::time::SystemTime>>,
}

/// Reads the modified time of the station logo on a thread of its own, about once a second,
/// so that a slow disk or share never stops the control thread.
pub(crate) struct LogoWatch {
    shared: std::sync::Arc<std::sync::Mutex<Watched>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// False when the thread did not start: the logo then goes out without its modified
    /// time, and a file changed in place is not sent again.
    watching: bool,
}

impl LogoWatch {
    const EVERY: std::time::Duration = std::time::Duration::from_secs(1);
    const STEP: std::time::Duration = std::time::Duration::from_millis(50);

    fn start() -> Self {
        use std::sync::atomic::Ordering;
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Watched::default()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (theirs, stopped) = (shared.clone(), stop.clone());
        let watching = std::thread::Builder::new()
            .name("brand-watch".into())
            .spawn(move || {
                let mut last = (None, std::time::Instant::now());
                while !stopped.load(Ordering::Acquire) {
                    std::thread::sleep(Self::STEP);
                    let Ok(asked) = theirs.lock().map(|w| w.path.clone()) else {
                        return;
                    };
                    let Some(path) = asked else {
                        last.0 = None;
                        continue;
                    };
                    // A new path is read at once; the same one again after a second.
                    if last.0.as_ref() == Some(&path) && last.1.elapsed() < Self::EVERY {
                        continue;
                    }
                    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    last = (Some(path.clone()), std::time::Instant::now());
                    if let Ok(mut w) = theirs.lock()
                        && w.path.as_ref() == Some(&path)
                    {
                        w.seen = Some(modified);
                    }
                }
            })
            .is_ok();
        Self {
            shared,
            stop,
            watching,
        }
    }

    /// The modified time of `path` as last seen: `None` while it is not known yet, `Some(None)`
    /// for no logo or a file that cannot be read.
    fn modified(&self, path: Option<&std::path::Path>) -> Option<Option<std::time::SystemTime>> {
        let Some(path) = path else {
            if let Ok(mut w) = self.shared.lock() {
                *w = Watched::default();
            }
            return Some(None);
        };
        if !self.watching {
            return Some(None);
        }
        let mut w = self.shared.lock().ok()?;
        if w.path.as_deref() != Some(path) {
            w.path = Some(path.to_path_buf());
            w.seen = None;
        }
        w.seen
    }
}

impl Drop for LogoWatch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// What plays now, as WebRTC listeners see it.
pub(crate) struct NowPlaying {
    title: String,
    artist: String,
    file: Option<std::path::PathBuf>,
    /// When it started.
    at: Timestamp,
    /// It goes into the history after it: its title is not hidden.
    listed: bool,
}

/// The longest history that listeners get.
const PLAYED_MAX: usize = 40;

/// The title and artist that listeners see of `item`, or `None` when it hides them.
fn listed_title(item: &Item, station_name: &str) -> Option<(String, String)> {
    let mut shown = item.clone();
    if shown.meta.title.is_empty() {
        shown.meta.title.clone_from(&item.title);
        shown.meta.artist = item.artist.clone().unwrap_or_default();
    }
    match shown.meta.publish {
        PublishMode::FromTags => Some((shown.meta.title.clone(), shown.meta.artist.clone())),
        PublishMode::Custom => Some((stream_title(&shown, "", station_name), String::new())),
        PublishMode::Hide | PublishMode::KeepPrevious => None,
    }
    .filter(|(title, _)| !title.trim().is_empty())
}

/// The file that gives listeners the cover of `item`. An item that shows its tags or its own
/// text shows its own picture. Without one, only an item that shows its tags falls back to
/// the cover in the tags of its audio file. A hidden item has no cover.
fn cover_file(item: &Item) -> Option<std::path::PathBuf> {
    let own = item.meta.picture.trim();
    match item.meta.publish {
        PublishMode::FromTags | PublishMode::Custom if !own.is_empty() => {
            Some(std::path::PathBuf::from(own))
        }
        PublishMode::FromTags => match &item.source {
            Source::File { path } => Some(std::path::PathBuf::from(path)),
            _ => None,
        },
        _ => None,
    }
}

impl NowPlaying {
    fn send(&self, server: &WebRtcHandle, played: &std::collections::VecDeque<Played>) {
        let history: Vec<Played> = played.iter().cloned().collect();
        server.set_now(
            self.title.clone(),
            self.artist.clone(),
            self.file.clone(),
            &history,
        );
    }
}

impl Core {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(publish: PublishMode, picture: &str, source: Source) -> Item {
        let mut item = radio_testkit::track(1, "Show", "Host", 1);
        item.source = source;
        item.meta.publish = publish;
        item.meta.picture = picture.to_owned();
        item
    }

    fn file() -> Source {
        Source::File {
            path: "/music/show.mp3".into(),
        }
    }

    #[test]
    fn an_item_picture_beats_the_tag_cover_and_a_hidden_item_has_none() {
        use PublishMode::{FromTags, Hide, KeepPrevious};
        let own = std::path::PathBuf::from("/art/show.png");
        for mode in [FromTags, PublishMode::Custom] {
            assert_eq!(
                cover_file(&item(mode, "/art/show.png", file())),
                Some(own.clone())
            );
        }
        for mode in [Hide, KeepPrevious] {
            assert_eq!(
                cover_file(&item(mode, "/art/show.png", file())),
                None,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn without_a_picture_only_tags_mode_reads_the_audio_file() {
        let audio = Some(std::path::PathBuf::from("/music/show.mp3"));
        assert_eq!(cover_file(&item(PublishMode::FromTags, "", file())), audio);
        assert_eq!(cover_file(&item(PublishMode::Custom, "  ", file())), None);
        let stream = Source::Stream {
            url: "https://relay.example.org/live".into(),
            keep_alive: true,
        };
        assert_eq!(cover_file(&item(PublishMode::FromTags, "", stream)), None);
    }
}
