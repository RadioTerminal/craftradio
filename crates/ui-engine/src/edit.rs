//! Each [`Edit`] of the UI as engine commands.

use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use radio_engine::{CartMode as ECartMode, Command, Mode, Outcome, Route as ERoute};
use radio_model::{Item, ItemId, PickOrder, PlaylistId, Source};
use radio_ui_egui::theme::PALETTE;
use radio_ui_egui::view::{
    CartChange, CartMode, DeckCommand, DeckState, DeviceRole, DropTarget, Edit, Imported,
    MetaTarget, MoveTo, Place, PlayMode, PlaylistTab, Route, StripChange,
};

use crate::map::{display_title, model_meta};
use crate::{CARTWALL_PART, EngineSource};

/// A station edit waits this long for the engine to answer.
const EDIT_WAIT: Duration = Duration::from_secs(2);
/// An audio command waits this long for a refusal. A later answer only confirms the audio.
const AUDIO_WAIT: Duration = Duration::from_millis(300);
/// After an edit, the source waits this long for the plan that uses it.
const PLAN_WAIT: Duration = Duration::from_millis(500);
/// The name of the playlist that holds library files for carts.
const CART_ITEMS: &str = "Cart items";
/// The part of `settings_by_part` for the audio devices.
pub(crate) const AUDIO_PART: &str = "audio";

impl EngineSource {
    /// Runs one engine command and waits for its answer.
    pub(crate) fn run(&self, cmd: Command) -> Result<Outcome, String> {
        let rx = self.playout.send(cmd).map_err(|e| e.to_string())?;
        match rx.recv_timeout(EDIT_WAIT) {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(RecvTimeoutError::Timeout) => Err("the playout did not answer in time".into()),
            Err(RecvTimeoutError::Disconnected) => Err("the playout has stopped".into()),
        }
    }

    /// Runs a station edit, then waits until the snapshot shows it.
    pub(crate) fn run_edit(&mut self, cmd: Command) -> Result<Outcome, String> {
        let before = self.playout.revision();
        let out = self.run(cmd)?;
        let deadline = Instant::now() + PLAN_WAIT;
        while self.playout.revision() == before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(out)
    }

    /// Runs station edits as one undo step.
    pub(crate) fn run_batch(&mut self, mut commands: Vec<Command>) -> Result<Outcome, String> {
        match commands.len() {
            0 => Ok(Outcome::default()),
            1 => self.run_edit(commands.remove(0)),
            _ => self.run_edit(Command::StationBatch { commands }),
        }
    }

    /// Runs an audio command. A refusal comes back as an error.
    fn run_audio(&self, cmd: Command) -> Result<usize, String> {
        let rx = self.playout.send(cmd).map_err(|e| e.to_string())?;
        match rx.recv_timeout(AUDIO_WAIT) {
            Ok(Ok(o)) if o.audio_required && !o.applied_to_audio => Err(o
                .note
                .unwrap_or_else(|| "The deck holds another item now; try again".into())),
            Ok(Ok(_)) => Ok(1),
            Ok(Err(e)) => Err(e.to_string()),
            // The engine took the command. The mixer confirms it later.
            Err(RecvTimeoutError::Timeout) => Ok(1),
            Err(RecvTimeoutError::Disconnected) => Err("the playout has stopped".into()),
        }
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
    fn copies(&mut self, items: Vec<Item>) -> Vec<Item> {
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

    /// Adds items to a playlist. For the main playlist, `at` is a row index.
    fn add_to(
        &mut self,
        playlist: &str,
        items: Vec<Item>,
        at: Option<usize>,
    ) -> Result<usize, String> {
        let id = self.playlist_id(playlist)?;
        let at = if Some(id) == self.main_id() {
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
        let n = items.len();
        let items = self.copies(items);
        self.run_edit(Command::PlaylistAddItems {
            playlist: id,
            items,
            at,
        })?;
        Ok(n)
    }

    /// Items for audio files and folders, each with the length and tags of its header.
    pub(crate) fn probe_paths(&self, paths: &[String]) -> (Vec<Item>, usize) {
        let mut files = Vec::new();
        let mut skipped = 0;
        for p in paths {
            let path = Path::new(p);
            if path.is_dir() {
                let list = radio_library::list_audio(path, false, self.limits);
                skipped += list.errors.len();
                files.extend(list.files);
            } else {
                files.push(p.clone());
            }
        }
        files.truncate(self.limits.max_files);
        let mut items = Vec::new();
        for f in files {
            match file_item(&f) {
                Some(item) => items.push(item),
                None => skipped += 1,
            }
        }
        (items, skipped)
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

    fn deck_command(&self, deck: usize, command: DeckCommand) -> Result<usize, String> {
        let d = u8::try_from(deck).map_err(|_| "No such deck".to_owned())?;
        let name = ['A', 'B', 'C', 'D'].get(deck).copied().unwrap_or('?');
        let expect = self
            .decks
            .get(deck)
            .copied()
            .flatten()
            .ok_or_else(|| format!("Deck {name} is empty"))?;
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
            },
            DeckCommand::Fade(_) => Command::DeckFade {
                deck: d,
                expect_item,
            },
            DeckCommand::Skip => Command::DeckSkip {
                deck: d,
                expect_item,
            },
        };
        self.run_audio(cmd)
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
                return Err(format!("{title} is on air; use Skip instead"));
            }
            match by_list.iter_mut().find(|(p, _)| *p == r.playlist) {
                Some((_, v)) if !v.contains(&r.item) => v.push(r.item),
                Some(_) => {}
                None => by_list.push((r.playlist, vec![r.item])),
            }
        }
        let cmds = by_list
            .into_iter()
            .map(|(playlist, items)| Command::PlaylistRemoveItems { playlist, items })
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
        let out = self.run_edit(Command::CartSet {
            page,
            slot,
            duck_db,
            mode,
        })?;
        match out.note {
            Some(note) => Err(format!("Saved, but {note}")),
            None => Ok(1),
        }
    }

