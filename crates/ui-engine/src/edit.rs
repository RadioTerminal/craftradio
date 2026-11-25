//! Each [`Edit`] of the UI as engine commands.

use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use radio_engine::{CartMode as ECartMode, Command, Mode, Outcome};
use radio_model::{Item, ItemId, PickOrder, PlaylistId, Source};
use radio_ui_egui::theme::PALETTE;
use radio_ui_egui::view::{
    CartChange, CartMode, DeckCommand, DeckState, DeviceRole, DropTarget, Edit, Imported,
    MetaTarget, MoveTo, Place, PlayMode, PlaylistTab, StripChange,
};

use crate::map::{display_title, model_meta};
use crate::{CARTWALL_PART, EngineSource};

/// A command that has no answer after this time gets a notice.
const ANSWER_WAIT: Duration = Duration::from_secs(30);

/// A command on its way to the engine.
pub(crate) struct Pending {
    /// The notice when it worked, or empty for none.
    what: String,
    rx: Receiver<Result<Outcome, radio_engine::EngineError>>,
    at: Instant,
}

/// Where probed files go.
pub(crate) struct AddTarget {
    pub(crate) playlist: PlaylistId,
    pub(crate) name: String,
    pub(crate) at: Option<usize>,
    pub(crate) play_next: bool,
}

/// What to do with the items of a probe when they arrive.
pub(crate) enum ProbeAfter {
    Add(AddTarget),
    /// A new folder playlist: its id, name and source.
    NewPlaylist {
        id: PlaylistId,
        name: String,
        source: serde_json::Value,
    },
    Import {
        category: String,
    },
    /// A one-off of Upcoming that plays the file.
    OneOff(radio_ui_egui::view::OneOffView),
}
/// The name of the playlist that holds library files for carts.
const CART_ITEMS: &str = "Cart items";
/// The part of `settings_by_part` for the audio devices.
pub const AUDIO_PART: &str = "audio";

impl EngineSource {
    /// Sends one engine command. It never waits: the answer comes back to a later snapshot,
    /// and an error or a note becomes a notice for the UI.
    pub(crate) fn send(&mut self, what: &str, cmd: Command) -> Result<(), String> {
        let rx = self.playout.send(cmd).map_err(|e| e.to_string())?;
        self.pending.push(Pending {
            what: what.to_owned(),
            rx,
            at: Instant::now(),
        });
        Ok(())
    }

    /// Sends a station edit. The snapshot shows it once the engine took it.
    pub(crate) fn run_edit(&mut self, cmd: Command) -> Result<Outcome, String> {
        self.send("", cmd)?;
        Ok(Outcome::default())
    }

    /// Sends station edits as one undo step.
    pub(crate) fn run_batch(&mut self, mut commands: Vec<Command>) -> Result<Outcome, String> {
        match commands.len() {
            0 => Ok(Outcome::default()),
            1 => self.run_edit(commands.remove(0)),
            _ => self.run_edit(Command::StationBatch { commands }),
        }
    }

    /// Sends an audio command. A refusal comes back as a notice.
    fn run_audio(&mut self, cmd: Command) -> Result<usize, String> {
        self.send("", cmd)?;
        Ok(1)
    }

    /// Takes the answers that arrived. Errors and notes become notices.
    pub(crate) fn poll_pending(&mut self) {
        let mut notices = Vec::new();
        self.pending.retain(|p| match p.rx.try_recv() {
            Ok(Ok(o)) => {
                if let Some(note) = o.note {
                    notices.push(format!("Applied, but {note}"));
                } else if o.audio_required && !o.applied_to_audio {
                    notices.push("The mixer did not confirm the command".to_owned());
                } else if !p.what.is_empty() {
                    if let Some(reason) = self.playout.log().persistence_error() {
                        notices.push(format!("Changes are not saved: {reason}"));
                    } else {
                        notices.push(p.what.replace("Saved", "Applied"));
                    }
                }
                false
            }
            Ok(Err(e)) => {
                notices.push(e.to_string());
                false
            }
            Err(TryRecvError::Disconnected) => false,
            Err(TryRecvError::Empty) => {
                if p.at.elapsed() > ANSWER_WAIT {
                    notices.push("The playout did not answer a command".to_owned());
                    false
                } else {
                    true
                }
            }
        });
        self.notices.extend(notices);
    }

    /// New item ids, `n` of them, that no item of the station has.
    pub(crate) fn fresh_ids(&mut self, n: u64) -> u64 {
        let max = self.items.keys().map(|i| i.0).max().unwrap_or(0);
        let first = self.next_id.max(max + 1);
        self.next_id = first + n;
        first
    }

