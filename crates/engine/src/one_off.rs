//! One-off items of Upcoming. The system playlist One-off holds them. Each item has one
//! At rule that names it with `Constraint::OnlyItem`. The rules script of the playlist
//! comes from the rules, so a person can read and edit it.

use std::time::Duration;

use jiff::civil;
use jiff::tz::TimeZone;
use radio_model::{
    Constraint, FixMode, Item, ItemId, PickOrder, Placement, Playlist, PlaylistId, PlaylistRole,
    Repeat, Rule, Source, Station, Trigger,
};
use radio_timing::LocalStart;

use crate::{Engine, EngineError, EngineEvent, Outcome, State};

/// The name of the system playlist, when no other playlist has it.
pub const ONE_OFF_PLAYLIST: &str = "One-off";
/// The colour of the system playlist: teal of the palette.
const ONE_OFF_COLOR: [u8; 3] = [0x14, 0xb8, 0xa6];
/// The priority of one-offs over other fixed events at the same time.
const ONE_OFF_PRIORITY: i32 = 100;
/// The widest tolerance of a soft one-off.
const MAX_TOLERANCE: Duration = Duration::from_secs(3600);

/// One one-off item, as its rule describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct OneOff {
    pub playlist: PlaylistId,
    pub item: Item,
    /// The local start time in the station time zone.
    pub start: civil::DateTime,
    pub fix: FixMode,
    pub tolerance: Duration,
    pub repeat: Repeat,
}

/// The id of the One-off playlist, if the station has one.
pub fn one_off_playlist(station: &Station) -> Option<PlaylistId> {
    station
        .playlists
        .iter()
        .find(|p| p.role == PlaylistRole::OneOff)
        .map(|p| p.id)
}

fn binds(rule: &Rule, item: ItemId) -> bool {
    rule.constraints.contains(&Constraint::OnlyItem(item))
}

impl State {
    /// The one-off items, in the order of their playlist. An item without its At rule is
    /// not a one-off.
    pub fn one_offs(&self) -> Vec<OneOff> {
        let Some(p) = self
            .station
            .playlists
            .iter()
            .find(|p| p.role == PlaylistRole::OneOff)
        else {
            return Vec::new();
        };
        p.items
            .iter()
            .filter_map(|item| {
                p.rules.iter().find(|r| binds(r, item.id)).and_then(|r| {
                    let (Trigger::At { at, repeat }, Placement::Fixed { mode, tolerance }) =
                        (&r.trigger, &r.placement)
                    else {
                        return None;
                    };
                    Some(OneOff {
                        playlist: p.id,
                        item: item.clone(),
                        start: *at,
                        fix: *mode,
                        tolerance: *tolerance,
                        repeat: *repeat,
                    })
                })
            })
            .collect()
    }
}

/// A title for an item that has none: the file name, the address or "Live input".
fn default_title(source: &Source) -> String {
    match source {
        Source::File { path } => std::path::Path::new(path)
            .file_stem()
            .map_or_else(|| path.clone(), |s| s.to_string_lossy().into_owned()),
        Source::Stream { url, .. } => url.clone(),
        Source::LiveInput { .. } => "Live input".to_owned(),
    }
}

/// Checks that the item has a length. A stream and a live input need a planned length.
fn check_length(item: &Item) -> Result<(), EngineError> {
    let planned = item.planned_len.is_some_and(|l| !l.is_zero());
    let ok = match item.source {
        Source::File { .. } => item.timing.is_some() || planned,
        Source::Stream { .. } | Source::LiveInput { .. } => planned,
    };
    if ok {
        Ok(())
    } else {
        Err(EngineError::BadArgument(
            "a one-off needs a length: give a stream or a live input a planned length".into(),
        ))
    }
}

/// Takes the one-off `id` and its rules out of `station`.
fn take_out(station: &mut Station, id: ItemId) -> Result<(), EngineError> {
    let p = station
        .playlists
        .iter_mut()
        .find(|p| p.role == PlaylistRole::OneOff)
        .filter(|p| p.items.iter().any(|i| i.id == id))
        .ok_or(EngineError::UnknownItem(id))?;
    p.items.retain(|i| i.id != id);
    p.rules.retain(|r| !binds(r, id));
    Ok(())
}