    fn part_setting(
        &mut self,
        part: String,
        key: String,
        value: serde_json::Value,
    ) -> Result<usize, String> {
        let mut cmds = vec![Command::SettingsSet {
            part: part.clone(),
            key: key.clone(),
            value: value.clone(),
        }];
        if part == CARTWALL_PART && (key == "cols" || key == "rows") {
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
                let at = self.main_index(at)?;
                let items = self.copies(vec![lib]);
                self.run_edit(Command::PlaylistAddItems {
                    playlist: main,
                    items,
                    at,
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
            Edit::RemovePlaylistItem { playlist, id } => {
                let p = self.playlist_id(&playlist)?;
                self.run_edit(Command::PlaylistRemoveItems {
                    playlist: p,
                    items: vec![ItemId(id)],
                })?;
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
            Edit::SetDevice { role, device } => {
                let key = match role {
                    DeviceRole::Out => "out",
                    DeviceRole::Pfl => "pfl",
                    DeviceRole::Input => "input",
                };
                self.run_edit(Command::SettingsSet {
                    part: AUDIO_PART.into(),
                    key: key.into(),
                    value: serde_json::json!(device),
                })?;
                Err(format!(
                    "Saved: the app uses {device} from its next start"
                ))
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
                let (items, skipped) = self.probe_paths(&paths);
                if items.is_empty() {
                    return Err(format!("No audio files to add · skipped {skipped}"));
                }
                self.add_to(&playlist, items, at)
            }
            Edit::AddFolderItems {
                playlist,
                path,
                recursive,
            } => {
                let list = radio_library::list_audio(Path::new(&path), recursive, self.limits);
                let (items, _) = self.probe_paths(&list.files);
                if items.is_empty() {
                    let why = list.errors.first().cloned().unwrap_or_default();
                    return Err(format!("No audio files in {path} {why}"));
                }
                self.add_to(&playlist, items, None)
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
                let ch = |c: u32| u16::try_from(c).map_err(|_| "No such input channel".to_owned());
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
        }
    }

    fn set_strip(&mut self, strip: String, change: StripChange) -> Result<usize, String> {
        let (mut gain_db, mut mute, mut pfl, mut route) = (None, None, None, None);
        match change {
            StripChange::Gain(g) => gain_db = Some(g),
            StripChange::Mute(m) => mute = Some(m),
            StripChange::Pfl(p) => pfl = Some(p),
            StripChange::Route(r) => {
                route = Some(match r {
                    Route::Out => ERoute::Out,
                    Route::OutAndCue => ERoute::OutAndCue,
                    Route::CueOnly => ERoute::CueOnly,
                });
            }
        }
        let cmd = Command::MixerSetStrip {
            strip,
            gain_db,
            mute,
            pfl,
            route,
        };
        if gain_db.is_some() {
            return self.fader(cmd);
        }
        let out = self.run(cmd)?;
        match out.note {
            Some(note) => Err(format!("Saved, but {note}")),
            None => Ok(1),
        }
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

    pub(crate) fn import(&mut self, paths: &[String], category: &str) -> Imported {
        let (items, skipped) = self.probe_paths(paths);
        let ids = items
            .into_iter()
            .map(|item| self.add_file(item, category))
            .collect();
        Imported { ids, skipped }
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
pub(crate) fn file_item(path: &str) -> Option<Item> {
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
