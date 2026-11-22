//! Folder scanning, audio analysis, file tags and content-based relinking.
//!
//! Files decode with the station's codecs (`radio-codecs`): Symphonia's, and Opus through
//! craft-audio's native decoder. AAC support is AAC-LC; unsupported profiles return errors.
//! Keep scan results in station data. Playout keeps the loudness of each analysed file
//! in station data, by content hash, so each file gets one measurement.
mod listing;
mod watch;
pub use listing::{Limits, Listing, list_audio, probe};
use lofty::{
    file::TaggedFileExt,
    tag::{Accessor, ItemKey, Tag, TagExt},
};
use radio_model::{FolderSource, Item, ItemId, ItemMeta, Loudness, Source, Timing};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use symphonia::core::{
    codecs::audio::AudioDecoderOptions,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};
pub use watch::{WatchEvent, Watcher};

/// A file, tag, decoder or watcher operation failed.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Media(String),
    #[error("folder contains more than 50 000 audio files")]
    FileLimit,
}
fn media(e: impl std::fmt::Display) -> LibraryError {
    LibraryError::Media(e.to_string())
}

/// Results of a complete audio decode. Silence has zero gain and zero cue points.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub len: Duration,
    pub timing: Timing,
    pub gain_db: f32,
    /// The EBU R128 integrated loudness and the true peak. `None` for a silent file.
    pub loudness: Option<Loudness>,
    pub meta: ItemMeta,
    pub hash: [u8; 32],
    /// The file size in bytes, as [`content_hash`] gives it.
    pub size: u64,
}
fn audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| radio_model::AUDIO_EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
}
fn paths(src: &FolderSource) -> Result<Vec<PathBuf>, LibraryError> {
    collect_paths(src, &mut Vec::new())
}
fn collect_paths(
    src: &FolderSource,
    errors: &mut Vec<(String, LibraryError)>,
) -> Result<Vec<PathBuf>, LibraryError> {
    let root = PathBuf::from(&src.path);
    let mut dirs = vec![root.clone()];
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if dir == root => return Err(error.into()),
            Err(error) => {
                errors.push((dir.to_string_lossy().into_owned(), error.into()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push((dir.to_string_lossy().into_owned(), error.into()));
                    continue;
                }
            };
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    errors.push((entry.path().to_string_lossy().into_owned(), error.into()));
                    continue;
                }
            };
            if kind.is_dir() && src.recursive {
                dirs.push(entry.path());
            }
            if kind.is_file() && audio_path(&entry.path()) {
                files.push(entry.path());
                if files.len() > 50_000 {
                    return Err(LibraryError::FileLimit);
                }
            }
        }
    }
    files.sort();
    Ok(files)
}
/// Files imported by a scan and per-file failures, both in path order.
#[derive(Debug)]
pub struct ScanResult {
    pub items: Vec<Item>,
    pub errors: Vec<(String, LibraryError)>,
}
/// Scans in path order. Progress is the number of fully analysed files.
/// IDs are local to this scan; the station must assign its own IDs on import.
pub fn scan(
    src: &FolderSource,
    progress: &mut dyn FnMut(usize),
) -> Result<ScanResult, LibraryError> {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    for path in collect_paths(src, &mut errors)? {
        let a = match analyse(&path) {
            Ok(a) => a,
            Err(error) => {
                errors.push((path.to_string_lossy().into_owned(), error));
                continue;
            }
        };
        let Some(path) = path.to_str().map(str::to_owned) else {
            errors.push((
                path.to_string_lossy().into_owned(),
                media("path is not UTF-8"),
            ));
            continue;
        };
        items.push(Item {
            id: ItemId(items.len() as u64 + 1),
            title: a.meta.title.clone(),
            artist: (!a.meta.artist.is_empty()).then(|| a.meta.artist.clone()),
            meta: a.meta,
            source: Source::File { path },
            timing: Some(a.timing),
            planned_len: Some(a.len),
            movable: true,
            trimmable: true,
            locked: false,
            valid_from: None,
            valid_to: None,
            max_per_day: None,
        });
        progress(items.len());
    }
    errors.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(ScanResult { items, errors })
}
/// The content hash and the size of a file, as [`relink`] matches them.
pub fn content_hash(path: impl AsRef<Path>) -> Result<([u8; 32], u64), LibraryError> {
    let path = path.as_ref();
    Ok((hash(path)?, fs::metadata(path)?.len()))
}

