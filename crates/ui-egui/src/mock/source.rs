//! The view source commands of the mock.

use super::*;

impl ViewSource for MockSource {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let local = now.to_zoned(self.tz.clone());
        let top = local
            .with()
            .minute(0)
            .second(0)
            .subsec_nanosecond(0)
            .build()
            .unwrap_or_else(|_| local.clone());
        self.last_top = Some(top.clone());
        self.last_now = now;
        if let Some((from, secs)) = self.fade {
            let end = SignedDuration::try_from_secs_f32(secs).map_or(from, |d| from + d);
            if now >= end {
                self.fade = None;
                self.skip_on_air(end);
            }
        }
        let mut all = self.rows_from(&top).to_vec();
        let ops = self.ops.clone();
        self.apply_ops(&mut all, &ops);
        for r in &mut all {
            if let Some(m) = self.row_meta.get(&r.id) {
                r.meta = m.clone();
            }
        }
        let since = now
            .checked_sub(SignedDuration::from_hours(PAST_HOURS))
            .unwrap_or(now);
        let history = all
            .iter()
            .filter(|r| r.start.timestamp() >= since && r.start.timestamp() <= now)
            .cloned()
            .collect();
        let rows: Vec<Row> = all.into_iter().filter(|r| r.start >= top).collect();

        let on_air = rows.iter().position(|r| {
            let start = r.start.timestamp();
            let end = SignedDuration::try_from(r.len).map_or(start, |d| start + d);
            start <= now && now < end
        });
        let elapsed = on_air.map_or(Duration::ZERO, |i| {
            remaining(rows[i].start.timestamp(), now)
        });
        let at = |k: usize| on_air.and_then(|i| rows.get(i + k));
        let a_state = if self.fade.is_some() {
            DeckState::Fading
        } else if self.paused_at.is_some() {
            DeckState::Paused
        } else {
            DeckState::Playing
        };
        let elapsed = match self.paused_at {
            Some(p) => on_air.map_or(elapsed, |i| remaining(rows[i].start.timestamp(), p)),
            None => elapsed,
        };
        let decks = [
            deck('A', at(0), a_state, elapsed),
            deck('B', at(1), DeckState::Ready, Duration::ZERO),
            deck('C', at(2), DeckState::Ready, Duration::ZERO),
            deck('D', at(3), DeckState::Ready, Duration::ZERO),
        ];
        let meters = meters(now, &decks);

