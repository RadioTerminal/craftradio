//! The edit commands of the mock. Each returns the number of items that changed.

use super::*;

fn gone(what: &str) -> String {
    format!("preview — {what} is gone")
}

/// The undo stack keeps at least this many steps.
const UNDO_STEPS: usize = 100;

/// The state that an undo step restores.
#[derive(Clone)]
pub(super) struct Saved {
    ops: Vec<Op>,
    next_manual: u64,
    tabs: Vec<PlaylistTab>,
    main: PlaylistTab,
    main_tracks: Option<Vec<(String, Duration)>>,
    carts: Vec<String>,
    cartwall: CartwallView,
    row_meta: HashMap<u64, ItemMeta>,
    row_timing: HashMap<u64, Timing>,
    library: Vec<LibraryItem>,
}

/// True for an edit of playlists or carts that Undo can take back.
fn undoable(edit: &Edit) -> bool {
    !matches!(
        edit,
        Edit::Deck { .. }
            | Edit::SetStrip { .. }
            | Edit::SetIcecast(_)
            | Edit::SetDevice { .. }
            | Edit::AddFolder { .. }
            | Edit::RemoveFolder { .. }
            | Edit::Rescan { .. }
            | Edit::SetPartSetting { .. }
            | Edit::SetMode(_)
            | Edit::StopAfterCurrent(_)
            | Edit::FireCart { .. }
            | Edit::StopCarts
            | Edit::Undo
            | Edit::Redo
    )
}

impl MockSource {
    fn save(&self) -> Saved {
        Saved {
            ops: self.ops.clone(),
            next_manual: self.next_manual,
            tabs: self.tabs.clone(),
            main: self.main.clone(),
            main_tracks: self.main_tracks.clone(),
            carts: self.carts.clone(),
            cartwall: self.cartwall.clone(),
            row_meta: self.row_meta.clone(),
            row_timing: self.row_timing.clone(),
            library: self.library.clone(),
        }
    }

    fn restore(&mut self, s: Saved) {
        let main_changed = s.main.name != self.main.name;
        self.ops = s.ops;
        self.next_manual = s.next_manual;
        self.tabs = s.tabs;
        self.main = s.main;
        self.main_tracks = s.main_tracks;
        self.carts = s.carts;
        self.cartwall = s.cartwall;
        self.row_meta = s.row_meta;
        self.row_timing = s.row_timing;
        self.library = s.library;
        if main_changed {
            self.cache = None;
        }
    }

