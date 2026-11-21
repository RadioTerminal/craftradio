//! Opens the station database, and fills it on the first run.

use std::path::{Path, PathBuf};

use radio_engine::{Command, Engine};
use radio_model::{FolderSource, Source, Station};

/// The example station. A first run without `--station` starts from it.
const EXAMPLE: &str = include_str!("../../../examples/station.toml");

/// What the first run did.
pub enum Seeded {
    /// The database existed already.
    No,
    /// The example station, with its file paths under this Music folder.
    Example { music: PathBuf, kept: usize },
    /// The station file from `--station`.
    File { path: PathBuf, items: usize },
}

/// Opens `<dir>/station.db`. On the first run it imports `station`, or else the example.
pub fn open(dir: &Path, station: Option<&Path>) -> Result<(Engine, Seeded), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let db = dir.join("station.db");
    let mut engine = Engine::open(&db).map_err(|e| format!("cannot open {}: {e}", db.display()))?;
    // A first run is a station without the seeded mark and without data of its own. The mark
    // comes only after a seed that worked, so a failed import tries again at the next start.
    let marked = engine.runtime(SEEDED).map_err(|e| e.to_string())?.is_some();
    let st = &engine.state().station;
    let empty = st.playlists.len() == 1 && st.playlists.iter().all(|p| p.items.is_empty());
    if marked || !empty {
        if !marked {
            mark_seeded(&engine)?;
        }
        return Ok((engine, Seeded::No));
    }
    let seeded = match station {
        Some(path) => {
            let station = read_station(path)?;
            let items = station.playlists.iter().map(|p| p.items.len()).sum();
            import(&mut engine, station)?;
            Seeded::File {
                path: path.to_owned(),
                items,
            }
        }
        None => seed_example(&mut engine, dir)?,
    };
    engine.flush().map_err(|e| e.to_string())?;
    mark_seeded(&engine)?;
    Ok((engine, seeded))
}

/// The runtime key that marks a station as seeded.
const SEEDED: &str = "seeded";

fn mark_seeded(engine: &Engine) -> Result<(), String> {
    engine
        .save_runtime(SEEDED, &serde_json::json!(true))
        .map_err(|e| format!("cannot mark the station as seeded: {e}"))
}

fn import(engine: &mut Engine, station: Station) -> Result<(), String> {
    engine
        .dispatch(Command::StationImport { station })
        .map(|_| ())
        .map_err(|e| format!("cannot import the station: {e}"))
}

/// Reads a station file. Relative file paths count from the folder of the file.
fn read_station(path: &Path) -> Result<Station, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut station: Station =
        toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    for item in station
        .playlists
        .iter_mut()
        .flat_map(|p| p.items.iter_mut())
    {
        if let Source::File { path } = &mut item.source
            && Path::new(path.as_str()).is_relative()
        {
            *path = base.join(&*path).to_string_lossy().into_owned();
        }
    }
    station.validate().map_err(|e| e.to_string())?;
    Ok(station)
}

/// The folder under Music for a first path part of the example, such as `jingles`.
fn folder_for(music: &Path, part: &str) -> PathBuf {
    if part == "music" {
        return music.to_owned();
    }
    let mut name = part.to_owned();
    if let Some(first) = name.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    music.join(name)
}

/// Imports the example station with its files under `<dir>/Music`. Items whose file is
/// not there, and streams of the example host, leave the station. Each playlist follows
/// its folder, so files that a person adds there join it.
fn seed_example(engine: &mut Engine, dir: &Path) -> Result<Seeded, String> {
    let mut station: Station =
        toml::from_str(EXAMPLE).map_err(|e| format!("the example station is broken: {e}"))?;
    let music = dir.join("Music");
    let mut sources = Vec::new();
    let mut kept = 0;
    for p in &mut station.playlists {
        let mut folder = None;
        for item in &mut p.items {
            if let Source::File { path } = &mut item.source {
                let rel = PathBuf::from(&*path);
                let mut parts = rel.components();
                let first = parts
                    .next()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .unwrap_or_default();
                let home = folder_for(&music, &first);
                *path = home.join(parts.as_path()).to_string_lossy().into_owned();
                folder.get_or_insert(home);
            }
        }
        p.items.retain(|i| match &i.source {
            Source::File { path } => Path::new(path).is_file(),
            Source::Stream { url, .. } => !url.contains("example.invalid"),
            Source::LiveInput { .. } => true,
        });
        kept += p.items.len();
        if let Some(home) = folder {
            std::fs::create_dir_all(&home)
                .map_err(|e| format!("cannot make {}: {e}", home.display()))?;
            sources.push((p.id, home));
        }
    }
    import(engine, station)?;
    for (playlist, home) in sources {
        let source = FolderSource {
            path: home.to_string_lossy().into_owned(),
            recursive: false,
            watch: true,
        };
        let source = serde_json::to_value(&source).map_err(|e| e.to_string())?;
        engine
            .dispatch(Command::PlaylistSetSource { playlist, source })
            .map_err(|e| e.to_string())?;
    }
    engine
        .dispatch(Command::LibraryAddFolder {
            path: music.to_string_lossy().into_owned(),
        })
        .map_err(|e| e.to_string())?;
    Ok(Seeded::Example { music, kept })
}

/// Brings each watched folder playlist up to date with its folder: new audio files join it,
/// and items whose file is gone leave it. It returns one line per changed playlist.
pub fn reconcile(engine: &mut Engine) -> Vec<String> {
    let mut lines = Vec::new();
    let playlists: Vec<(radio_model::PlaylistId, String, FolderSource)> = engine
        .state()
        .station
        .playlists
        .iter()
        .filter_map(|p| {
            let src = p.source.clone().or_else(|| {
                engine
                    .state()
                    .source(p.id)
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
            })?;
            src.watch.then(|| (p.id, p.name.clone(), src))
        })
        .collect();
    for (playlist, name, src) in playlists {
        let list = radio_library::list_audio(
            Path::new(&src.path),
            src.recursive,
            radio_library::Limits::DEFAULT,
        );
        let station = &engine.state().station;
        let Some(p) = station.playlists.iter().find(|p| p.id == playlist) else {
            continue;
        };
        let has = |f: &str| {
            p.items
                .iter()
                .any(|i| matches!(&i.source, Source::File { path } if path == f))
        };
        let gone: Vec<radio_model::ItemId> = p
            .items
            .iter()
            .filter(|i| matches!(&i.source, Source::File { path } if !Path::new(path).is_file()))
            .map(|i| i.id)
            .collect();
        let mut next = station
            .playlists
            .iter()
            .flat_map(|p| p.items.iter().map(|i| i.id.0))
            .max()
            .unwrap_or(0)
            + 1;
        let mut items = Vec::new();
        for f in list.files.iter().filter(|f| !has(f)) {
            if let Some(mut item) = radio_ui_engine::file_item(f) {
                item.id = radio_model::ItemId(next);
                next += 1;
                items.push(item);
            }
        }
        let (added, removed) = (items.len(), gone.len());
        if added > 0 {
            let _ = engine.dispatch(Command::PlaylistAddItems {
                playlist,
                items,
                at: None,
            });
        }
        if removed > 0 {
            let _ = engine.dispatch(Command::PlaylistRemoveItems {
                playlist,
                items: gone,
            });
        }
        if added + removed > 0 {
            lines.push(format!("{name}: {added} new files, {removed} gone"));
        }
    }
    lines
}