fn hash(path: &Path) -> Result<[u8; 32], LibraryError> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    let mut reader = file.take(1024 * 1024);
    let mut buffer = [0; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    hasher.update(&size.to_le_bytes());
    Ok(*hasher.finalize().as_bytes())
}
fn metadata(path: &Path) -> Result<ItemMeta, LibraryError> {
    let file = lofty::read_from_path(path).map_err(media)?;
    let mut m = ItemMeta::default();
    if let Some(t) = file.primary_tag().or_else(|| file.first_tag()) {
        m.title = t.title().unwrap_or_default().into_owned();
        m.artist = t.artist().unwrap_or_default().into_owned();
        m.album = t.album().unwrap_or_default().into_owned();
        m.year = t
            .get_string(&ItemKey::RecordingDate)
            .unwrap_or_default()
            .to_owned();
        m.genre = t.genre().unwrap_or_default().into_owned();
        m.comment = t.comment().unwrap_or_default().into_owned();
    }
    Ok(m)
}
/// Decodes audio in packets, with bounded sample memory. Unsupported codecs return an error.
/// Tail levels use 100 ms RMS windows. Hash input ends with the little-endian u64 file size.
pub fn analyse(path: impl AsRef<Path>) -> Result<Analysis, LibraryError> {
    let path = path.as_ref();
    let meta = metadata(path)?;
    let hash = hash(path)?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|x| x.to_str()) {
        hint.with_extension(ext);
    }
    let source = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(media)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| media("no audio track"))?;
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| media("no audio track"))?;
    let registry = radio_codecs::registry();
    let mut decoder = registry
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(media)?;
    let mut meter = None;
    let mut samples: Vec<f32> = Vec::new();
    let mut rate = 0;
    let mut channels = 0;
    let mut frames = 0u64;
    let mut first = None;
    let mut last = 0;
    let mut windows = Vec::new();
    let mut energy = 0.0;
    let mut count = 0u32;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => return Err(media(e)),
        };
        if packet.track_id != id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(media)?;
        let spec = decoded.spec().clone();
        if meter.is_none() {
            rate = spec.rate();
            channels = spec.channels().count();
            if channels == 0 {
                return Err(media("audio has no channels"));
            }
            meter = Some(
                ebur128::EbuR128::new(
                    channels as u32,
                    rate,
                    ebur128::Mode::I | ebur128::Mode::TRUE_PEAK,
                )
                .map_err(media)?,
            );
        }
        if rate != spec.rate() || channels != spec.channels().count() {
            return Err(media("audio format changed"));
        }
        decoded.copy_to_vec_interleaved(&mut samples);
        if let Some(m) = meter.as_mut() {
            m.add_frames_f32(&samples).map_err(media)?;
        }
        for frame in samples.chunks_exact(channels) {
            let peak = frame.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            if peak > 10.0f32.powf(-45.0 / 20.0) {
                first.get_or_insert(frames);
                last = frames + 1;
            }
            energy += frame.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / channels as f64;
            count += 1;
            frames += 1;
            if count >= (rate / 10).max(1) {
                windows.push((frames, energy / f64::from(count)));
                energy = 0.0;
                count = 0;
            }
        }
    }
    if frames == 0 || rate == 0 {
        return Err(media("empty audio"));
    }
    if count > 0 {
        windows.push((frames, energy / f64::from(count)));
    }
    let meter = meter.as_ref().ok_or_else(|| media("no loudness meter"))?;
    let loudness = meter.loudness_global().map_err(media)?;
    let mut peak = 0.0f64;
    for c in 0..channels as u32 {
        peak = peak.max(meter.true_peak(c).map_err(media)?);
    }
    let measured = (loudness.is_finite() && peak > 0.0).then(|| Loudness {
        lufs: loudness as f32,
        peak_db: Some((20.0 * peak.log10()) as f32),
    });
    let gain_db = if loudness.is_finite() {
        (-18.0 - loudness) as f32
    } else {
        0.0
    };
    let time = |n| Duration::from_secs_f64(n as f64 / f64::from(rate));
    let cue_in = time(first.unwrap_or(0));
    let cue_out = time(last);
    let threshold = 10.0f64.powf((loudness - 20.0) / 10.0);
    let tail = windows
        .iter()
        .rev()
        .find(|(_, e)| *e >= threshold && *e > 0.0)
        .map(|(n, _)| time(*n));
    let segue = tail
        .filter(|t| *t < cue_out)
        .unwrap_or_else(|| cue_out.saturating_sub(Duration::from_secs(2)))
        .max(cue_in)
        .min(cue_out);
    Ok(Analysis {
        len: time(frames),
        timing: Timing {
            cue_in,
            segue,
            cue_out,
            intro_end: None,
            fade_in: Duration::ZERO,
            fade_out: Duration::ZERO,
        },
        gain_db,
        loudness: measured,
        meta,
        hash,
        size: fs::metadata(path)?.len(),
    })
}
/// The ReplayGain reference level, in LUFS (ReplayGain 2.0).
const REPLAYGAIN_REF_LUFS: f32 = -18.0;
/// The reference level of the Opus `R128_TRACK_GAIN` tag, in LUFS (RFC 7845).
const R128_REF_LUFS: f32 = -23.0;