    /// Apply one edit. An undoable edit that works pushes the state before it onto the undo stack.
    pub(super) fn apply_edit(&mut self, edit: Edit) -> Result<usize, String> {
        match edit {
            Edit::Undo => {
                let Some(before) = self.undo.pop() else {
                    return Err("preview — nothing to undo".to_owned());
                };
                let now = self.save();
                self.redo.push(now);
                self.restore(before);
                return Ok(1);
            }
            Edit::Redo => {
                let Some(after) = self.redo.pop() else {
                    return Err("preview — nothing to redo".to_owned());
                };
                let now = self.save();
                self.undo.push(now);
                self.restore(after);
                return Ok(1);
            }
            _ => {}
        }
        // Edits inside another edit, such as each insert of Add files, make no step of their own.
        let outer = self.edit_depth == 0 && undoable(&edit);
        let before = outer.then(|| self.save());
        self.edit_depth += 1;
        let result = self.apply_edit_inner(edit);
        self.edit_depth -= 1;
        if let (Ok(_), Some(before)) = (&result, before) {
            self.undo.push(before);
            if self.undo.len() > UNDO_STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        result
    }

    /// The rows on air and on the decks, which a remove must not take.
    fn protected_rows(&mut self) -> Vec<(u64, String)> {
        let now = self.last_now;
        let rows = self.current();
        let Some(i) = rows
            .iter()
            .position(|r| r.start.timestamp() <= now && now < end_of(r))
        else {
            return Vec::new();
        };
        rows.iter()
            .skip(i)
            .take(DECKS)
            .enumerate()
            .map(|(k, r)| {
                let why = if k == 0 {
                    format!("{} is on air; use Skip instead", r.title)
                } else {
                    format!("{} is loaded on deck {}", r.title, ['A', 'B', 'C', 'D'][k])
                };
                (r.id, why)
            })
            .collect()
    }

    fn apply_edit_inner(&mut self, edit: Edit) -> Result<usize, String> {
        self.ensure_item_ids();
        match edit {
            Edit::Undo | Edit::Redo => Err("preview — undo cannot nest".to_owned()),
            Edit::SetMode(mode) => {
                self.play_mode = mode;
                Ok(1)
            }
            // The preview plays no audio. The cartwall flashes the cart instead.
            Edit::FireCart { cart } => match self.carts.get(cart) {
                Some(c) if !c.is_empty() => Ok(1),
                _ => Err(gone("that cart")),
            },
            Edit::StopCarts => Ok(1),
            Edit::RemoveDuplicates { playlist } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(gone(&format!("the playlist {playlist}")));
                };
                let mut seen = std::collections::HashSet::new();
                let mut gone_n = 0;
                let mut k = 0;
                while k < tab.items.len() {
                    if seen.insert(tab.items[k].0.clone()) {
                        k += 1;
                    } else {
                        tab.items.remove(k);
                        if k < tab.weights.len() {
                            tab.weights.remove(k);
                        }
                        if k < tab.ids.len() {
                            tab.ids.remove(k);
                        }
                        gone_n += 1;
                    }
                }
                Ok(gone_n)
            }
            Edit::StopAfterCurrent(on) => {
                self.stop_after_current = on;
                Ok(1)
            }
            Edit::Insert { item, at } => {
                let Some(lib) = self.library_item(item) else {
                    return Err(gone("that library item"));
                };
                let title = match &lib.artist {
                    Some(a) => format!("{a} – {}", lib.title),
                    None => lib.title.clone(),
                };
                let len = if lib.len.is_zero() {
                    estimated_len(&title)
                } else {
                    lib.len
                };
                let id = self.next_manual;
                self.next_manual += 1;
                self.try_op(Op::Insert {
                    at,
                    id,
                    item,
                    title,
                    len,
                })
            }
            Edit::Remove(ids) => {
                let protected = self.protected_rows();
                if let Some((_, why)) = protected.iter().find(|(id, _)| ids.contains(id)) {
                    return Err(format!("preview — {why}"));
                }
                let n = ids.len();
                let Some(top) = self.last_top.clone() else {
                    return Err("preview — no timeline yet".to_owned());
                };
                let mut ops = self.ops.clone();
                ops.extend(ids.into_iter().map(Op::Remove));
                let mut rows = self.rows_from(&top).to_vec();
                if self.apply_ops(&mut rows, &ops) {
                    self.ops = ops;
                    Ok(n)
                } else {
                    Err(
                        "preview — nothing is left to fill the gap before the next fixed item"
                            .to_owned(),
                    )
                }
            }
            Edit::Move { id, to } => self.move_row(id, to),
            Edit::ToggleLock(id) => self.try_op(Op::Lock(id)),
            Edit::RenamePlaylist { from, to } => {
                let to = to.trim().replace(['\n', '\r'], " ");
                if to.is_empty() || self.unique_name(&to) != to {
                    return Err(format!("preview — the name {to:?} is empty or taken"));
                }
                let Some(tab) = self.tab_mut(&from) else {
                    return Err(gone(&format!("the playlist {from}")));
                };
                tab.name = to;
                rules_script::refresh(tab);
                self.cache = None;
                Ok(1)
            }
            Edit::DuplicatePlaylist(name) => {
                let Some(tab) = self
                    .tabs
                    .iter()
                    .chain([&self.main])
                    .find(|t| t.name == name)
                else {
                    return Err(gone(&format!("the playlist {name}")));
                };
                let mut copy = tab.clone();
                copy.name = self.unique_name(&format!("{name} copy"));
                rules_script::refresh(&mut copy);
                self.tabs.push(copy);
                Ok(1)
            }
            Edit::NewPlaylist(name) => {
                let name = self.unique_name(&name);
                let colour = crate::theme::PALETTE[self.tabs.len() % crate::theme::PALETTE.len()].1;
                self.tabs.push(playlist(
                    &name,
                    colour,
                    Pick::Shuffle,
                    Vec::new(),
                    Vec::new(),
                ));
                Ok(1)
            }
            Edit::DeletePlaylist(name) => {
                if self.main.name == name {
                    return Err(
                        "preview — the main playlist cannot go; make another one main first"
                            .to_owned(),
                    );
                }
                let before = self.tabs.len();
                self.tabs.retain(|t| t.name != name);
                if self.tabs.len() == before {
                    return Err(gone(&format!("the playlist {name}")));
                }
                Ok(1)
            }
            Edit::RemovePlaylistItem { playlist, id } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(gone(&format!("the playlist {playlist}")));
                };
                let Some(index) = tab.ids.iter().position(|i| *i == id) else {
                    return Err(gone("that item"));
                };
                tab.ids.remove(index);
                if index < tab.items.len() {
                    tab.items.remove(index);
                }
                if index < tab.weights.len() {
                    tab.weights.remove(index);
                }
                Ok(1)
            }
            Edit::MovePlaylistItem {
                playlist,
                index,
                up,
            } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(gone(&format!("the playlist {playlist}")));
                };
                let other = if up {
                    index.checked_sub(1)
                } else {
                    Some(index + 1)
                };
                match other.filter(|o| *o < tab.items.len() && index < tab.items.len()) {
                    Some(o) => {
                        tab.items.swap(index, o);
                        if o < tab.weights.len() && index < tab.weights.len() {
                            tab.weights.swap(index, o);
                        }
                        if o < tab.ids.len() && index < tab.ids.len() {
                            tab.ids.swap(index, o);
                        }
                        Ok(1)
                    }
                    None => Err("preview — the item is already at the end".to_owned()),
                }
            }
            Edit::SetMeta(list) => {
                let mut n = 0;
                for (target, m) in list {
                    match target {
                        MetaTarget::Row(id) => {
                            self.row_meta.insert(id, m);
                            n += 1;
                        }
                        MetaTarget::Library(id) => {
                            if let Some(l) = self.library.iter_mut().find(|l| l.id == id) {
                                l.meta = m;
                                n += 1;
                            }
                        }
                    }
                }
                Ok(n)
            }
            Edit::Deck { deck, command } => self.deck_command(deck, command),
            Edit::AssignCart { cart, item } => {
                let label = match item {
                    Some(id) => match self.library_item(id) {
                        Some(l) => l.title.clone(),
                        None => return Err(gone("that library item")),
                    },
                    None => String::new(),
                };
                let Some(slot) = self.carts.get_mut(cart) else {
                    return Err(gone("that cart"));
                };
                *slot = label;
                Ok(1)
            }
            Edit::SetTiming { row, timing } => {
                match timing {
                    Some(t) => self.row_timing.insert(row, t),
                    None => self.row_timing.remove(&row),
                };
                Ok(1)
            }
            Edit::SetPlaylistColour { playlist, colour } => {
                let Some((_, rgb)) = crate::theme::PALETTE.get(colour).copied() else {
                    return Err(format!("no palette colour {colour}"));
                };
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(gone(&format!("the playlist {playlist}")));
                };
                tab.color = rgb;
                Ok(1)
            }
            Edit::SetPlaylistEnabled { playlist, enabled } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(gone(&format!("the playlist {playlist}")));
                };
                tab.enabled = enabled;
                Ok(1)
            }
            Edit::SetWeight {
                playlist,
                index,
                weight,
            } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(gone(&format!("the playlist {playlist}")));
                };
                tab.weights.resize(tab.items.len(), 1);
                let Some(w) = tab.weights.get_mut(index) else {
                    return Err(gone("that item"));
                };
                *w = weight;
                Ok(1)
            }
            Edit::SetStrip { strip, change } => {
                let st = self.strips.entry(strip).or_default();
                match change {
                    StripChange::Gain(g) => st.gain_db = g,
                    StripChange::Mute(m) => st.mute = m,
                    StripChange::Pfl(p) => st.pfl = p,
                    StripChange::Route(r) => st.route = r,
                }
                Ok(1)
            }
            Edit::SetIcecast(ice) => {
                self.icecast = ice;
                Ok(1)
            }
            Edit::SetDevice { role, device } => {
                if !DEVICES.contains(&device.as_str()) {
                    return Err(format!("preview — no device {device}"));
                }
                match role {
                    DeviceRole::Out => self.device_roles.out = device,
                    DeviceRole::Pfl => self.device_roles.pfl = device,
                    DeviceRole::Input => self.device_roles.input = device,
                }
                Ok(1)
            }
            Edit::AddCartPage { name } => {
                self.cartwall.pages.push(name);
                let n = self.cartwall.per_page;
                self.carts.extend(std::iter::repeat_n(String::new(), n));
                self.cartwall
                    .slots
                    .extend(std::iter::repeat_n(DEFAULT_SLOT, n));
                Ok(1)
            }
            Edit::RenameCartPage { page, name } => {
                let Some(p) = self.cartwall.pages.get_mut(page) else {
                    return Err(gone("that page"));
                };
                *p = name;
                Ok(1)
            }
            Edit::RemoveCartPage { page } => {
                if self.cartwall.pages.len() <= 1 {
                    return Err("preview — the cartwall keeps at least one page".to_owned());
                }
                if page >= self.cartwall.pages.len() {
                    return Err(gone("that page"));
                }
                self.cartwall.pages.remove(page);
                let per = self.cartwall.per_page;
                let from = (page * per).min(self.carts.len());
                let to = (from + per).min(self.carts.len());
                self.carts.drain(from..to);
                self.cartwall
                    .slots
                    .drain(from..to.min(self.cartwall.slots.len()));
                Ok(1)
            }
            Edit::SetCart { cart, change } => {
                self.cartwall.slots.resize(self.carts.len(), DEFAULT_SLOT);
                let Some(slot) = self.cartwall.slots.get_mut(cart) else {
                    return Err(gone("that cart"));
                };
                match change {
                    CartChange::Duck(db) => slot.duck_db = db,
                    CartChange::Mode(m) => slot.mode = m,
                    CartChange::Colour(c) => slot.colour = c,
                }
                Ok(1)
            }
            Edit::AddFolder { path } => {
                let path = path.trim().to_owned();
                if path.is_empty() {
                    return Err("preview — the folder path is empty".to_owned());
                }
                Ok(self.scan_folder(&path))
            }
            Edit::RemoveFolder { path } => {
                let before = self.folders.len();
                self.folders.retain(|f| f.path != path);
                if self.folders.len() == before {
                    return Err(gone(&format!("the folder {path}")));
                }
                Ok(1)
            }
            Edit::Rescan { folder } => {
                if !self.folders.iter().any(|f| f.path == folder) {
                    return Err(gone(&format!("the folder {folder}")));
                }
                Ok(self.scan_folder(&folder))
            }
            Edit::AddFiles {
                playlist,
                paths,
                at,
            } => {
                let imported = self.import_files(paths, "Unsorted")?;
                if imported.ids.is_empty() {
                    return Err(format!(
                        "preview — no audio files to add · skipped {}",
                        imported.skipped
                    ));
                }
                self.add_to(&playlist, &imported.ids, at)
            }
            Edit::AddFolderItems {
                playlist,
                path,
                recursive,
            } => {
                let scan = folders::scan(std::path::Path::new(&path), recursive, self.limits);
                if scan.files.is_empty() {
                    let why = scan.errors.first().cloned().unwrap_or_default();
                    return Err(format!("preview — no audio files in {path} {why}"));
                }
                let ids = self.add_library_files(&scan.files, "Unsorted");
                self.add_to(&playlist, &ids, None)
            }
            Edit::AddStream {
                playlist,
                url,
                title,
                planned_len,
                keep_alive,
                at,
            } => {
                let host =
                    crate::view::check_stream_url(&url).map_err(|e| format!("preview — {e}"))?;
                // The default title is the host, never the full address.
                let title = if title.trim().is_empty() {
                    host
                } else {
                    title.trim().to_owned()
                };
                let id = self.add_special(&title, &url, planned_len, "Streams");
                if let Some(l) = self.library.iter_mut().find(|l| l.id == id) {
                    l.meta.keep_alive = Some(keep_alive);
                }
                self.add_to(&playlist, &[id], at)
            }
            Edit::AddLive {
                playlist,
                device,
                channels,
                planned_len,
                at,
            } => {
                if !DEVICES.contains(&device.as_str()) {
                    return Err(format!("preview — no device {device}"));
                }
                let title = format!("Live input · {device} ({}/{})", channels.0, channels.1);
                let path = format!("live://{device}");
                let id = self.add_special(&title, &path, planned_len, "Live");
                self.add_to(&playlist, &[id], at)
            }
            Edit::SetPartSetting { part, key, value } => {
                self.settings
                    .entry(part.clone())
                    .or_default()
                    .insert(key, value);
                if part == CARTWALL_PART {
                    self.fit_cart_grid();
                }
                Ok(1)
            }
        }
    }

    /// Give every playlist item an id and a weight. New items at the end get new ids.
    pub(super) fn ensure_item_ids(&mut self) {
        let mut next = self.next_item_id;
        for tab in self.tabs.iter_mut().chain(std::iter::once(&mut self.main)) {
            tab.ids.truncate(tab.items.len());
            while tab.ids.len() < tab.items.len() {
                tab.ids.push(next);
                next += 1;
            }
            tab.weights.resize(tab.items.len(), 1);
        }
        self.next_item_id = next;
    }

    /// Add a library item that is not a file, such as a stream. It returns the new id.
    fn add_special(&mut self, title: &str, path: &str, len: Duration, category: &str) -> u64 {
        let id = self.next_library_id();
        self.library.push(LibraryItem {
            id,
            title: title.to_owned(),
            artist: None,
            len,
            category: category.to_owned(),
            path: path.to_owned(),
            meta: from_title(title, PublishMode::FromTags),
        });
        id
    }

    /// Put library items into a playlist at item index `at`, or at the end.
    /// For the main playlist, `at` is an index into the rows of the last snapshot.
    fn add_to(&mut self, playlist: &str, ids: &[u64], at: Option<usize>) -> Result<usize, String> {
        if playlist == self.main.name {
            let top = self.last_top.clone();
            let rows: Vec<Row> = self
                .current()
                .into_iter()
                .filter(|r| top.as_ref().is_none_or(|t| r.start >= *t))
                .collect();
            let place = at
                .and_then(|i| rows.get(i))
                .map_or(Place::End, |r| Place::Before(r.id));
            let ordered: Vec<u64> = match place {
                Place::End => ids.to_vec(),
                _ => ids.iter().rev().copied().collect(),
            };
            let mut placed = 0;
            for item in ordered {
                placed += self.apply_edit(Edit::Insert { item, at: place })?;
            }
            return Ok(placed);
        }
        let entries: Vec<(String, Duration)> = ids
            .iter()
            .filter_map(|id| self.library_item(*id))
            .map(|l| (l.title.clone(), l.len))
            .collect();
        let mut next = self.next_item_id;
        let Some(tab) = self.tab_mut(playlist) else {
            return Err(gone(&format!("the playlist {playlist}")));
        };
        tab.weights.resize(tab.items.len(), 1);
        let at = at.unwrap_or(tab.items.len()).min(tab.items.len());
        let n = entries.len();
        tab.ids.truncate(tab.items.len());
        for (k, entry) in entries.into_iter().enumerate() {
            tab.items.insert(at + k, entry);
            tab.weights.insert(at + k, 1);
            tab.ids.insert(at + k, next);
            next += 1;
        }
        self.next_item_id = next;
        Ok(n)
    }

    fn move_row(&mut self, id: u64, to: MoveTo) -> Result<usize, String> {
        let rows = self.current();
        let Some(i) = rows.iter().position(|r| r.id == id) else {
            return Err(gone("that row"));
        };
        let at = match to {
            MoveTo::Next => {
                let now = self.last_now;
                match rows
                    .iter()
                    .find(|r| r.start.timestamp() <= now && now < end_of(r))
                {
                    Some(r) => Place::After(r.id),
                    None => return Err("preview — nothing is on air".to_owned()),
                }
            }
            MoveTo::Top => match rows[..i].iter().rev().find(|r| r.fixed.is_some()) {
                Some(r) => Place::After(r.id),
                None => Place::Before(rows[0].id),
            },
            MoveTo::Bottom => match rows[i + 1..].iter().find(|r| r.fixed.is_some()) {
                Some(r) => Place::Before(r.id),
                None => Place::End,
            },
        };
        self.try_op(Op::Move { id, at })
    }

    /// Make each cart page as big as the grid in the cartwall settings.
    /// Assigned carts keep their order. Carts that do not fit move to the next page.
    fn fit_cart_grid(&mut self) {
        let cols = self.setting_u64(CARTWALL_PART, CART_COLS_KEY);
        let rows = self.setting_u64(CARTWALL_PART, CART_ROWS_KEY);
        let (Some(c), Some(r)) = (cols, rows) else {
            return;
        };
        let per = usize::try_from(c * r).unwrap_or(CARTS_PER_PAGE).max(1);
        if per == self.cartwall.per_page {
            return;
        }
        let old = self.cartwall.per_page.max(1);
        // Each old page keeps its carts in order. Overflow goes onto new pages after it.
        let mut carts = Vec::new();
        let mut slots = Vec::new();
        let mut pages = Vec::new();
        for (p, name) in self.cartwall.pages.iter().enumerate() {
            let from = (p * old).min(self.carts.len());
            let to = (from + old).min(self.carts.len());
            let mut used: Vec<(String, CartSlot)> = (from..to)
                .filter(|k| !self.carts[*k].is_empty())
                .map(|k| {
                    (
                        self.carts[k].clone(),
                        self.cartwall.slots.get(k).copied().unwrap_or(DEFAULT_SLOT),
                    )
                })
                .collect();
            let mut first = true;
            loop {
                let take = used.len().min(per);
                let chunk: Vec<(String, CartSlot)> = used.drain(..take).collect();
                pages.push(if first {
                    name.clone()
                } else {
                    format!("{name} (more)")
                });
                first = false;
                for k in 0..per {
                    match chunk.get(k) {
                        Some((label, slot)) => {
                            carts.push(label.clone());
                            slots.push(*slot);
                        }
                        None => {
                            carts.push(String::new());
                            slots.push(DEFAULT_SLOT);
                        }
                    }
                }
                if used.is_empty() {
                    break;
                }
            }
        }
        self.carts = carts;
        self.cartwall.slots = slots;
        self.cartwall.pages = pages;
        self.cartwall.per_page = per;
    }
}
