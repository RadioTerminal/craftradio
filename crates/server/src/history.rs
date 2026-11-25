//! What a station played before, kept by the server for the listener page: each title the
//! station's source named, once another took its place, with the cover the station's app
//! set for it. Kept in memory only, and across the ends of the source, so a push that
//! drops and comes back keeps the list.

use crate::cover::Cover;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The most items the history keeps.
pub const HISTORY_MAX: usize = 40;
/// How far back the history reaches: an item that started longer ago is left out.
pub const HISTORY_SPAN: Duration = Duration::from_secs(3600);
/// A cover set this short a time before a new title came with that title: the app sends
/// both as an item starts, in either order.
pub const LEADING_COVER: Duration = Duration::from_secs(5);

/// One item the station played.
#[derive(Debug, Clone)]
pub struct Played {
    pub title: String,
    pub artist: String,
    pub started: Instant,
    pub cover: Option<Arc<Cover>>,
}

/// What plays now, and what it needs to give a leading cover to the next title.
#[derive(Debug)]
struct Now {
    item: Played,
    /// The title is the station's name: a hidden item, which stays out of the history.
    hidden: bool,
    /// When the app last set (or cleared) this item's cover.
    cover_at: Option<Instant>,
    /// The cover the item had before its last one.
    before: Option<Arc<Cover>>,
}

/// The history of one station.
#[derive(Debug, Default)]
pub struct History {
    now: Option<Now>,
    /// A cover set before any title, and when.
    early: Option<(Option<Arc<Cover>>, Instant)>,
    /// Newest first.
    played: VecDeque<Played>,
}

impl History {
    /// The source names `title` by `artist` at `at`, on a station called `name`. A new
    /// title ends the item before it, which joins the history unless it was hidden. No
    /// title (the push dropped, or the station sent none) ends nothing, and the same title
    /// again (the push came back) changes nothing.
    pub fn title(&mut self, title: &str, artist: &str, name: &str, at: Instant) {
        if title.is_empty() {
            return;
        }
        if self
            .now
            .as_ref()
            .is_some_and(|now| now.item.title == title && now.item.artist == artist)
        {
            return;
        }
        let leading = |set: Instant| at.saturating_duration_since(set) < LEADING_COVER;
        let mut cover = None;
        if let Some(mut old) = self.now.take() {
            // A cover set just before this title, after the old one started, is this one's.
            if old
                .cover_at
                .is_some_and(|set| set > old.item.started && leading(set))
            {
                cover = old.item.cover.take();
                old.item.cover = old.before.take();
            }
            if !old.hidden {
                self.played.push_front(old.item);
                self.played.truncate(HISTORY_MAX);
            }
        } else if let Some((early, set)) = self.early.take()
            && leading(set)
        {
            cover = early;
        }
        self.now = Some(Now {
            item: Played {
                title: title.to_owned(),
                artist: artist.to_owned(),
                started: at,
                cover,
            },
            hidden: title == name,
            cover_at: None,
            before: None,
        });
    }

    /// The app set the cover of what plays now (`None` cleared it) at `at`.
    pub fn cover(&mut self, cover: Option<Arc<Cover>>, at: Instant) {
        match &mut self.now {
            Some(now) => {
                now.before = std::mem::replace(&mut now.item.cover, cover);
                now.cover_at = Some(at);
            }
            None => self.early = Some((cover, at)),
        }
    }

    /// What played before, newest first, as of `at`: what started within the last hour.
    pub fn played(&self, at: Instant) -> impl Iterator<Item = &Played> {
        self.played
            .iter()
            .filter(move |p| at.saturating_duration_since(p.started) < HISTORY_SPAN)
    }

    /// The cover of an item in the history, or of what plays now, by its tag (unquoted).
    pub fn cover_tagged(&self, tag: &str) -> Option<Arc<Cover>> {
        self.played
            .iter()
            .chain(self.now.as_ref().map(|now| &now.item))
            .filter_map(|p| p.cover.as_ref())
            .find(|cover| cover.etag.trim_matches('"') == tag)
            .cloned()
    }

    /// The history as the listener page reads it at `GET /<slug>/history`: each item's
    /// title, artist, how long ago it started in milliseconds, and the path of its cover.
    pub fn json(&self, slug: &str, at: Instant) -> String {
        let items: Vec<_> = self
            .played(at)
            .map(|p| {
                let ago = u64::try_from(at.saturating_duration_since(p.started).as_millis())
                    .unwrap_or(u64::MAX);
                serde_json::json!({
                    "title": p.title,
                    "artist": p.artist,
                    "ago_ms": ago,
                    "cover": p.cover.as_ref().map(|c| format!("/{slug}/cover/{}", c.etag.trim_matches('"'))),
                })
            })
            .collect();
        serde_json::json!({ "items": items }).to_string()
    }
}
