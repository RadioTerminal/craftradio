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