/// Reads the loudness from the tags of a file: `REPLAYGAIN_TRACK_GAIN` and
/// `REPLAYGAIN_TRACK_PEAK`, or `R128_TRACK_GAIN` (Opus, Q7.8 dB). It returns `None` when
/// the file has no such tag, or the tag does not parse.
pub fn read_loudness_tags(path: impl AsRef<Path>) -> Option<Loudness> {
    let file = lofty::read_from_path(path.as_ref()).ok()?;
    let number = |text: &str| -> Option<f32> {
        let text = text.trim();
        let text = text
            .strip_suffix("dB")
            .or_else(|| text.strip_suffix("db"))
            .unwrap_or(text);
        text.trim().parse::<f32>().ok().filter(|v| v.is_finite())
    };
    for tag in file.tags() {
        if let Some(gain) = tag
            .get_string(&ItemKey::ReplayGainTrackGain)
            .and_then(number)
        {
            let peak_db = tag
                .get_string(&ItemKey::ReplayGainTrackPeak)
                .and_then(number)
                .filter(|p| *p > 0.0)
                .map(|p| 20.0 * p.log10());
            return Some(Loudness {
                lufs: REPLAYGAIN_REF_LUFS - gain,
                peak_db,
            });
        }
        // Tag formats keep an unknown key as written, so the match ignores case.
        let r128 = tag
            .items()
            .find(|i| {
                matches!(i.key(), ItemKey::Unknown(k) if k.eq_ignore_ascii_case("R128_TRACK_GAIN"))
            })
            .and_then(|i| i.value().text())
            .and_then(|v| v.trim().parse::<i32>().ok());
        if let Some(q78) = r128 {
            return Some(Loudness {
                lufs: R128_REF_LUFS - q78 as f32 / 256.0,
                peak_db: None,
            });
        }
    }
    None
}
/// Writes file tags. Publish settings stay in station data.
pub fn write_tags(path: impl AsRef<Path>, meta: &ItemMeta) -> Result<(), LibraryError> {
    let path = path.as_ref();
    let mut file = lofty::read_from_path(path).map_err(media)?;
    let kind = file.primary_tag_type();
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file
        .primary_tag_mut()
        .ok_or_else(|| media("no writable tag"))?;
    tag.set_title(meta.title.clone());
    tag.set_artist(meta.artist.clone());
    tag.set_album(meta.album.clone());
    tag.insert_text(ItemKey::RecordingDate, meta.year.clone());
    tag.set_genre(meta.genre.clone());
    tag.set_comment(meta.comment.clone());
    tag.save_to_path(path, lofty::config::WriteOptions::default())
        .map_err(media)
}
/// Finds the first sorted matching path per ID. Unreadable roots and files are skipped.
/// Missing entries contain the ID, hash and file size in bytes.
/// Hashing does not require a supported audio decoder.
pub fn relink(
    missing: &[(ItemId, [u8; 32], u64)],
    roots: &[FolderSource],
) -> Vec<(ItemId, String)> {
    let mut candidates: Vec<_> = roots
        .iter()
        .filter_map(|r| paths(r).ok())
        .flatten()
        .collect();
    candidates.sort();
    candidates.dedup();
    let mut found = std::collections::BTreeMap::new();
    let sizes: std::collections::BTreeSet<_> = missing.iter().map(|(_, _, size)| *size).collect();
    for path in candidates {
        if !fs::metadata(&path).is_ok_and(|m| sizes.contains(&m.len())) {
            continue;
        }
        if let Ok(h) = hash(&path)
            && let Some(path) = path.to_str()
        {
            found.entry(h).or_insert_with(|| path.to_owned());
        }
    }
    missing
        .iter()
        .filter_map(|(id, h, _)| found.get(h).map(|p| (*id, p.clone())))
        .collect()
}

/// The largest cover that [`cover`] returns.
const MAX_COVER: usize = 4 * 1024 * 1024;

/// The front cover in the tags of an audio file, or its first picture. `None` when the
/// file has no picture, or the picture is not JPEG, PNG, WebP or GIF.
pub fn cover(path: impl AsRef<Path>) -> Option<Cover> {
    use lofty::picture::PictureType;
    let file = lofty::read_from_path(path.as_ref()).ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pictures = tag.pictures();
    let picture = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())?;
    let bytes = picture.data();
    if bytes.is_empty() || bytes.len() > MAX_COVER {
        return None;
    }
    let mime = picture.mime_type().map_or("image/jpeg", |m| m.as_str());
    if !matches!(
        mime,
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    ) {
        return None;
    }
    Some(Cover {
        mime: mime.to_owned(),
        bytes: bytes.to_vec(),
    })
}

/// A picture from the tags of an audio file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cover {
    pub mime: String,
    pub bytes: Vec<u8>,
}
