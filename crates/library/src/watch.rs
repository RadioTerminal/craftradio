use crate::{LibraryError, audio_path, media};
use notify::{EventKind, RecursiveMode};
use radio_model::FolderSource;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

/// A settled change to an audio path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    Added(PathBuf),
    Removed(PathBuf),
    Changed(PathBuf),
}
/// Owns the operating system watch. Poll `events` to drain settled changes.
pub struct Watcher {
    _watch: Box<dyn notify::Watcher>,
    rx: Receiver<notify::Result<notify::Event>>,
    original_root: PathBuf,
    canonical_root: PathBuf,
    seen: BTreeSet<PathBuf>,
    pending: BTreeMap<PathBuf, (Instant, WatchEvent)>,
}
impl Watcher {
    /// Starts a native watch, with polling if the native watch cannot start.
    /// `watch` is a persisted UI preference; this explicit call starts it.
    pub fn new(src: &FolderSource) -> Result<Self, LibraryError> {
        Self::start::<notify::RecommendedWatcher>(src, notify::Config::default()).or_else(|_| {
            Self::start::<notify::PollWatcher>(
                src,
                notify::Config::default().with_poll_interval(Duration::from_millis(100)),
            )
        })
    }
    fn start<W: notify::Watcher + 'static>(
        src: &FolderSource,
        config: notify::Config,
    ) -> Result<Self, LibraryError> {
        let original_root = PathBuf::from(&src.path);
        let canonical_root = original_root.canonicalize()?;
        let seen = crate::paths(src)?.into_iter().collect();
        let (tx, rx) = mpsc::channel();
        let mut watch = W::new(
            move |e| {
                let _ = tx.send(e);
            },
            config,
        )
        .map_err(media)?;
        watch
            .watch(
                &canonical_root,
                if src.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )
            .map_err(media)?;
        Ok(Self {
            _watch: Box::new(watch),
            rx,
            original_root,
            canonical_root,
            seen,
            pending: BTreeMap::new(),
        })
    }

    /// Returns events after 500 ms without another event for the path.
    /// Watch errors are returned rather than treated as successful changes.
    pub fn events(&mut self) -> Result<std::vec::IntoIter<WatchEvent>, LibraryError> {
        let mut first_error = None;
        for event in self.rx.try_iter() {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    first_error.get_or_insert_with(|| media(error));
                    continue;
                }
            };
            for path in event.paths {
                let Some(path) = map_path(&path, &self.canonical_root, &self.original_root) else {
                    continue;
                };
                if !audio_path(&path) {
                    continue;
                }
                let change = match event.kind {
                    // macOS reports later writes to a new file with its "created" flag still
                    // set. A path that the watch knows already is a change, not a new file.
                    EventKind::Create(_) if self.seen.contains(&path) => {
                        WatchEvent::Changed(path.clone())
                    }
                    EventKind::Create(_) => WatchEvent::Added(path.clone()),
                    EventKind::Remove(_) => WatchEvent::Removed(path.clone()),
                    EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
                        if path.exists() {
                            WatchEvent::Added(path.clone())
                        } else {
                            WatchEvent::Removed(path.clone())
                        }
                    }
                    EventKind::Modify(_) if !path.exists() => continue,
                    EventKind::Modify(_) if !self.seen.contains(&path) => {
                        WatchEvent::Added(path.clone())
                    }
                    EventKind::Modify(_) => WatchEvent::Changed(path.clone()),
                    _ => continue,
                };
                if matches!(change, WatchEvent::Removed(_)) {
                    self.seen.remove(&path);
                } else {
                    self.seen.insert(path.clone());
                }
                let change = match self.pending.get(&path) {
                    Some((_, WatchEvent::Added(_))) if matches!(change, WatchEvent::Changed(_)) => {
                        WatchEvent::Added(path.clone())
                    }
                    _ => change,
                };
                self.pending.insert(path, (Instant::now(), change));
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        let mut ready = Vec::new();
        self.pending.retain(|_, (time, event)| {
            if time.elapsed() >= Duration::from_millis(500) {
                ready.push(event.clone());
                false
            } else {
                true
            }
        });
        Ok(ready.into_iter())
    }
}

fn map_path(path: &Path, canonical: &Path, original: &Path) -> Option<PathBuf> {
    path.strip_prefix(canonical)
        .ok()
        .map(|rest| original.join(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_canonical_paths_including_deleted_files() {
        let original = Path::new("/var/station");
        let canonical = Path::new("/private/var/station");
        assert_eq!(
            map_path(&canonical.join("sub/deleted.wav"), canonical, original),
            Some(original.join("sub/deleted.wav"))
        );
        assert_eq!(
            map_path(Path::new("/private/var/other/a.wav"), canonical, original),
            None
        );
    }

    #[test]
    fn drains_errors_and_classifies_coalesced_changes() {
        let dir = tempfile::tempdir().unwrap();
        let src = FolderSource {
            path: dir.path().to_str().unwrap().into(),
            recursive: false,
            watch: true,
        };
        let mut watcher =
            Watcher::start::<notify::PollWatcher>(&src, notify::Config::default()).unwrap();
        let (tx, rx) = mpsc::channel();
        watcher.rx = rx;
        let path = dir.path().join("new.wav");
        std::fs::write(&path, b"test").unwrap();
        tx.send(Err(notify::Error::generic("first"))).unwrap();
        tx.send(Err(notify::Error::generic("second"))).unwrap();
        for path in [&path, &dir.path().join("gone.wav")] {
            tx.send(Ok(notify::Event::new(EventKind::Modify(
                notify::event::ModifyKind::Any,
            ))
            .add_path(
                path.canonicalize()
                    .unwrap_or_else(|_| watcher.canonical_root.join("gone.wav")),
            )))
            .unwrap();
        }
        assert!(watcher.events().unwrap_err().to_string().contains("first"));
        assert!(watcher.rx.try_recv().is_err());
        assert_eq!(watcher.pending.len(), 1);
        let (time, event) = watcher.pending.get_mut(&path).unwrap();
        assert_eq!(*event, WatchEvent::Added(path.clone()));
        *time = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            watcher.events().unwrap().collect::<Vec<_>>(),
            vec![WatchEvent::Added(path)]
        );
    }
}
