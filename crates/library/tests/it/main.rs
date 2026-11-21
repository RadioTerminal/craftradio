use radio_library::{WatchEvent, Watcher, analyse, relink, scan, write_tags};
use radio_model::{FolderSource, ItemId, ItemMeta};
use std::{
    path::Path,
    time::{Duration, Instant},
};
fn tone(path: &Path, amplitude: f32, lead: u32) {
    let mut wav = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for n in 0..(lead + 3) * 48000 {
        let value = if n < lead * 48000 {
            0.0
        } else {
            amplitude * (n as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin()
        };
        wav.write_sample((value * i16::MAX as f32) as i16).unwrap();
    }
    wav.finalize().unwrap();
}
fn source(path: &Path, recursive: bool) -> FolderSource {
    FolderSource {
        path: path.to_str().unwrap().into(),
        recursive,
        watch: true,
    }
}
#[test]
fn scan_sorted_recursive_and_relink() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    for name in ["z.wav", "a.wav", "sub/b.wav"] {
        tone(&dir.path().join(name), 0.3, 0);
    }
    std::fs::write(dir.path().join("skip.txt"), "not audio").unwrap();
    let mut progress = Vec::new();
    let items = scan(&source(dir.path(), true), &mut |n| progress.push(n)).unwrap();
    assert!(items.errors.is_empty());
    let items = items.items;
    assert_eq!(items.len(), 3);
    assert_eq!(progress, vec![1, 2, 3]);
    let paths: Vec<_> = items
        .iter()
        .map(|i| match &i.source {
            radio_model::Source::File { path } => path,
            _ => unreachable!(),
        })
        .collect();
    assert!(paths.windows(2).all(|w| w[0] < w[1]));
    assert!(
        scan(&source(dir.path(), false), &mut |_| {})
            .unwrap()
            .items
            .len()
            < items.len()
    );
    let hash = analyse(dir.path().join("a.wav")).unwrap().hash;
    let moved = dir.path().join("moved.wav");
    std::fs::rename(dir.path().join("a.wav"), &moved).unwrap();
    let found = relink(
        &[(ItemId(9), hash, std::fs::metadata(&moved).unwrap().len())],
        &[source(dir.path(), true)],
    );
    assert!(relink(&[(ItemId(9), hash, 1)], &[source(dir.path(), true)]).is_empty());
    assert_eq!(found[0].0, ItemId(9));
    assert!(Path::new(&found[0].1).exists());
    assert_eq!(analyse(&found[0].1).unwrap().hash, hash);
}
#[test]
fn leading_silence_and_gain() {
    let dir = tempfile::tempdir().unwrap();
    let quiet = dir.path().join("quiet.wav");
    let loud = dir.path().join("loud.wav");
    tone(&quiet, 0.1, 1);
    tone(&loud, 0.6, 1);
    let q = analyse(quiet).unwrap();
    let l = analyse(loud).unwrap();
    assert!(q.timing.cue_in.abs_diff(Duration::from_secs(1)) < Duration::from_millis(50));
    assert!(q.gain_db > l.gain_db);
    assert!(q.timing.segue >= q.timing.cue_in && q.timing.segue <= q.timing.cue_out);
    assert!(q.len >= q.timing.cue_out);
}
#[test]
fn tag_roundtrip_and_silence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silent.wav");
    tone(&path, 0.0, 0);
    let meta = ItemMeta {
        title: "Silver Beacon".into(),
        artist: "Paper Harbor".into(),
        album: "Cloud Steps".into(),
        year: "2026".into(),
        genre: "Test".into(),
        comment: "Generated".into(),
        ..ItemMeta::default()
    };
    write_tags(&path, &meta).unwrap();
    let result = analyse(&path).unwrap();
    assert_eq!(result.meta, meta);
    assert!(result.gain_db.is_finite());
    assert_eq!(result.timing.cue_in, result.timing.cue_out);
    std::fs::write(dir.path().join("broken.wav"), "broken").unwrap();
    assert!(analyse(dir.path().join("broken.wav")).is_err());
}
#[test]
fn watch_added_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut watcher = Watcher::new(&source(dir.path(), false)).unwrap();
    let path = dir.path().join("new.wav");
    tone(&path, 0.3, 0);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if watcher
            .events()
            .unwrap()
            .any(|e| matches!(e, WatchEvent::Added(p) if p.canonicalize().unwrap() == path.canonicalize().unwrap()))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("watcher did not report the new file within 5 seconds");
}

#[test]
fn scan_keeps_good_files_after_analysis_failure() {
    let dir = tempfile::tempdir().unwrap();
    tone(&dir.path().join("a.wav"), 0.3, 0);
    std::fs::write(dir.path().join("broken.mp3"), b"not audio").unwrap();
    tone(&dir.path().join("z.wav"), 0.3, 0);
    let result = scan(&source(dir.path(), false), &mut |_| {}).unwrap();
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].0.ends_with("broken.mp3"));
    assert!(scan(&source(&dir.path().join("absent"), false), &mut |_| {}).is_err());
}

