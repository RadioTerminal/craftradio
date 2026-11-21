use jiff::Timestamp;
use radio_engine::{Command, Engine, EngineError, LogEntry, LogStatus, Tier, registry};
use radio_model::{Item, ItemId, PlaylistId, Source};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

const MAIN: PlaylistId = PlaylistId(1);

fn item(id: u64, title: &str) -> Item {
    Item {
        id: ItemId(id),
        title: title.into(),
        artist: Some("Test Artist".into()),
        source: Source::Stream {
            url: "http://example.invalid/s".into(),
            keep_alive: true,
        },
        timing: None,
        planned_len: Some(Duration::from_secs(180)),
        movable: true,
        trimmable: true,
        locked: false,
        valid_from: None,
        valid_to: None,
        max_per_day: None,
        meta: Default::default(),
    }
}

fn temp_db(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radio-engine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("station.db")
}

fn ids(e: &Engine) -> Vec<u64> {
    e.state()
        .station
        .main()
        .items
        .iter()
        .map(|i| i.id.0)
        .collect()
}

fn entry(secs: i64, title: &str) -> LogEntry {
    LogEntry {
        start: Timestamp::from_second(secs).unwrap(),
        playlist: "Main".into(),
        title: title.into(),
        artist: Some("Test Artist".into()),
        origin: "music".into(),
        rule: None,
    }
}

#[test]
fn add_move_remove_survive_reopen() {
    let path = temp_db("items");
    {
        let mut e = Engine::open(&path).unwrap();
        let add = Command::PlaylistAddItems {
            playlist: MAIN,
            items: vec![item(1, "One"), item(2, "Two"), item(3, "Three")],
            at: None,
        };
        e.dispatch(add).unwrap();
        e.dispatch(Command::PlaylistMoveItems {
            playlist: MAIN,
            items: vec![ItemId(3)],
            to: 0,
        })
        .unwrap();
        e.dispatch(Command::PlaylistRemoveItems {
            playlist: MAIN,
            items: vec![ItemId(2)],
        })
        .unwrap();
        assert_eq!(ids(&e), vec![3, 1]);
    } // dropping flushes the pending checkpoint
    let e = Engine::open(&path).unwrap();
    assert_eq!(ids(&e), vec![3, 1]);
    // hand edits are locked (spec 6.15)
    assert!(e.state().station.main().items.iter().all(|i| i.locked));
}

#[test]
fn bad_rules_script_reports_line_and_keeps_old_rules() {
    let mut e = Engine::open_in_memory().unwrap();
    let good = "rule({at: {minutes: [20, 50]}});";
    e.dispatch(Command::PlaylistSetRulesScript {
        playlist: MAIN,
        script: good.into(),
    })
    .unwrap();
    let before = e.state().station.main().rules.clone();
    assert!(!before.is_empty());

    let err = e
        .dispatch(Command::PlaylistSetRulesScript {
            playlist: MAIN,
            script: "\n\nrule({every: {tracks: 0}})".into(),
        })
        .unwrap_err();
    assert!(matches!(err, EngineError::Script(_)));
    assert_eq!(err.script_line(), Some(3));
    assert_eq!(e.state().station.main().rules, before);
}

#[test]
fn unfinished_row_is_marked_after_reopen() {
    let path = temp_db("log");
    let (done, cut) = {
        let e = Engine::open(&path).unwrap();
        let done = e.log_start(&entry(1_000, "Finished")).unwrap();
        e.log_end(
            done,
            Timestamp::from_second(1_180).unwrap(),
            LogStatus::Aired,
        )
        .unwrap();
        let cut = e.log_start(&entry(2_000, "Cut off")).unwrap();
        (done, cut)
    };
    let e = Engine::open(&path).unwrap();
    let rows = e.history(Timestamp::from_second(0).unwrap(), 10).unwrap();
    let status = |id| rows.iter().find(|r| r.id == id).unwrap().status;
    assert_eq!(status(cut), Some(LogStatus::Unfinished));
    assert_eq!(status(done), Some(LogStatus::Aired));
}

#[test]
fn csv_has_header_and_one_row_per_aired_item() {
    let e = Engine::open_in_memory().unwrap();
    for n in 0..3 {
        let row = e
            .log_start(&entry(1_000 + n * 200, &format!("Song {n}, live")))
            .unwrap();
        let end = Timestamp::from_second(1_000 + n * 200 + 180).unwrap();
        e.log_end(row, end, LogStatus::Aired).unwrap();
    }
    let csv = e
        .export_csv(
            Timestamp::from_second(0).unwrap(),
            Timestamp::from_second(10_000).unwrap(),
        )
        .unwrap();
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some("start,end,playlist,title,artist,origin,rule,status")
    );
    let body: Vec<&str> = lines.collect();
    assert_eq!(body.len(), 3);
    // a comma inside a title is quoted, not a new column
    assert!(body[0].contains("\"Song 0, live\""));
    assert!(body.iter().all(|l| l.ends_with(",aired")));
}

#[test]
fn registry_ids_unique_and_onair_marked() {
    let specs = registry();
    let unique: BTreeSet<&str> = specs.iter().map(|s| s.id).collect();
    assert_eq!(unique.len(), specs.len());

    let read_only: BTreeSet<&str> = ["station.get", "history.list", "history.export_csv"].into();
    for s in &specs {
        let expected = if read_only.contains(s.id) {
            Tier::ReadOnly
        } else {
            Tier::OnAir
        };
        assert_eq!(s.tier, expected, "{}", s.id);
    }
    // the typed commands and the registry agree on ids and tiers
    let t = Timestamp::UNIX_EPOCH;
    let sample = [
        Command::PlaylistCreate { name: "x".into() },
        Command::DeckPlay {
            deck: 0,
            expect_item: None,
        },
        Command::CartStopAll,
        Command::StationGet,
        Command::HistoryList { since: t, limit: 1 },
        Command::HistoryExportCsv { since: t, until: t },
    ];
    for c in sample {
        let spec = specs.iter().find(|s| s.id == c.id()).expect("registered");
        assert_eq!(spec.tier, c.tier());
    }
}

#[test]
fn deck_commands_queue_intent_and_do_not_claim_audio() {
    let mut e = Engine::open_in_memory().unwrap();
    let events = e.events();
    e.dispatch(Command::PlaylistAddItems {
        playlist: MAIN,
        items: vec![item(1, "One")],
        at: None,
    })
    .unwrap();
    e.dispatch(Command::DeckLoad {
        deck: 0,
        item: ItemId(1),
    })
    .unwrap();
    let out = e
        .dispatch(Command::DeckPlay {
            deck: 0,
            expect_item: None,
        })
        .unwrap();
    assert!(out.audio_required && !out.applied_to_audio);
    let intents = events
        .try_iter()
        .filter(|ev| matches!(ev, radio_engine::EngineEvent::AudioIntent(_)))
        .count();
    assert_eq!(intents, 2);
    assert!(
        e.dispatch(Command::DeckPlay {
            deck: 9,
            expect_item: None,
        })
        .is_err()
    ); // A command for an item the deck no longer holds fails and does nothing.
    let stale = e.dispatch(Command::DeckSkip {
        deck: 0,
        expect_item: Some(ItemId(2)),
    });
    assert!(matches!(
        stale,
        Err(radio_engine::EngineError::StaleTarget { .. })
    ));
    assert_eq!(events.try_iter().count(), 0);
}