        StationView {
            now: local,
            mode: "AUTO".to_owned(),
            decks,
            rows,
            tabs: self.tabs.clone(),
            main: self.main.clone(),
            main_playlist: self.main.name.clone(),
            station: STATION.to_owned(),
            meters,
            history,
            library: self.library.clone(),
            carts: self.carts.clone(),
            devices: DEVICES.iter().map(|d| (*d).to_owned()).collect(),
            warnings: 0,
        }
    }

    fn drop_item(&mut self, item: u64, target: DropTarget) -> bool {
        let Some(lib) = self.library.iter().find(|l| l.id == item) else {
            return false;
        };
        let entry = (lib.title.clone(), lib.len);
        match target {
            DropTarget::Main { after } => self
                .edit(Edit::Insert {
                    item,
                    at: Place::After(after),
                })
                .is_ok(),
            DropTarget::Playlist(name) => {
                let tab = if self.main.name == name {
                    Some(&mut self.main)
                } else {
                    self.tabs.iter_mut().find(|t| t.name == name)
                };
                tab.map(|t| t.items.push(entry)).is_some()
            }
            DropTarget::Cart(i) => self.carts.get_mut(i).map(|c| *c = entry.0).is_some(),
        }
    }

    fn import_files(&mut self, paths: Vec<String>, category: &str) -> Vec<u64> {
        let mut ids = Vec::new();
        for path in paths {
            let name = folders::stem_of(&path).to_owned();
            let id = self.library.iter().map(|l| l.id).max().unwrap_or(0) + 1;
            self.library.push(LibraryItem {
                id,
                meta: meta::from_title(&name, PublishMode::FromTags),
                title: name,
                artist: None,
                len: Duration::ZERO,
                category: category.to_owned(),
                path,
            });
            ids.push(id);
        }
        ids
    }

    fn edit(&mut self, edit: Edit) -> Result<String, String> {
        let not_saved = |what: String| format!("preview — {what} (not saved)");
        match edit {
            Edit::Insert { item, at } => {
                let Some(lib) = self.library_item(item) else {
                    return Err("preview — that library item is gone".to_owned());
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
                let done = not_saved(format!("added {title} to {}", self.main.name));
                self.try_op(Op::Insert { at, id, title, len }, done)
            }
            Edit::Remove(ids) => {
                let n = ids.len();
                let mut ops = self.ops.clone();
                ops.extend(ids.into_iter().map(Op::Remove));
                let Some(top) = self.last_top.clone() else {
                    return Err("preview — no timeline yet".to_owned());
                };
                let mut rows = self.rows_from(&top).to_vec();
                if self.apply_ops(&mut rows, &ops) {
                    self.ops = ops;
                    Ok(not_saved(format!("removed {n} rows")))
                } else {
                    Err(
                        "preview — nothing left to fill the gap before the next fixed item"
                            .to_owned(),
                    )
                }
            }
            Edit::Move { id, to } => {
                let rows = self.current();
                let Some(i) = rows.iter().position(|r| r.id == id) else {
                    return Err("preview — that row is gone".to_owned());
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
                self.try_op(Op::Move { id, at }, not_saved("moved the row".to_owned()))
            }
            Edit::ToggleLock(id) => {
                self.try_op(Op::Lock(id), not_saved("changed the lock".to_owned()))
            }
            Edit::RenamePlaylist { from, to } => {
                let to = to.trim().to_owned();
                if to.is_empty() || self.unique_name(&to) != to {
                    return Err(format!("preview — the name {to:?} is empty or taken"));
                }
                let Some(tab) = self.tab_mut(&from) else {
                    return Err(format!("preview — no playlist {from}"));
                };
                tab.name.clone_from(&to);
                rules_script::refresh(tab);
                self.cache = None;
                Ok(not_saved(format!("renamed {from} to {to}")))
            }
            Edit::DuplicatePlaylist(name) => {
                let Some(tab) = self
                    .tabs
                    .iter()
                    .chain([&self.main])
                    .find(|t| t.name == name)
                else {
                    return Err(format!("preview — no playlist {name}"));
                };
                let mut copy = tab.clone();
                copy.name = self.unique_name(&format!("{name} copy"));
                rules_script::refresh(&mut copy);
                let msg = not_saved(format!("made {}", copy.name));
                self.tabs.push(copy);
                Ok(msg)
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
                    return Err(format!("preview — no playlist {name}"));
                }
                Ok(not_saved(format!("deleted {name}")))
            }
            Edit::RemovePlaylistItem { playlist, index } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(format!("preview — no playlist {playlist}"));
                };
                if index >= tab.items.len() {
                    return Err("preview — that item is gone".to_owned());
                }
                let (title, _) = tab.items.remove(index);
                Ok(not_saved(format!("removed {title} from {playlist}")))
            }
            Edit::MovePlaylistItem {
                playlist,
                index,
                up,
            } => {
                let Some(tab) = self.tab_mut(&playlist) else {
                    return Err(format!("preview — no playlist {playlist}"));
                };
                let other = if up {
                    index.checked_sub(1)
                } else {
                    Some(index + 1)
                };
                match other.filter(|o| *o < tab.items.len() && index < tab.items.len()) {
                    Some(o) => {
                        tab.items.swap(index, o);
                        Ok(not_saved("moved the item".to_owned()))
                    }
                    None => Err("preview — the item is already at the end".to_owned()),
                }
            }
            Edit::SetMeta(list) => {
                let n = list.len();
                for (target, m) in list {
                    match target {
                        MetaTarget::Row(id) => {
                            self.row_meta.insert(id, m);
                        }
                        MetaTarget::Library(id) => {
                            if let Some(l) = self.library.iter_mut().find(|l| l.id == id) {
                                l.meta = m;
                            }
                        }
                    }
                }
                Ok(not_saved(format!("changed the tags of {n} items")))
            }
            Edit::Deck { deck, command } => self.deck_command(deck, command),
            Edit::AssignCart { cart, item } => {
                let label = match item {
                    Some(id) => match self.library_item(id) {
                        Some(l) => l.title.clone(),
                        None => return Err("preview — that library item is gone".to_owned()),
                    },
                    None => String::new(),
                };
                let Some(slot) = self.carts.get_mut(cart) else {
                    return Err("preview — no such cart".to_owned());
                };
                let msg = if label.is_empty() {
                    format!("cleared cart {}", cart + 1)
                } else {
                    format!("assigned {label} to cart {}", cart + 1)
                };
                *slot = label;
                Ok(not_saved(msg))
            }
        }
    }

    fn library_ref(&mut self, title: &str, len: Duration) -> Option<u64> {
        let found = self.library.iter().find(|l| {
            l.title == title
                || l.artist
                    .as_ref()
                    .is_some_and(|a| format!("{a} – {}", l.title) == title)
        });
        if let Some(l) = found {
            return Some(l.id);
        }
        let id = self.library.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        self.library.push(LibraryItem {
            id,
            title: title.to_owned(),
            artist: None,
            len,
            category: "Unsorted".to_owned(),
            path: "—".to_owned(),
            meta: meta::from_title(title, PublishMode::FromTags),
        });
        Some(id)
    }

    fn playlist_from_folder(&mut self, folder: FolderSource, files: Vec<String>) -> Option<String> {
        let name = self.unique_name(folders::name_of(&folder.path));
        let colour = FOLDER_COLOURS[self.tabs.len() % FOLDER_COLOURS.len()];
        let items = files
            .iter()
            .map(|f| (folders::stem_of(f).to_owned(), Duration::ZERO))
            .collect();
        let mut tab = playlist(&name, colour, Pick::Shuffle, Vec::new(), items);
        tab.source = Some(folder);
        rules_script::refresh(&mut tab);
        self.tabs.push(tab);
        Some(name)
    }

    fn use_as_main(&mut self, name: &str) -> bool {
        let Some(i) = self.tabs.iter().position(|t| t.name == name) else {
            return false;
        };
        let sample = self.tabs[i].name == SAMPLE;
        if self.tabs[i].items.is_empty() && !sample {
            return false;
        }
        let tab = self.tabs.remove(i);
        let mut old = std::mem::replace(&mut self.main, tab);
        if old.source.is_none() && old.items.is_empty() && old.name == "Music" {
            old.name = SAMPLE.to_owned();
        }
        self.tabs.push(old);
        self.main_tracks = if sample {
            None
        } else {
            Some(
                self.main
                    .items
                    .iter()
                    .map(|(t, l)| (t.clone(), if l.is_zero() { estimated_len(t) } else { *l }))
                    .collect(),
            )
        };
        self.cache = None;
        self.ops.clear();
        true
    }

    fn apply_rules(&mut self, tab: PlaylistTab) -> bool {
        if self.main.name == tab.name {
            self.main = tab;
            return true;
        }
        match self.tabs.iter_mut().find(|t| t.name == tab.name) {
            Some(slot) => {
                *slot = tab;
                true
            }
            None => false,
        }
    }
}