    pub(crate) fn playlist_id(&self, name: &str) -> Result<PlaylistId, String> {
        self.state
            .station
            .playlists
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.id)
            .ok_or_else(|| format!("The playlist {name} is gone"))
    }

    fn playlist_name_of(&self, id: PlaylistId) -> String {
        self.state
            .station
            .playlists
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    pub(crate) fn main_id(&self) -> Option<PlaylistId> {
        self.state
            .station
            .playlists
            .iter()
            .find(|p| p.is_main)
            .map(|p| p.id)
    }

    pub(crate) fn unique_name(&self, base: &str) -> String {
        let base = base.trim().replace(['\n', '\r'], " ");
        let base = if base.is_empty() {
            "Playlist".to_owned()
        } else {
            base
        };
        let taken = |n: &str| self.state.station.playlists.iter().any(|p| p.name == n);
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|k| format!("{base} {k}"))
            .find(|n| !taken(n))
            .unwrap_or(base)
    }

    fn main_len(&self) -> usize {
        let main = self.main_id();
        self.state
            .station
            .playlists
            .iter()
            .find(|p| Some(p.id) == main)
            .map_or(0, |p| p.items.len())
    }

    /// The main playlist index of the first main row from row index `i` on, or `None` for the end.
    fn main_index_at_row(&self, i: usize) -> Option<usize> {
        let main = self.main_id()?;
        let list = &self
            .state
            .station
            .playlists
            .iter()
            .find(|p| p.id == main)?
            .items;
        self.row_order.iter().skip(i).find_map(|id| {
            let r = self.rows.get(id)?;
            (r.playlist == main)
                .then(|| list.iter().position(|it| it.id == r.item))
                .flatten()
        })
    }

    /// True when row index `i` is the place right after the row on air (or the first row,
    /// when nothing is on air).
    fn is_next_row(&self, i: usize) -> bool {
        let on_air = self
            .row_order
            .iter()
            .position(|id| self.on_air_rows.contains(id));
        match on_air {
            Some(k) => i <= k + 1,
            None => i == 0,
        }
    }

    fn row_index(&self, row: u64) -> Result<usize, String> {
        self.row_order
            .iter()
            .position(|r| *r == row)
            .ok_or_else(|| "That row is gone from the timeline".to_owned())
    }

    /// The main playlist index for a place in the timeline.
    fn main_index(&self, at: Place) -> Result<Option<usize>, String> {
        Ok(match at {
            Place::Before(row) => self.main_index_at_row(self.row_index(row)?),
            Place::After(row) => self.main_index_at_row(self.row_index(row)? + 1),
            Place::End => None,
        })
    }

    /// The item behind a library id: a station item or a library file.
    pub(crate) fn library_item(&self, id: u64) -> Option<Item> {
        if let Some((_, item)) = self.items.get(&ItemId(id)) {
            return Some(item.clone());
        }
        self.files
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.item.clone())
    }

    /// Copies of items with fresh ids, for a playlist.
    pub(crate) fn copies(&mut self, items: Vec<Item>) -> Vec<Item> {
        let first = self.fresh_ids(items.len() as u64);
        items
            .into_iter()
            .enumerate()
            .map(|(k, mut i)| {
                i.id = ItemId(first + k as u64);
                i
            })
            .collect()
    }

    /// Where items for `playlist` go. For the main playlist, `at` is a row index.
    pub(crate) fn add_target(
        &self,
        playlist: &str,
        at: Option<usize>,
    ) -> Result<AddTarget, String> {
        let id = self.playlist_id(playlist)?;
        let main = Some(id) == self.main_id();
        // A row right after the item on air means "play next": the items join the end of the
        // main playlist, and the play-next queue puts them on air next.
        if main && at.is_some_and(|i| self.is_next_row(i)) {
            return Ok(AddTarget {
                playlist: id,
                name: playlist.to_owned(),
                at: None,
                play_next: true,
            });
        }
        let at = if main {
            at.and_then(|i| self.main_index_at_row(i))
        } else {
            let len = self
                .state
                .station
                .playlists
                .iter()
                .find(|p| p.id == id)
                .map_or(0, |p| p.items.len());
            at.map(|i| i.min(len))
        };
        Ok(AddTarget {
            playlist: id,
            name: playlist.to_owned(),
            at,
            play_next: false,
        })
    }

    /// Adds items to a playlist. For the main playlist, `at` is a row index.
    fn add_to(
        &mut self,
        playlist: &str,
        items: Vec<Item>,
        at: Option<usize>,
    ) -> Result<usize, String> {
        let target = self.add_target(playlist, at)?;
        self.finish_add(&target, items, "")
    }

    /// Sends the items to their target. `what` is the notice when the engine took them.
    pub(crate) fn finish_add(
        &mut self,
        target: &AddTarget,
        items: Vec<Item>,
        what: &str,
    ) -> Result<usize, String> {
        let id = target.playlist;
        let at = target.at;
        if target.play_next {
            let n = items.len();
            let items = self.copies(items);
            let mut cmds: Vec<Command> = items
                .iter()
                .rev()
                .map(|i| Command::PlayoutPlayNext { item: i.id })
                .collect();
            cmds.insert(
                0,
                Command::PlaylistAddItems {
                    playlist: id,
                    items,
                    at: None,
                    lock: true,
                },
            );
            self.send(what, Command::StationBatch { commands: cmds })?;
            return Ok(n);
        }
        let n = items.len();
        let items = self.copies(items);
        self.send(
            what,
            Command::PlaylistAddItems {
                playlist: id,
                items,
                at,
                lock: true,
            },
        )?;
        Ok(n)
    }

    /// Asks the background lane to probe paths, then to do `after` with the items.
    pub(crate) fn queue_probe(
        &mut self,
        paths: Vec<String>,
        recursive: bool,
        after: ProbeAfter,
    ) -> Result<(), String> {
        let worker = self
            .worker
            .as_ref()
            .ok_or("The background reader could not start")?;
        self.next_req += 1;
        let req = self.next_req;
        worker
            .jobs
            .send(crate::bg::Job::Probe {
                req,
                paths,
                recursive,
                limits: self.limits,
            })
            .map_err(|_| "The background reader has stopped".to_owned())?;
        self.probes.insert(req, after);
        Ok(())
    }

    /// Finishes a probe: its items go where the request said.
    pub(crate) fn probed(&mut self, req: u64, items: Vec<Item>, skipped: usize, errors: &[String]) {
        let Some(after) = self.probes.remove(&req) else {
            return;
        };
        let skip = if skipped > 0 {
            format!(" · skipped {skipped}")
        } else {
            String::new()
        };
        let result = match after {
            ProbeAfter::Add(target) => {
                if items.is_empty() {
                    let why = errors.first().cloned().unwrap_or_default();
                    Err(format!(
                        "No audio files to add to {}{skip} {why}",
                        target.name
                    ))
                } else {
                    let what = format!("Added {} files to {}{skip}", items.len(), target.name);
                    self.finish_add(&target, items, &what).map(|_| ())
                }
            }
            ProbeAfter::NewPlaylist { id, name, source } => {
                if items.is_empty() {
                    let why = errors.first().cloned().unwrap_or_default();
                    Err(format!("No audio files for the playlist {name} {why}"))
                } else {
                    let what = format!("Made the playlist {name} from {} files", items.len());
                    let items = self.copies(items);
                    self.send(
                        &what,
                        Command::StationBatch {
                            commands: vec![
                                Command::PlaylistCreate { name },
                                Command::PlaylistSetPick {
                                    playlist: id,
                                    pick: PickOrder::Shuffle { seed: id.0 },
                                },
                                Command::PlaylistAddItems {
                                    playlist: id,
                                    items,
                                    at: None,
                                    lock: false,
                                },
                                Command::PlaylistSetSource {
                                    playlist: id,
                                    source,
                                },
                            ],
                        },
                    )
                }
            }
            ProbeAfter::OneOff(view) => match items.into_iter().next() {
                Some(item) => self.send_one_off(&view, item),
                None => {
                    let why = errors.first().cloned().unwrap_or_default();
                    Err(format!("The one-off has no audio file to play {why}"))
                }
            },
            ProbeAfter::Import { category } => {
                let n = items.len();
                for item in items {
                    self.add_file(item, &category);
                }
                self.notices.push(format!(
                    "Added {n} files to the library under {category}{skip}"
                ));
                Ok(())
            }
        };
        if let Err(e) = result {
            self.notices.push(e);
        }
    }

    /// The cart index as page and slot.
    fn cart_place(&self, cart: usize) -> (u32, u32) {
        let per = self.per_page().max(1);
        (
            u32::try_from(cart / per).unwrap_or(u32::MAX),
            u32::try_from(cart % per).unwrap_or(u32::MAX),
        )
    }

    /// Commands that put a library item on a cart. A library file goes into a playlist first.
    fn assign_cart(&mut self, cart: usize, item: u64) -> Result<Vec<Command>, String> {
        let (page, slot) = self.cart_place(cart);
        if self.items.contains_key(&ItemId(item)) {
            return Ok(vec![Command::CartAssign {
                page,
                slot,
                item: ItemId(item),
            }]);
        }
        let file = self
            .library_item(item)
            .ok_or_else(|| "That library item is gone".to_owned())?;
        let mut cmds = Vec::new();
        let playlist = match self.playlist_id(CART_ITEMS) {
            Ok(id) => id,
            Err(_) => {
                let max = self.state.station.playlists.iter().map(|p| p.id.0).max();
                let id = PlaylistId(max.unwrap_or(0) + 1);
                cmds.push(Command::PlaylistCreate {
                    name: CART_ITEMS.into(),
                });
                cmds.push(Command::PlaylistEnable {
                    playlist: id,
                    enabled: false,
                });
                id
            }
        };
        let copy = self.copies(vec![file]).remove(0);
        let id = copy.id;
        cmds.push(Command::PlaylistAddItems {
            playlist,
            items: vec![copy],
            at: None,
            lock: false,
        });
        cmds.push(Command::CartAssign {
            page,
            slot,
            item: id,
        });
        Ok(cmds)
    }

    fn page_names(&self) -> Vec<String> {
        self.cartwall().1.pages
    }

    /// Commands that lay the carts out on pages of `per` carts. Old pages keep their carts
    /// in order; extra carts go onto a new page after their page.
    fn fit_cart_grid(&self, per: usize) -> Vec<Command> {
        let old = self.per_page().max(1);
        let per = per.max(1);
        if per == old {
            return Vec::new();
        }
        let names = self.page_names();
        let mut pages = Vec::new();
        let mut assigns = Vec::new();
        for (p, name) in names.iter().enumerate() {
            let mut used: Vec<_> = self
                .state
                .carts
                .iter()
                .filter(|c| c.page as usize == p)
                .collect();
            used.sort_by_key(|c| c.slot);
            let mut first = true;
            loop {
                let take = used.len().min(per);
                let page = u32::try_from(pages.len()).unwrap_or(u32::MAX);
                pages.push(if first {
                    name.clone()
                } else {
                    format!("{name} (more)")
                });
                first = false;
                for (k, c) in used.drain(..take).enumerate() {
                    assigns.push((page, u32::try_from(k).unwrap_or(u32::MAX), c.item));
                }
                if used.is_empty() {
                    break;
                }
            }
        }
        let mut cmds: Vec<Command> = self
            .state
            .carts
            .iter()
            .map(|c| Command::CartClear {
                page: c.page,
                slot: c.slot,
            })
            .collect();
        for (page, slot, item) in assigns {
            cmds.push(Command::CartAssign { page, slot, item });
        }
        cmds.push(Command::SettingsSet {
            part: CARTWALL_PART.into(),
            key: "pages".into(),
            value: serde_json::json!(pages),
        });
        cmds
    }

    fn deck_command(&mut self, deck: usize, command: DeckCommand) -> Result<usize, String> {
        let d = u8::try_from(deck).map_err(|_| "No such deck".to_owned())?;
        let name = ['A', 'B', 'C', 'D'].get(deck).copied().unwrap_or('?');
        let expect = self
            .decks
            .get(deck)
            .copied()
            .flatten()
            .ok_or_else(|| format!("Deck {name} is empty"))?;
        // A cheap check against the playout now: a deck that changed since the snapshot
        // refuses at once. The engine checks again when the command arrives.
        if self.playout.deck_items().get(deck).copied().flatten() != Some(expect) {
            return Err(format!(
                "Deck {name} holds another item now; look again and try again"
            ));
        }
        let expect_item = Some(expect);
        let cmd = match command {
            DeckCommand::PlayPause if self.deck_states[deck] == DeckState::Playing => {
                Command::DeckPause {
                    deck: d,
                    expect_item,
                }
            }
            DeckCommand::PlayPause => Command::DeckPlay {
                deck: d,
                expect_item,
                fade_in: Duration::ZERO,
            },
            DeckCommand::Play(secs) => Command::DeckPlay {
                deck: d,
                expect_item,
                fade_in: Duration::try_from_secs_f32(secs.max(0.0)).unwrap_or_default(),
            },
            DeckCommand::Fade(secs) => Command::DeckFade {
                deck: d,
                expect_item,
                over: Some(Duration::try_from_secs_f32(secs.max(0.0)).unwrap_or_default()),
            },
            DeckCommand::Seek(position) => Command::DeckSeek {
                deck: d,
                position,
                expect_item,
            },
        };
        self.run_audio(cmd)
    }

    /// Plays the NEXT 1 deck: the deck that waits with the earliest start.
    fn play_next(&mut self, fade_in_s: f32) -> Result<usize, String> {
        let deck = self.next_ready.ok_or("No deck waits to play next")?;
        self.deck_command(deck, DeckCommand::Play(fade_in_s))
    }

    fn move_row(&mut self, id: u64, to: MoveTo) -> Result<usize, String> {
        let r = *self
            .rows
            .get(&id)
            .ok_or_else(|| "That row is gone from the timeline".to_owned())?;
        let main = self.main_id().ok_or("The station has no main playlist")?;
        if r.playlist != main {
            return Err("Only rows of the main playlist move; rules place the other rows".into());
        }
        if to == MoveTo::Next {
            self.run_edit(Command::PlayoutPlayNext { item: r.item })?;
            return Ok(1);
        }
        let i = self.row_index(id)?;
        let fixed = |k: usize| {
            self.row_order
                .get(k)
                .is_some_and(|id| self.fixed_rows.contains(id))
        };
        let target_row = match to {
            MoveTo::Next => {
                let on_air = self
                    .row_order
                    .iter()
                    .position(|id| self.on_air_rows.contains(id))
                    .ok_or("Nothing is on air")?;
                on_air + 1
            }
            MoveTo::Top => (0..i).rev().find(|k| fixed(*k)).map_or(0, |k| k + 1),
            MoveTo::Bottom => (i + 1..self.row_order.len())
                .find(|k| fixed(*k))
                .unwrap_or(self.row_order.len()),
        };
        let pos = self
            .main_index_at_row(target_row)
            .unwrap_or_else(|| self.main_len());
        let from = self
            .state
            .station
            .playlists
            .iter()
            .find(|p| p.id == main)
            .and_then(|p| p.items.iter().position(|it| it.id == r.item))
            .ok_or("That item is gone")?;
        // The remaining list loses the moved item, so a later place shifts by one.
        let to = if pos > from { pos - 1 } else { pos };
        self.run_edit(Command::PlaylistMoveItems {
            playlist: main,
            items: vec![r.item],
            to,
        })?;
        Ok(1)
    }

    fn remove_rows(&mut self, ids: &[u64]) -> Result<usize, String> {
        let mut by_list: Vec<(PlaylistId, Vec<ItemId>)> = Vec::new();
        for id in ids {
            let r = self
                .rows
                .get(id)
                .ok_or_else(|| "That row is gone from the timeline".to_owned())?;
            if self.on_air_rows.contains(id) {
                let title = self
                    .items
                    .get(&r.item)
                    .map_or_else(String::new, |(_, i)| display_title(i));
                return Err(format!("{title} is on air; use Play next instead"));
            }
            match by_list.iter_mut().find(|(p, _)| *p == r.playlist) {
                Some((_, v)) if !v.contains(&r.item) => v.push(r.item),
                Some(_) => {}
                None => by_list.push((r.playlist, vec![r.item])),
            }
        }
        let one_off = radio_engine::one_off_playlist(&self.state.station);
        let cmds = by_list
            .into_iter()
            .flat_map(|(playlist, items)| {
                // A one-off goes with its rule.
                if Some(playlist) == one_off {
                    items
                        .into_iter()
                        .map(|id| Command::ScheduleRemove { id })
                        .collect::<Vec<_>>()
                } else {
                    vec![Command::PlaylistRemoveItems { playlist, items }]
                }
            })
            .collect();
        self.run_batch(cmds)?;
        Ok(ids.len())
    }

    fn set_meta(
        &mut self,
        list: &[(MetaTarget, radio_ui_egui::view::ItemMeta)],
    ) -> Result<usize, String> {
        let mut cmds = Vec::new();
        for (target, meta) in list {
            let item = match target {
                MetaTarget::Row(id) => self.rows.get(id).map(|r| r.item),
                MetaTarget::Library(id) if self.items.contains_key(&ItemId(*id)) => {
                    Some(ItemId(*id))
                }
                MetaTarget::Library(id) => {
                    self.set_file_meta(*id, meta);
                    None
                }
            };
            if let Some(item) = item {
                let value = serde_json::to_value(model_meta(meta)).map_err(|e| e.to_string())?;
                cmds.push(Command::ItemSetMeta { item, meta: value });
                let now = self.items.get(&item).and_then(|(_, i)| match i.source {
                    Source::Stream { keep_alive, .. } => Some(keep_alive),
                    _ => None,
                });
                if let (Some(want), Some(now)) = (meta.keep_alive, now)
                    && want != now
                {
                    cmds.push(Command::ItemSetKeepAlive {
                        item,
                        keep_alive: want,
                    });
                }
                let file = self
                    .items
                    .get(&item)
                    .is_some_and(|(_, i)| matches!(i.source, Source::File { .. }));
                if let Some(db) = meta.gain_trim_db.filter(|_| file)
                    && db != self.state.trim_of(item)
                {
                    cmds.push(Command::ItemSetGainTrim { item, db });
                }
            }
        }
        self.run_batch(cmds)?;
        Ok(list.len())
    }

    fn set_weight(&mut self, playlist: &str, index: usize, weight: u32) -> Result<usize, String> {
        let p = self.playlist_id(playlist)?;
        let list = self
            .state
            .station
            .playlists
            .iter()
            .find(|x| x.id == p)
            .ok_or("That playlist is gone")?;
        let item = list.items.get(index).ok_or("That item is gone")?.id;
        let mut weights: Vec<(ItemId, u32)> = match &list.pick {
            PickOrder::Weighted { weights } => weights.clone(),
            _ => list.items.iter().map(|i| (i.id, 1)).collect(),
        };
        match weights.iter_mut().find(|(i, _)| *i == item) {
            Some(w) => w.1 = weight,
            None => weights.push((item, weight)),
        }
        self.run_edit(Command::PlaylistSetPick {
            playlist: p,
            pick: PickOrder::Weighted { weights },
        })?;
        Ok(1)
    }

    fn pages_command(pages: &[String]) -> Command {
        Command::SettingsSet {
            part: CARTWALL_PART.into(),
            key: "pages".into(),
            value: serde_json::json!(pages),
        }
    }

    fn remove_page(&mut self, page: usize) -> Result<usize, String> {
        let mut pages = self.page_names();
        if pages.len() <= 1 {
            return Err("The cartwall keeps at least one page".into());
        }
        if page >= pages.len() {
            return Err("That page is gone".into());
        }
        pages.remove(page);
        let p = u32::try_from(page).unwrap_or(u32::MAX);
        let mut cmds: Vec<Command> = self
            .state
            .carts
            .iter()
            .filter(|c| c.page >= p)
            .map(|c| Command::CartClear {
                page: c.page,
                slot: c.slot,
            })
            .collect();
        for c in self.state.carts.iter().filter(|c| c.page > p) {
            cmds.push(Command::CartAssign {
                page: c.page - 1,
                slot: c.slot,
                item: c.item,
            });
        }
        cmds.push(Self::pages_command(&pages));
        self.run_batch(cmds)?;
        Ok(1)
    }

    fn set_cart(&mut self, cart: usize, change: CartChange) -> Result<usize, String> {
        let (page, slot) = self.cart_place(cart);
        let now = self
            .state
            .cart_settings
            .iter()
            .find(|c| c.page == page && c.slot == slot)
            .copied();
        let (mut duck_db, mut mode) =
            now.map_or((0.0, ECartMode::OneShot), |c| (c.duck_db, c.mode));
        match change {
            CartChange::Duck(db) => duck_db = db.min(0.0),
            CartChange::Mode(m) => {
                mode = match m {
                    CartMode::OneShot => ECartMode::OneShot,
                    CartMode::Loop => ECartMode::Loop,
                    CartMode::Restart => ECartMode::Restart,
                };
            }
            CartChange::Colour(c) => {
                let mut colours: serde_json::Map<String, serde_json::Value> = self
                    .state
                    .settings(CARTWALL_PART)
                    .and_then(|s| s.get("colours"))
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                colours.insert(cart.to_string(), serde_json::json!(c));
                self.run_edit(Command::SettingsSet {
                    part: CARTWALL_PART.into(),
                    key: "colours".into(),
                    value: serde_json::Value::Object(colours),
                })?;
                return Ok(1);
            }
        }
        let cmd = Command::CartSet {
            page,
            slot,
            duck_db,
            mode,
        };
        if matches!(change, CartChange::Duck(_)) {
            return self.merged(format!("cart:{cart}"), cmd);
        }
        self.run_edit(cmd)?;
        Ok(1)
    }

    fn part_setting(
        &mut self,
        part: String,
        key: String,
        value: serde_json::Value,
    ) -> Result<usize, String> {
        let grid = part == CARTWALL_PART && (key == "cols" || key == "rows");
        if !grid {
            let cmd = Command::SettingsSet {
                part: part.clone(),
                key: key.clone(),
                value: value.clone(),
            };
            self.pending_settings
                .insert((part.clone(), key.clone()), value);
            return self.merged(format!("part:{part}:{key}"), cmd);
        }
        let mut cmds = vec![Command::SettingsSet {
            part: part.clone(),
            key: key.clone(),
            value: value.clone(),
        }];
        {
            let num = |k: &str| {
                if k == key {
                    value.as_u64()
                } else {
                    self.state
                        .settings(CARTWALL_PART)
                        .and_then(|s| s.get(k))
                        .and_then(serde_json::Value::as_u64)
                }
            };
            if let (Some(c), Some(r)) = (num("cols"), num("rows")) {
                cmds.extend(self.fit_cart_grid(usize::try_from(c * r).unwrap_or(1)));
            }
        }
        self.run_batch(cmds)?;
        Ok(1)
    }

    pub(crate) fn apply(&mut self, edit: Edit) -> Result<usize, String> {
        match edit {
            Edit::Insert { item, at } => {
                let lib = self
                    .library_item(item)
                    .ok_or_else(|| "That library item is gone".to_owned())?;
                let main = self.main_id().ok_or("The station has no main playlist")?;
                let row = match at {
                    Place::Before(r) => Some(self.row_index(r)?),
                    Place::After(r) => Some(self.row_index(r)? + 1),
                    Place::End => None,
                };
                if let Some(row) = row.filter(|r| self.is_next_row(*r)) {
                    let name = self.playlist_name_of(main);
                    return self.add_to(&name, vec![lib], Some(row));
                }
                let at = self.main_index(at)?;
                let items = self.copies(vec![lib]);
                self.run_edit(Command::PlaylistAddItems {
                    playlist: main,
                    items,
                    at,
                    lock: true,
                })?;
                Ok(1)
            }
            Edit::Remove(ids) => self.remove_rows(&ids),
            Edit::Move { id, to } => self.move_row(id, to),
            Edit::ToggleLock(id) => {
                let r = *self
                    .rows
                    .get(&id)
                    .ok_or_else(|| "That row is gone from the timeline".to_owned())?;
                let locked = self.items.get(&r.item).is_some_and(|(_, i)| i.locked);
                let cmd = if locked {
                    Command::ItemUnlock { item: r.item }
                } else {
                    Command::ItemLock { item: r.item }
                };
                self.run_edit(cmd)?;
                Ok(1)
            }
            Edit::RenamePlaylist { from, to } => {
                let to = to.trim().replace(['\n', '\r'], " ");
                if to.is_empty() || (to != from && self.unique_name(&to) != to) {
                    return Err(format!("The name {to:?} is empty or taken"));
                }
                let playlist = self.playlist_id(&from)?;
                self.run_edit(Command::PlaylistRename { playlist, name: to })?;
                Ok(1)
            }
            Edit::DuplicatePlaylist(name) => {
                let playlist = self.playlist_id(&name)?;
                let copy = self.unique_name(&format!("{name} copy"));
                self.run_edit(Command::PlaylistDuplicate {
                    playlist,
                    name: copy,
                })?;
                // The engine gave the copies ids after the largest id.
                self.next_id = 0;
                Ok(1)
            }
            Edit::NewPlaylist(name) => {
                let name = self.unique_name(&name);
                let colour = PALETTE[self.tabs.len() % PALETTE.len()].1;
                let max = self.state.station.playlists.iter().map(|p| p.id.0).max();
                let id = PlaylistId(max.unwrap_or(0) + 1);
                self.run_batch(vec![
                    Command::PlaylistCreate { name },
                    Command::PlaylistSetColor {
                        playlist: id,
                        color: colour,
                    },
                    Command::PlaylistSetPick {
                        playlist: id,
                        pick: PickOrder::Shuffle { seed: id.0 },
                    },
                ])?;
                Ok(1)
            }
            Edit::DeletePlaylist(name) => {
                let playlist = self.playlist_id(&name)?;
                if Some(playlist) == self.main_id() {
                    return Err("The main playlist cannot go; make another one main first".into());
                }
                self.run_edit(Command::PlaylistDelete { playlist })?;
                Ok(1)
            }
            Edit::RemovePlaylistItems { playlist, ids } => {
                let p = self.playlist_id(&playlist)?;
                let n = ids.len();
                self.run_edit(Command::PlaylistRemoveItems {
                    playlist: p,
                    items: ids.into_iter().map(ItemId).collect(),
                })?;
                Ok(n)
            }
            Edit::RemoveLibraryItems(ids) => self.remove_library_items(&ids),
            Edit::ScheduleOneOff(view) => self.schedule_one_off(view),
            Edit::RemoveOneOff(id) => {
                self.run_edit(Command::ScheduleRemove { id: ItemId(id) })?;
                Ok(1)
            }
            Edit::MovePlaylistItem {
                playlist,
                index,
                up,
            } => {
                let p = self.playlist_id(&playlist)?;
                let items = &self
                    .state
                    .station
                    .playlists
                    .iter()
                    .find(|x| x.id == p)
                    .ok_or("That playlist is gone")?
                    .items;
                let to = if up {
                    index.checked_sub(1)
                } else {
                    Some(index + 1)
                };
                let (Some(to), Some(item)) = (to.filter(|t| *t < items.len()), items.get(index))
                else {
                    return Err("The item is already at the end".into());
                };
                let item = item.id;
                self.run_edit(Command::PlaylistMoveItems {
                    playlist: p,
                    items: vec![item],
                    to,
                })?;
                Ok(1)
            }
            Edit::AssignCart { cart, item } => {
                let cmds = match item {
                    Some(item) => self.assign_cart(cart, item)?,
                    None => {
                        let (page, slot) = self.cart_place(cart);
                        vec![Command::CartClear { page, slot }]
                    }
                };
                self.run_batch(cmds)?;
                Ok(1)
            }
            Edit::SetMeta(list) => self.set_meta(&list),
            Edit::Deck { deck, command } => self.deck_command(deck, command),
            Edit::PlayNext { fade_in_s } => self.play_next(fade_in_s),
            Edit::SetTiming { .. } => Err(
                "The timing of a fixed row comes from its rule; change it in the rules of the playlist"
                    .into(),
            ),
            Edit::SetPlaylistEnabled { playlist, enabled } => {
                let p = self.playlist_id(&playlist)?;
                self.run_edit(Command::PlaylistEnable {
                    playlist: p,
                    enabled,
                })?;
                Ok(1)
            }
            Edit::SetPlaylistColour { playlist, colour } => {
                let (_, rgb) = *PALETTE.get(colour).ok_or("No such palette colour")?;
                let p = self.playlist_id(&playlist)?;
                self.run_edit(Command::PlaylistSetColor {
                    playlist: p,
                    color: rgb,
                })?;
                Ok(1)
            }
            Edit::SetWeight {
                playlist,
                index,
                weight,
            } => self.set_weight(&playlist, index, weight),
            Edit::SetStrip { strip, change } => self.set_strip(strip, change),
            Edit::SetIcecast(ice) => self.set_icecast(ice),
            Edit::Pair { address, key } => self.pair(&address, &key),
            Edit::Unpair => self.unpair(),
            Edit::SetStreaming(on) => {
                self.run_edit(Command::SettingsSet {
                    part: radio_playout::WEBRTC_PART.into(),
                    key: "enabled".into(),
                    value: on.into(),
                })?;
                Ok(1)
            }
            Edit::SetLoudness(l) => {
                self.run_edit(Command::MixerSetLoudness {
                    enabled: Some(l.enabled),
                    target_lufs: Some(l.target_lufs),
                    max_boost_db: Some(l.max_boost_db),
                })?;
                Ok(1)
            }
            Edit::SetDynamics(d) => {
                let (c, l) = (d.compressor, d.limiter);
                self.run_edit(Command::MixerSetDynamics {
                    compressor: Some(radio_engine::CompressorPatch {
                        enabled: Some(c.enabled),
                        threshold_db: Some(c.threshold_db),
                        ratio: Some(c.ratio),
                        attack_ms: Some(c.attack_ms),
                        release_ms: Some(c.release_ms),
                        makeup_db: Some(c.makeup_db),
                    }),
                    limiter: Some(radio_engine::LimiterPatch {
                        enabled: Some(l.enabled),
                        ceiling_db: Some(l.ceiling_db),
                        release_ms: Some(l.release_ms),
                    }),
                })?;
                Ok(1)
            }
            Edit::SetDevice { role, device } => {
                let key = match role {
                    DeviceRole::Out => "out",
                    DeviceRole::Monitor => radio_engine::MONITOR_KEY,
                    DeviceRole::Input => "input",
                };
                self.run_edit(Command::SettingsSet {
                    part: AUDIO_PART.into(),
                    key: key.into(),
                    value: serde_json::json!(device),
                })?;
                Ok(1)
            }
            Edit::AddCartPage { name } => {
                let mut pages = self.page_names();
                pages.push(name);
                self.run_edit(Self::pages_command(&pages))?;
                Ok(1)
            }
            Edit::RenameCartPage { page, name } => {
                let mut pages = self.page_names();
                *pages.get_mut(page).ok_or("That page is gone")? = name;
                self.run_edit(Self::pages_command(&pages))?;
                Ok(1)
            }
            Edit::RemoveCartPage { page } => self.remove_page(page),
            Edit::SetCart { cart, change } => self.set_cart(cart, change),
            Edit::FireCart { cart } => {
                let (page, slot) = self.cart_place(cart);
                self.run_audio(Command::CartFire { page, slot })
            }
            Edit::StopCarts => self.run_audio(Command::CartStopAll),
            Edit::ClearPlaylist {
                playlist,
                expect_count,
                remove_locked,
            } => {
                let p = self.playlist_id(&playlist)?;
                self.run_edit(Command::PlaylistClear {
                    playlist: p,
                    expect_count,
                    remove_locked,
                })?;
                Ok(expect_count)
            }
            Edit::RemoveDuplicates { playlist } => {
                let p = self.playlist_id(&playlist)?;
                let items = self.duplicate_items(p);
                if items.is_empty() {
                    return Ok(0);
                }
                let n = items.len();
                self.run_edit(Command::PlaylistRemoveItems { playlist: p, items })?;
                Ok(n)
            }
            Edit::AddFolder { path } => self.add_folder(&path),
            Edit::RemoveFolder { path } => self.remove_folder(&path),
            Edit::Rescan { folder } => self.rescan(&folder),
            Edit::SetMode(mode) => {
                let mode = match mode {
                    PlayMode::Auto => Mode::Auto,
                    PlayMode::Assist => Mode::Assist,
                    PlayMode::Manual => Mode::Manual,
                };
                self.run_edit(Command::ModeSet { mode })?;
                Ok(1)
            }
            Edit::StopAfterCurrent(on) => {
                let cmd = if on {
                    Command::StationStopAfterCurrent
                } else {
                    Command::StationContinue
                };
                self.run_edit(cmd)?;
                Ok(1)
            }
            Edit::Undo => {
                self.run_edit(Command::HistoryUndo)?;
                self.next_id = 0;
                Ok(1)
            }
            Edit::Redo => {
                self.run_edit(Command::HistoryRedo)?;
                self.next_id = 0;
                Ok(1)
            }
            Edit::SetPartSetting { part, key, value } => self.part_setting(part, key, value),
            Edit::AddFiles {
                playlist,
                paths,
                at,
            } => {
                // The background lane reads the files. The result comes back as a notice.
                let n = paths.len();
                let target = self.add_target(&playlist, at)?;
                self.queue_probe(paths, true, ProbeAfter::Add(target))?;
                Ok(n)
            }
            Edit::AddFolderItems {
                playlist,
                path,
                recursive,
            } => {
                let target = self.add_target(&playlist, None)?;
                self.queue_probe(vec![path], recursive, ProbeAfter::Add(target))?;
                Ok(1)
            }
            Edit::AddStream {
                playlist,
                url,
                title,
                planned_len,
                keep_alive,
                at,
            } => {
                let host = radio_ui_egui::view::check_stream_url(&url)?;
                if planned_len.is_zero() {
                    return Err("The planned length must be over 0".into());
                }
                let title = if title.trim().is_empty() {
                    host
                } else {
                    title.trim().to_owned()
                };
                let mut item = crate::plain_item(&title);
                item.source = Source::Stream {
                    url: url.trim().to_owned(),
                    keep_alive,
                };
                item.planned_len = Some(planned_len);
                item.trimmable = false;
                self.add_to(&playlist, vec![item], at)
            }
            Edit::AddLive {
                playlist,
                device,
                channels,
                planned_len,
                at,
            } => {
                if planned_len.is_zero() {
                    return Err("The planned length must be over 0".into());
                }
                let ch = |c: u32| c.checked_sub(1).and_then(|c| u16::try_from(c).ok()).ok_or_else(|| "No such input channel".to_owned());
                let title = format!("Live input · {device} ({}/{})", channels.0, channels.1);
                let mut item = crate::plain_item(&title);
                item.source = Source::LiveInput {
                    device,
                    channels: [ch(channels.0)?, ch(channels.1)?],
                };
                item.planned_len = Some(planned_len);
                item.trimmable = false;
                self.add_to(&playlist, vec![item], at)
            }
            Edit::AckAlarm(id) => {
                self.run_edit(Command::AlarmAck { id })?;
                self.pending_acks.insert(id);
                Ok(1)
            }
            Edit::AckAlarms(ids) => {
                let new: Vec<u64> = ids
                    .into_iter()
                    .filter(|id| !self.state.is_acked(*id) && !self.pending_acks.contains(id))
                    .collect();
                if new.is_empty() {
                    return Ok(0);
                }
                let n = new.len();
                self.run_edit(Command::AlarmAckAll { ids: new.clone() })?;
                self.pending_acks.extend(new);
                Ok(n)
            }
            Edit::ExportAlarms { path, ids } => {
                let rows: Vec<_> = self
                    .alarms
                    .iter()
                    .filter(|a| ids.contains(&a.id))
                    .cloned()
                    .collect();
                let worker = self
                    .worker
                    .as_ref()
                    .ok_or("The background lane is not running, so the export cannot run")?;
                let n = rows.len();
                worker
                    .jobs
                    .send(crate::bg::Job::ExportAlarms { path, rows })
                    .map_err(|_| "The background lane stopped, so the export cannot run")?;
                Ok(n)
            }
            Edit::ExportHistory {
                path,
                report,
                since,
                until,
                playlist,
            } => {
                let worker = self
                    .worker
                    .as_ref()
                    .ok_or("The background lane is not running, so the export cannot run")?;
                worker
                    .jobs
                    .send(crate::bg::Job::ExportHistory {
                        log: self.playout.log(),
                        path,
                        report,
                        since,
                        until,
                        playlist,
                        zone: self.state.station.tz.clone(),
                    })
                    .map_err(|_| "The background lane stopped, so the export cannot run")?;
                Ok(0)
            }
        }
    }

    fn set_strip(&mut self, strip: String, change: StripChange) -> Result<usize, String> {
        let (mut gain_db, mut mute, mut snd) = (None, None, None);
        match change {
            StripChange::Gain(g) => gain_db = Some(g),
            StripChange::Mute(m) => mute = Some(m),
            StripChange::Snd(p) => snd = Some(p),
        }
        let cmd = Command::MixerSetStrip {
            strip,
            gain_db,
            mute,
            snd,
            route: None,
        };
        if gain_db.is_some() {
            return self.fader(cmd);
        }
        self.run_edit(cmd)?;
        Ok(1)
    }

    pub(crate) fn drop_on(&mut self, item: u64, target: DropTarget) -> Result<usize, String> {
        match target {
            DropTarget::Main { after } => self.apply(Edit::Insert {
                item,
                at: Place::After(after),
            }),
            DropTarget::Playlist(name) => {
                let lib = self
                    .library_item(item)
                    .ok_or_else(|| "That library item is gone".to_owned())?;
                self.add_to(&name, vec![lib], None)
            }
            DropTarget::Cart(cart) => self.apply(Edit::AssignCart {
                cart,
                item: Some(item),
            }),
        }
    }

    pub(crate) fn rules(&mut self, tab: PlaylistTab) -> Result<(), String> {
        let playlist = self.playlist_id(&tab.name)?;
        let mut cmds = vec![Command::PlaylistSetRulesScript {
            playlist,
            script: tab.script.clone(),
        }];
        let now = self
            .tabs
            .iter()
            .chain(self.main.as_ref())
            .find(|t| t.name == tab.name);
        if let Some(src) = &tab.source
            && now.and_then(|t| t.source.as_ref()) != Some(src)
        {
            let value = serde_json::json!({
                "path": src.path,
                "recursive": src.recursive,
                "watch": src.watch,
            });
            cmds.push(Command::PlaylistSetSource {
                playlist,
                source: value,
            });
        }
        self.run_batch(cmds)?;
        Ok(())
    }

    /// Adds files to the library on the background lane. A notice reports the result.
    pub(crate) fn import(
        &mut self,
        paths: Vec<String>,
        category: &str,
    ) -> Result<Imported, String> {
        self.queue_probe(
            paths,
            true,
            ProbeAfter::Import {
                category: category.to_owned(),
            },
        )?;
        Ok(Imported::default())
    }

    /// The items of a playlist whose file an earlier item of it has already.
    pub(crate) fn duplicate_items(&self, playlist: PlaylistId) -> Vec<ItemId> {
        let mut seen = std::collections::HashSet::new();
        self.state
            .station
            .playlists
            .iter()
            .filter(|p| p.id == playlist)
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match &i.source {
                Source::File { path } if !seen.insert(path.clone()) => Some(i.id),
                _ => None,
            })
            .collect()
    }

    /// Removes library items, as one undo step. The library has one row per file, so every
    /// playlist item of that file goes. A folder file is in no playlist; its folder brings it.
    /// An item on a deck keeps them all.
    fn remove_library_items(&mut self, ids: &[u64]) -> Result<usize, String> {
        let paths: Vec<&str> = self
            .library
            .iter()
            .filter(|l| ids.contains(&l.id))
            .map(|l| l.path.as_str())
            .collect();
        let on_deck = self.playout.deck_items();
        let mut cmds = Vec::new();
        let mut n = 0;
        // One-offs are copies for Upcoming: they stay when their file leaves the Library.
        let playlists = self
            .state
            .station
            .playlists
            .iter()
            .filter(|p| p.role != radio_model::PlaylistRole::OneOff);
        for p in playlists {
            let items: Vec<&radio_model::Item> = p
                .items
                .iter()
                .filter(|i| paths.contains(&crate::source_text(&i.source).as_str()))
                .collect();
            if let Some(i) = items.iter().find(|i| on_deck.contains(&Some(i.id))) {
                return Err(format!(
                    "“{}” is on a deck, so the items stay. Select them without it.",
                    i.title
                ));
            }
            if items.is_empty() {
                continue;
            }
            n += items.len();
            cmds.push(Command::PlaylistRemoveItems {
                playlist: p.id,
                items: items.iter().map(|i| i.id).collect(),
            });
        }
        if n == 0 {
            return Err(
                "Files of a folder leave the Library with their folder. Remove the folder instead."
                    .to_owned(),
            );
        }
        self.run_batch(cmds)?;
        Ok(n)
    }

    pub(crate) fn find_ref(&self, title: &str) -> Option<u64> {
        self.library
            .iter()
            .chain(self.files.iter().map(|f| &f.view))
            .find(|l| {
                l.title == title
                    || l.artist
                        .as_ref()
                        .is_some_and(|a| format!("{a} – {}", l.title) == title)
            })
            .map(|l| l.id)
    }

    pub(crate) fn set_main(&mut self, name: &str) -> Result<(), String> {
        let playlist = self.playlist_id(name)?;
        self.run_edit(Command::PlaylistSetMain { playlist })?;
        Ok(())
    }
}

/// A file item with the length and the tags of its header. A file that the probe cannot
/// read gives `None`; playout finds the cue points later.
pub fn file_item(path: &str) -> Option<Item> {
    let (len, meta) = radio_library::probe(path).ok()?;
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_owned();
    let title = if meta.title.is_empty() {
        stem
    } else {
        meta.title.clone()
    };
    let mut item = crate::plain_item(&title);
    item.artist = (!meta.artist.is_empty()).then(|| meta.artist.clone());
    item.meta = meta;
    item.source = Source::File {
        path: path.to_owned(),
    };
    item.planned_len = Some(len);
    Some(item)
}