/// The One-off playlist of `station`. It makes the playlist if there is none.
fn playlist_of(station: &mut Station) -> &mut Playlist {
    if one_off_playlist(station).is_none() {
        let id = PlaylistId(station.playlists.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
        let taken = |n: &str| station.playlists.iter().any(|p| p.name == n);
        let name = std::iter::once(ONE_OFF_PLAYLIST.to_owned())
            .chain((2..).map(|k| format!("{ONE_OFF_PLAYLIST} {k}")))
            .find(|n| !taken(n))
            .unwrap_or_else(|| ONE_OFF_PLAYLIST.to_owned());
        station.playlists.push(Playlist {
            id,
            source: None,
            name,
            color: ONE_OFF_COLOR,
            enabled: true,
            is_main: false,
            is_fill_pool: false,
            role: PlaylistRole::OneOff,
            on_interrupt: radio_model::OnInterrupt::Resume,
            priority: ONE_OFF_PRIORITY,
            pick: PickOrder::InOrder,
            items: Vec::new(),
            rules: Vec::new(),
            constraints: Vec::new(),
        });
    }
    let last = station.playlists.len() - 1;
    let at = station
        .playlists
        .iter()
        .position(|p| p.role == PlaylistRole::OneOff)
        .unwrap_or(last);
    &mut station.playlists[at]
}

/// What `schedule.add` and `schedule.update` ask for.
pub(crate) struct Wish {
    pub(crate) item: Item,
    pub(crate) start: civil::DateTime,
    pub(crate) fix: FixMode,
    pub(crate) tolerance: Duration,
    pub(crate) repeat: Repeat,
}

impl Engine {
    /// Refuses an edit that would break the One-off playlist. Upcoming owns it.
    pub(crate) fn refuse_system(&self, playlist: PlaylistId) -> Result<(), EngineError> {
        if one_off_playlist(&self.state.station) == Some(playlist) {
            return Err(EngineError::BadArgument(
                "the One-off playlist belongs to Upcoming; use schedule.add, schedule.update \
                 or schedule.remove"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Adds a one-off, or replaces the one-off `replace`.
    pub(crate) fn schedule(
        &mut self,
        replace: Option<ItemId>,
        wish: Wish,
    ) -> Result<Outcome, EngineError> {
        let Wish {
            mut item,
            start,
            fix,
            tolerance,
            repeat,
        } = wish;
        let tz = TimeZone::get(&self.state.station.tz)
            .map_err(|_| EngineError::BadArgument("the station time zone is unknown".into()))?;
        let when = radio_timing::local_start(&tz, start)
            .map_err(|e| EngineError::BadArgument(e.to_string()))?;
        if tolerance > MAX_TOLERANCE {
            return Err(EngineError::BadArgument(
                "the tolerance of a one-off is one hour at most".into(),
            ));
        }
        check_length(&item)?;
        if item.title.trim().is_empty() {
            item.title = default_title(&item.source);
        }
        // Fitting never moves or trims a one-off.
        item.movable = false;
        item.trimmable = false;
        item.locked = false;
        let mut next = self.state.station.clone();
        let rule = |id: ItemId| Rule {
            trigger: Trigger::At { at: start, repeat },
            placement: Placement::Fixed {
                mode: fix,
                tolerance,
            },
            take: 1,
            constraints: vec![Constraint::OnlyItem(id)],
        };
        match replace {
            Some(id) => item.id = id,
            None if item.id.0 == 0 => {
                let max = next
                    .playlists
                    .iter()
                    .flat_map(|p| p.items.iter().map(|i| i.id.0))
                    .max()
                    .unwrap_or(0);
                item.id = ItemId(max + 1);
            }
            None => {}
        }
        let id = item.id;
        match replace {
            // An update keeps the places of the item and its rule, so no other rule moves.
            Some(_) => {
                let p = next
                    .playlists
                    .iter_mut()
                    .find(|p| p.role == PlaylistRole::OneOff)
                    .ok_or(EngineError::UnknownItem(id))?;
                let i = p
                    .items
                    .iter()
                    .position(|x| x.id == id)
                    .ok_or(EngineError::UnknownItem(id))?;
                p.items[i] = item;
                match p.rules.iter().position(|r| binds(r, id)) {
                    Some(r) => {
                        p.rules[r] = rule(id);
                        p.rules = std::mem::take(&mut p.rules)
                            .into_iter()
                            .enumerate()
                            .filter(|(k, x)| *k == r || !binds(x, id))
                            .map(|(_, x)| x)
                            .collect();
                    }
                    None => p.rules.push(rule(id)),
                }
            }
            None => {
                let p = playlist_of(&mut next);
                p.items.push(item);
                p.rules.push(rule(id));
            }
        }
        next.validate()?;
        self.state.station = next;
        self.sync_one_off_script();
        let note = matches!(when, LocalStart::Twice { .. }).then(|| {
            format!(
                "{} happens twice, as the clocks go back; the one-off airs the first time",
                start.strftime("%Y-%m-%d %H:%M")
            )
        });
        Ok(Outcome {
            data: Some(id.0.into()),
            note,
            ..Outcome::default()
        })
    }

    /// Removes the one-off `id` and its rule.
    pub(crate) fn unschedule(&mut self, id: ItemId) -> Result<Outcome, EngineError> {
        let mut next = self.state.station.clone();
        take_out(&mut next, id)?;
        next.validate()?;
        self.state.station = next;
        self.sync_one_off_script();
        self.state.prune_orphans();
        Ok(Outcome::default())
    }

    /// Removes a one-off after it aired or missed its time. Playout calls this.
    /// It adds no undo step and keeps the undo history.
    pub fn expire_one_off(&mut self, id: ItemId) -> Result<(), EngineError> {
        self.unschedule(id)?;
        self.store.snapshot(self.state.clone());
        self.emit(EngineEvent::StateChanged);
        Ok(())
    }

    /// Writes the rules script of the One-off playlist from its rules.
    fn sync_one_off_script(&mut self) {
        let station = &self.state.station;
        let Some(p) = station
            .playlists
            .iter()
            .find(|p| p.role == PlaylistRole::OneOff)
        else {
            return;
        };
        let names: Vec<(String, PlaylistId)> = station
            .playlists
            .iter()
            .map(|p| (p.name.clone(), p.id))
            .collect();
        let set = radio_script::RuleSet {
            pick: p.pick.clone(),
            rules: p.rules.clone(),
            constraints: p.constraints.clone(),
        };
        let script = radio_script::generate(&set, &names);
        let id = p.id;
        self.state.scripts.insert(id, script);
    }
}
