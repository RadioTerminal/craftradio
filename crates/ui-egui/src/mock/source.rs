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
        self.ensure_item_ids();
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
            r.timing = self.row_timing.get(&r.id).copied();
            r.stream = r.meta.keep_alive.map(|keep| stream::simulate(keep, now));
        }
        self.last_all.clone_from(&all);
        let since = now
            .checked_sub(SignedDuration::from_hours(PAST_HOURS))
            .unwrap_or(now);
        let history = all
            .iter()
            .filter(|r| r.start.timestamp() >= since && r.start.timestamp() <= now)
            .cloned()
            .collect();
        let mut rows: Vec<Row> = all.into_iter().filter(|r| r.start >= top).collect();

        let on_air = rows
            .iter()
            .position(|r| r.start.timestamp() <= now && now < end_of(r));
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
        // The deck of each row: A holds the row on air, B to D the next three.
        if let Some(i) = on_air {
            for (k, r) in rows.iter_mut().skip(i).take(4).enumerate() {
                r.deck = Some(k);
            }
        }
        let meters = meters(now, &decks, &self.strips);
        self.last_decks = Some((decks.clone(), std::time::Instant::now()));

        StationView {
            now: local,
            mode: self.play_mode.label().to_uppercase(),
            play_mode: self.play_mode,
            stop_after_current: self.stop_after_current,
            decks,
            rows,
            tabs: self.tabs.clone(),
            main: self.main.clone(),
            main_playlist: self.main.name.clone(),
            station: STATION.to_owned(),
            meters,
            history,
            library: self.library.clone(),
            input_devices: Vec::new(),
            devices: DEVICES.iter().map(|d| (*d).to_owned()).collect(),
            device_roles: self.device_roles.clone(),
            carts: self.carts.clone(),
            cartwall: self.cartwall.clone(),
            icecast: self.icecast.clone(),
            output: OutputStatus {
                local: Status::Preview,
                icecast: Status::NotConnected,
                listeners: None,
            },
            folders: self.folder_views(),
            settings: self.settings.clone(),
            warnings: 0,
            preview: true,
        }
    }

    fn drop_item(&mut self, item: u64, target: DropTarget) -> Result<usize, String> {
        let Some(lib) = self.library_item(item) else {
            return Err("preview — that library item is gone".to_owned());
        };
        let entry = (lib.title.clone(), lib.len);
        match target {
            DropTarget::Main { after } => self.edit(Edit::Insert {
                item,
                at: Place::After(after),
            }),
            DropTarget::Playlist(name) => {
                let id = self.next_item_id;
                let Some(tab) = self.tab_mut(&name) else {
                    return Err(format!("preview — no playlist {name}"));
                };
                tab.ids.truncate(tab.items.len());
                tab.items.push(entry);
                tab.weights.push(1);
                tab.ids.push(id);
                self.next_item_id += 1;
                Ok(1)
            }
            DropTarget::Cart(cart) => self.edit(Edit::AssignCart {
                cart,
                item: Some(item),
            }),
        }
    }

    fn import_files(&mut self, paths: Vec<String>, category: &str) -> Result<Imported, String> {
        let mut audio = Vec::new();
        let mut skipped = 0;
        for p in &paths {
            let path = std::path::Path::new(p);
            if path.is_dir() {
                let scan = folders::scan(path, false, self.limits);
                skipped += scan.errors.len();
                audio.extend(scan.files);
            } else if folders::is_audio(path) {
                audio.push(p.clone());
            } else {
                skipped += 1;
            }
        }
        audio.truncate(self.limits.max_files);
        let ids = self.add_library_files(&audio, category);
        Ok(Imported { ids, skipped })
    }

    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        self.apply_edit(edit)
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
        let id = self.next_library_id();
        self.library.push(LibraryItem {
            id,
            title: title.to_owned(),
            artist: None,
            len,
            category: "Unsorted".to_owned(),
            path: "—".to_owned(),
            meta: from_title(title, PublishMode::FromTags),
        });
        Some(id)
    }

    fn playlist_from_folder(&mut self, folder: FolderSource) -> Result<String, String> {
        let scan = folders::scan(
            std::path::Path::new(&folder.path),
            folder.recursive,
            self.limits,
        );
        if scan.files.is_empty() {
            let why = scan.errors.first().cloned().unwrap_or_default();
            return Err(format!("preview — no audio files in {} {why}", folder.path));
        }
        let name = self.unique_name(folders::name_of(&folder.path));
        let colour = FOLDER_COLOURS[self.tabs.len() % FOLDER_COLOURS.len()];
        let items = scan
            .files
            .iter()
            .map(|f| (folders::stem_of(f).to_owned(), Duration::ZERO))
            .collect();
        let mut tab = playlist(&name, colour, Pick::Shuffle, Vec::new(), items);
        tab.source = Some(folder);
        rules_script::refresh(&mut tab);
        self.tabs.push(tab);
        Ok(name)
    }

    fn use_as_main(&mut self, name: &str) -> Result<(), String> {
        let Some(i) = self.tabs.iter().position(|t| t.name == name) else {
            return Err(format!("preview — no playlist {name}"));
        };
        let sample = self.tabs[i].name == SAMPLE;
        if self.tabs[i].items.is_empty() && !sample {
            return Err(format!("preview — {name} has no items to play"));
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
        Ok(())
    }

    fn apply_rules(&mut self, mut tab: PlaylistTab) -> Result<(), String> {
        // A folder playlist reads its folder again, with the subfolder setting of the copy.
        if let Some(src) = &tab.source {
            let scan = folders::scan(std::path::Path::new(&src.path), src.recursive, self.limits);
            if let Some(e) = scan.errors.first() {
                return Err(format!("preview — {e}"));
            }
            tab.items = scan
                .files
                .iter()
                .map(|f| (folders::stem_of(f).to_owned(), Duration::ZERO))
                .collect();
            tab.weights = vec![1; tab.items.len()];
            tab.ids.clear();
        }
        let Some(slot) = self.tab_mut(&tab.name) else {
            return Err(format!("preview — no playlist {}", tab.name));
        };
        *slot = tab;
        Ok(())
    }

    fn meters(&self) -> Option<crate::view::MeterFrame> {
        // The simulated levels move with the time since the last snapshot.
        let (decks, at) = self.last_decks.as_ref()?;
        let run = SignedDuration::try_from(at.elapsed()).unwrap_or_default();
        let now = self.last_now.checked_add(run).unwrap_or(self.last_now);
        let m = meters(now, decks, &self.strips);
        Some(crate::view::MeterFrame {
            level_db: m.iter().map(|m| m.level_db).collect(),
            peak_db: m.iter().map(|m| m.peak_db).collect(),
        })
    }

    fn stream_title(&self, row: &Row) -> String {
        let mut previous = STATION.to_owned();
        for r in &self.last_all {
            if r.id == row.id {
                return stream::title_for(&row.meta, &previous, STATION);
            }
            previous = stream::title_for(&r.meta, &previous, STATION);
        }
        // The row is not in the timeline, so the item before it is not known.
        stream::title_for(&row.meta, UNKNOWN_PREVIOUS, STATION)
    }
}
