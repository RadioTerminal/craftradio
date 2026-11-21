use crate::{LibraryError, audio_path, media};
use notify::{EventKind, RecursiveMode};
use radio_model::FolderSource;
use std::{
    collections::BTreeMap,
    path::PathBuf,
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
                std::path::Path::new(&src.path),
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
            pending: BTreeMap::new(),
        })
    }

    /// Returns events after 500 ms without another event for the path.
    /// Watch errors are returned rather than treated as successful changes.
    pub fn events(&mut self) -> Result<std::vec::IntoIter<WatchEvent>, LibraryError> {
        for event in self.rx.try_iter() {
            let event = event.map_err(media)?;
            for path in event.paths {
                if !audio_path(&path) {
                    continue;
                }
                let change = match event.kind {
                    EventKind::Create(_) => WatchEvent::Added(path.clone()),
                    EventKind::Remove(_) => WatchEvent::Removed(path.clone()),
                    EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
                        if path.exists() {
                            WatchEvent::Added(path.clone())
                        } else {
                            WatchEvent::Removed(path.clone())
                        }
                    }
                    EventKind::Modify(_) => WatchEvent::Changed(path.clone()),
                    _ => continue,
                };
                let change = match self.pending.get(&path) {
                    Some((_, WatchEvent::Added(_))) if matches!(change, WatchEvent::Changed(_)) => {
                        WatchEvent::Added(path.clone())
                    }
                    _ => change,
                };
                self.pending.insert(path, (Instant::now(), change));
            }
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