#[test]
fn a_listing_stops_at_its_limit_and_keeps_the_first_files_in_name_order() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["e.mp3", "c.mp3", "a.mp3", "d.mp3", "b.mp3", "notes.txt"] {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    let limits = radio_library::Limits {
        max_files: 3,
        ..radio_library::Limits::DEFAULT
    };
    let list = radio_library::list_audio(dir.path(), false, limits);
    let names: Vec<String> = list
        .files
        .iter()
        .map(|f| {
            Path::new(f)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        list.truncated,
        "a limit that the folder passes marks the listing"
    );
    assert_eq!(
        names,
        ["a.mp3", "b.mp3", "c.mp3"],
        "the first files by name stay"
    );
    let all = radio_library::list_audio(dir.path(), false, radio_library::Limits::DEFAULT);
    assert!(!all.truncated);
    assert_eq!(all.files.len(), 5, "only audio files count");
    let gone = radio_library::list_audio(&dir.path().join("missing"), false, limits);
    assert!(
        !gone.errors.is_empty(),
        "an unreadable folder reports an error"
    );
}

#[test]
fn a_probe_reads_the_length_without_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.wav");
    tone(&path, 0.3, 1);
    let (len, _) = radio_library::probe(&path).unwrap();
    assert!(
        len.abs_diff(Duration::from_secs(4)) < Duration::from_millis(50),
        "{len:?}"
    );
}

/// Writes a mono 48 kHz tone without lead silence, `secs` long.
fn tone_for(path: &Path, amplitude: f32, secs: u32) {
    let mut wav = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for n in 0..secs * 48000 {
        let value = amplitude * (n as f32 * 997.0 * std::f32::consts::TAU / 48000.0).sin();
        wav.write_sample((value * i16::MAX as f32) as i16).unwrap();
    }
    wav.finalize().unwrap();
}

/// A quiet tone gets a boost and a loud one a cut. With the gain applied to the samples,
/// both measure close to the target.
#[test]
fn the_measured_gain_brings_quiet_and_loud_tones_to_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let settings = radio_model::LoudnessSettings::default();
    let mut gains = Vec::new();
    for (name, amp) in [("quiet", 0.08), ("loud", 0.7)] {
        let path = dir.path().join(format!("{name}.wav"));
        tone_for(&path, amp, 4);
        let a = analyse(&path).unwrap();
        let loudness = a.loudness.expect("a tone has a loudness");
        let gain = settings.gain(Some((loudness, radio_model::GainSource::Measured)), 0.0);
        let again = dir.path().join(format!("{name}-gained.wav"));
        tone_for(&again, amp * 10f32.powf(gain.db / 20.0), 4);
        let after = analyse(&again).unwrap().loudness.unwrap().lufs;
        assert!(
            (after - settings.target_lufs).abs() < 1.0,
            "{name}: {} LUFS + {} dB measures {after} LUFS",
            loudness.lufs,
            gain.db
        );
        gains.push(gain.db);
    }
    assert!(gains[0] > 0.0 && gains[1] < 0.0, "gains {gains:?}");
}

/// The true peak of a tone is at least its sample peak, and a silent file has no
/// loudness, so it plays at unity.
#[test]
fn analysis_keeps_the_true_peak_and_silence_has_no_loudness() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peak.wav");
    tone_for(&path, 0.5, 2);
    let peak = analyse(&path).unwrap().loudness.unwrap().peak_db.unwrap();
    let sample_peak = 20.0 * 0.5f32.log10();
    assert!(
        peak >= sample_peak - 0.1 && peak < sample_peak + 1.0,
        "{peak}"
    );
    let silent = dir.path().join("silent.wav");
    tone_for(&silent, 0.0, 2);
    assert!(analyse(&silent).unwrap().loudness.is_none());
}

/// A ReplayGain tag gives a loudness: a negative tag gain means a file louder than the
/// ReplayGain reference.
#[test]
fn replaygain_tags_give_a_loudness() {
    use lofty::{
        file::TaggedFileExt,
        tag::{ItemKey, Tag, TagExt, TagType},
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tagged.wav");
    tone_for(&path, 0.2, 1);
    assert!(
        radio_library::read_loudness_tags(&path).is_none(),
        "no tag yet"
    );
    let mut tag = Tag::new(TagType::Id3v2);
    tag.insert_text(ItemKey::ReplayGainTrackGain, "-4.50 dB".into());
    tag.insert_text(ItemKey::ReplayGainTrackPeak, "0.891251".into());
    tag.save_to_path(&path, lofty::config::WriteOptions::default())
        .unwrap();
    assert!(
        lofty::read_from_path(&path)
            .unwrap()
            .tag(TagType::Id3v2)
            .is_some()
    );
    let l = radio_library::read_loudness_tags(&path).expect("a tagged loudness");
    assert!(l.lufs > -18.0, "louder than the reference: {}", l.lufs);
    let peak = l.peak_db.expect("a tagged peak");
    assert!(peak < 0.0 && peak > -2.0, "{peak}");
}
