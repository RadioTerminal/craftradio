use jiff::Timestamp;
use radio_engine::{Command, Engine, EngineError, LogEntry, LogStatus, Tier, registry};
use radio_model::{Item, ItemId, PlaylistId, Source};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

mod dynamics;
mod loudness;
mod one_off;
mod pairing;

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
            lock: true,
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

/// A person who adds items locks them. A folder or a watch adds items without a lock. The
/// main list shows a pin only on what a person placed. Old JSON without `lock` locks.
#[test]
fn only_hand_added_items_are_locked() {
    let mut e = Engine::open_in_memory().unwrap();
    e.dispatch(Command::StationSystem {
        commands: vec![Command::PlaylistAddItems {
            playlist: MAIN,
            items: vec![item(1, "From the folder")],
            at: None,
            lock: false,
        }],
    })
    .unwrap();
    let json = serde_json::json!({
        "id": "playlist.add_items",
        "args": {"playlist": MAIN, "items": [item(2, "By hand")], "at": null},
    });
    e.dispatch(serde_json::from_value(json).unwrap()).unwrap();
    let locks: Vec<(u64, bool)> = e
        .state()
        .station
        .main()
        .items
        .iter()
        .map(|i| (i.id.0, i.locked))
        .collect();
    assert_eq!(locks, [(1, false), (2, true)]);
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

/// Logs an airing of `title` by `artist` on `playlist` from `start` for `secs` seconds.
fn air(e: &Engine, start: i64, secs: i64, playlist: &str, artist: &str, title: &str) {
    let mut entry = entry(start, title);
    entry.playlist = playlist.into();
    entry.artist = Some(artist.into());
    let row = e.log_start(&entry).unwrap();
    let end = Timestamp::from_second(start + secs).unwrap();
    e.log_end(row, end, LogStatus::Aired).unwrap();
}

#[test]
fn plays_report_counts_each_title_once_per_airing_and_sums_its_air_time() {
    let mut e = Engine::open_in_memory().unwrap();
    air(&e, 1_000, 180, "Main", "Velka Thrun", "Glints of Varnholt");
    air(&e, 2_000, 60, "Main", "Velka Thrun", "Glints of Varnholt");
    air(&e, 3_000, 200, "Main", "Orlo Fenn", "Embers of Sorvik");
    // another playlist
    air(&e, 4_000, 30, "Ads", "Kesten Bakery", "Spot A");
    // outside the range
    air(&e, 9_000, 200, "Main", "Orlo Fenn", "Embers of Sorvik");
    let csv = e
        .dispatch(Command::HistoryPlaysCsv {
            since: Timestamp::from_second(0).unwrap(),
            until: Timestamp::from_second(5_000).unwrap(),
            playlist: Some("Main".into()),
        })
        .unwrap()
        .data
        .unwrap();
    let lines: Vec<&str> = csv.as_str().unwrap().lines().collect();
    assert_eq!(lines[0], "artist,title,plays,air_seconds");
    assert!(lines.contains(&"Velka Thrun,Glints of Varnholt,2,240"));
    assert!(lines.contains(&"Orlo Fenn,Embers of Sorvik,1,200"));
    assert_eq!(lines.len(), 3);
}

#[test]
fn airings_report_keeps_one_playlist_and_gives_station_local_times() {
    let e = Engine::open_in_memory().unwrap();
    // 2026-01-15 23:30:00 UTC is 00:30 the next day in Ljubljana.
    let start: Timestamp = "2026-01-15T23:30:00Z".parse().unwrap();
    air(&e, start.as_second(), 30, "Ads", "Kesten Bakery", "Spot A");
    air(
        &e,
        start.as_second() + 60,
        180,
        "Main",
        "Velka Thrun",
        "Glints of Varnholt",
    );
    let cut = e
        .log_start(&LogEntry {
            playlist: "Ads".into(),
            ..entry(start.as_second() + 300, "Spot B")
        })
        .unwrap();
    let end = Timestamp::from_second(start.as_second() + 310).unwrap();
    e.log_end(cut, end, LogStatus::Interrupted).unwrap();
    let csv = e
        .log_handle()
        .unwrap()
        .airings_csv(
            Timestamp::UNIX_EPOCH,
            Timestamp::MAX,
            Some("Ads"),
            "Europe/Ljubljana",
        )
        .unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        lines[0],
        "date,start,end,seconds,playlist,title,artist,rule,status"
    );
    assert_eq!(lines.len(), 3, "only the Ads airings: {lines:?}");
    assert!(lines[1].starts_with("2026-01-16,00:30:00,00:30:30,30,Ads,Spot A,"));
    assert!(lines[1].ends_with(",aired"));
    assert!(lines[2].ends_with(",interrupted"));
}

#[test]
fn registry_ids_unique_and_onair_marked() {
    let specs = registry();
    let unique: BTreeSet<&str> = specs.iter().map(|s| s.id).collect();
    assert_eq!(unique.len(), specs.len());

    let read_only: BTreeSet<&str> = [
        "station.get",
        "history.list",
        "history.export_csv",
        "history.plays_csv",
        "history.airings_csv",
    ]
    .into();
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
            fade_in: Duration::ZERO,
        },
        Command::CartStopAll,
        Command::StationGet,
        Command::HistoryList { since: t, limit: 1 },
        Command::HistoryExportCsv { since: t, until: t },
        Command::HistoryPlaysCsv {
            since: t,
            until: t,
            playlist: None,
        },
        Command::HistoryAiringsCsv {
            since: t,
            until: t,
            playlist: None,
        },
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
        lock: true,
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
            fade_in: Duration::ZERO,
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
            fade_in: Duration::ZERO,
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

#[test]
fn undo_takes_back_at_least_fifty_edits_and_redo_does_them_again() {
    let mut e = Engine::open_in_memory().unwrap();
    for i in 0..60 {
        e.dispatch(Command::PlaylistAddItems {
            playlist: radio_model::PlaylistId(1),
            items: vec![item(100 + i, &format!("t{i}"))],
            at: None,
            lock: true,
        })
        .unwrap();
    }
    for _ in 0..50 {
        e.dispatch(Command::HistoryUndo).unwrap();
    }
    assert_eq!(ids(&e).len(), 10, "fifty adds are undone");
    // Mode is not a playlist edit, so it neither makes a step nor clears redo.
    e.dispatch(Command::ModeSet {
        mode: radio_engine::Mode::Assist,
    })
    .unwrap();
    e.dispatch(Command::HistoryRedo).unwrap();
    assert_eq!(ids(&e).len(), 11, "redo adds one back");
    e.dispatch(Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: vec![item(999, "new")],
        at: None,
        lock: true,
    })
    .unwrap();
    assert!(
        e.dispatch(Command::HistoryRedo).is_err(),
        "a new edit clears the redo steps"
    );
}

#[test]
fn a_failed_batch_changes_nothing_and_undoes_as_one_step() {
    let mut e = Engine::open_in_memory().unwrap();
    let add = |id, title: &str| Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: vec![item(id, title)],
        at: None,
        lock: true,
    };
    let bad = e.dispatch(Command::StationBatch {
        commands: vec![add(1, "a"), add(1, "duplicate id")],
    });
    assert!(bad.is_err());
    assert!(ids(&e).is_empty(), "the failed batch left nothing");
    e.dispatch(Command::StationBatch {
        commands: vec![add(1, "a"), add(2, "b")],
    })
    .unwrap();
    assert_eq!(ids(&e).len(), 2);
    e.dispatch(Command::HistoryUndo).unwrap();
    assert!(ids(&e).is_empty(), "one undo takes back the whole batch");
}

#[test]
fn an_imported_station_replaces_the_playlists_and_survives_a_reopen() {
    let path = temp_db("import");
    let mut station = Engine::open_in_memory().unwrap().state().station.clone();
    station.tz = "Europe/Ljubljana".into();
    station.playlists[0].name = "Imported".into();
    station.playlists[0].items.push(item(7, "seven"));
    {
        let mut e = Engine::open(&path).unwrap();
        e.dispatch(Command::StationImport { station }).unwrap();
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.state().station.main().name, "Imported");
    assert_eq!(e.state().station.tz, "Europe/Ljubljana");
    assert_eq!(ids(&e), [7]);
}

#[test]
fn ui_settings_survive_a_reopen_beside_the_engine_parts() {
    let path = temp_db("ui-settings");
    {
        let mut e = Engine::open(&path).unwrap();
        e.dispatch(Command::SettingsSet {
            part: "library".into(),
            key: "docked".into(),
            value: serde_json::json!(true),
        })
        .unwrap();
        e.dispatch(Command::LibraryAddFolder {
            path: "/music".into(),
        })
        .unwrap();
        e.dispatch(Command::PlaylistAddItems {
            playlist: radio_model::PlaylistId(1),
            items: vec![item(1, "one")],
            at: None,
            lock: true,
        })
        .unwrap();
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    let docked = e.state().settings("library").and_then(|s| s.get("docked"));
    assert_eq!(docked, Some(&serde_json::json!(true)));
    assert!(
        e.state().folders.contains("/music"),
        "the engine part stays apart"
    );
}

#[test]
fn edited_tags_reach_the_item_that_playout_reads() {
    let mut e = Engine::open_in_memory().unwrap();
    e.dispatch(Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: vec![item(1, "one")],
        at: None,
        lock: true,
    })
    .unwrap();
    let meta = radio_model::ItemMeta {
        title: "New Title".into(),
        publish: radio_model::PublishMode::Custom,
        custom: "{title} live".into(),
        ..Default::default()
    };
    e.dispatch(Command::ItemSetMeta {
        item: radio_model::ItemId(1),
        meta: serde_json::to_value(&meta).unwrap(),
    })
    .unwrap();
    assert_eq!(e.state().station.main().items[0].meta, meta);
    e.dispatch(Command::ItemSetKeepAlive {
        item: radio_model::ItemId(1),
        keep_alive: false,
    })
    .unwrap();
    assert!(matches!(
        e.state().station.main().items[0].source,
        radio_model::Source::Stream {
            keep_alive: false,
            ..
        }
    ));
}

#[test]
fn start_up_edits_leave_no_undo_steps() {
    let mut e = Engine::open_in_memory().unwrap();
    e.dispatch(Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: vec![item(1, "seeded")],
        at: None,
        lock: true,
    })
    .unwrap();
    e.clear_history();
    assert!(e.dispatch(Command::HistoryUndo).is_err(), "nothing to undo");
    // A system edit, such as a watched folder, is no undo step and ends the history.
    e.dispatch(Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: vec![item(2, "by hand")],
        at: None,
        lock: true,
    })
    .unwrap();
    e.dispatch(Command::StationSystem {
        commands: vec![Command::PlaylistAddItems {
            playlist: radio_model::PlaylistId(1),
            items: vec![item(3, "from the folder")],
            at: None,
            lock: true,
        }],
    })
    .unwrap();
    assert!(e.dispatch(Command::HistoryUndo).is_err());
    assert_eq!(ids(&e), [1, 2, 3]);
}

#[test]
fn undo_never_takes_the_item_on_air_off_its_deck() {
    let mut e = Engine::open_in_memory().unwrap();
    e.dispatch(Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items: vec![item(1, "on deck")],
        at: None,
        lock: true,
    })
    .unwrap();
    e.set_deck(0, Some(radio_model::ItemId(1)));
    e.set_live(0, true);
    let refused = e.dispatch(Command::HistoryUndo);
    assert!(refused.is_err(), "the undo would take the deck's item away");
    assert_eq!(ids(&e), [1], "nothing changed");
}

#[test]
fn quick_repeats_of_one_cart_make_one_undo_step() {
    let mut e = Engine::open_in_memory().unwrap();
    for db in [-3.0, -6.0, -9.0] {
        e.dispatch(Command::CartSet {
            page: 0,
            slot: 0,
            duck_db: db,
            mode: radio_engine::CartMode::OneShot,
        })
        .unwrap();
    }
    e.dispatch(Command::HistoryUndo).unwrap();
    assert!(
        e.state().cart_settings.is_empty(),
        "one undo takes back the drag"
    );
}

#[test]
fn clear_keeps_the_item_on_air_and_undo_restores_the_rest_in_order() {
    let mut e = Engine::open_in_memory().unwrap();
    let items: Vec<_> = (1..=5).map(|i| item(i, &format!("t{i}"))).collect();
    e.dispatch(Command::PlaylistAddItems {
        playlist: radio_model::PlaylistId(1),
        items,
        at: None,
        lock: true,
    })
    .unwrap();
    e.set_deck(0, Some(radio_model::ItemId(3)));
    e.set_live(0, true);
    let stale = e.dispatch(Command::PlaylistClear {
        playlist: radio_model::PlaylistId(1),
        expect_count: 4,
        remove_locked: true,
    });
    assert!(stale.is_err(), "a changed playlist is not cleared");
    e.dispatch(Command::PlaylistClear {
        playlist: radio_model::PlaylistId(1),
        expect_count: 5,
        remove_locked: true,
    })
    .unwrap();
    assert_eq!(ids(&e), [3], "the item on air stays");
    e.set_live(0, false);
    e.dispatch(Command::HistoryUndo).unwrap();
    assert_eq!(
        ids(&e),
        [1, 2, 3, 4, 5],
        "undo restores every item in order"
    );
    assert!(
        e.state().station.main().items.iter().all(|i| i.locked),
        "and their locks"
    );
}

#[test]
fn checkpoint_failure_and_recovery_are_evented() {
    use radio_engine::EngineEvent;
    let path = temp_db("checkpoint-failure");
    let mut engine = Engine::open(&path).unwrap();
    let events = engine.events();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON playlists BEGIN SELECT RAISE(FAIL, 'write denied'); END;").unwrap();
    engine
        .dispatch(Command::PlaylistAddItems {
            playlist: MAIN,
            items: vec![item(991, "Checkpoint")],
            at: None,
            lock: true,
        })
        .unwrap();
    assert!(engine.flush().is_err());
    assert!(engine.persistence_error().is_some());
    assert!(
        events
            .try_iter()
            .any(|e| matches!(e, EngineEvent::PersistenceFailed { reason: Some(_) }))
    );
    conn.execute_batch("DROP TRIGGER reject_checkpoint")
        .unwrap();
    engine
        .dispatch(Command::PlaylistAddItems {
            playlist: MAIN,
            items: vec![item(992, "Recovery")],
            at: None,
            lock: true,
        })
        .unwrap();
    engine.flush().unwrap();
    assert!(engine.persistence_error().is_none());
    assert!(
        events
            .try_iter()
            .any(|e| matches!(e, EngineEvent::PersistenceFailed { reason: None }))
    );
}

#[test]
fn live_input_with_a_planned_length_can_be_scheduled() {
    let mut e = Engine::open_in_memory().unwrap();
    let mut live = item(500, "Studio");
    live.source = Source::LiveInput {
        device: "Test input".into(),
        channels: [1, 2],
    };
    live.planned_len = Some(Duration::from_secs(60));
    live.timing = None;
    e.dispatch(Command::PlaylistAddItems {
        playlist: MAIN,
        items: vec![live],
        at: None,
        lock: true,
    })
    .unwrap();
    assert!(ids(&e).contains(&500));
}

fn set_strip(
    e: &mut Engine,
    args: serde_json::Value,
) -> Result<radio_engine::Outcome, EngineError> {
    e.dispatch(
        serde_json::from_value(serde_json::json!({"id": "mixer.set_strip", "args": args})).unwrap(),
    )
}

fn set_setting(e: &mut Engine, part: &str, key: &str, value: &str) {
    e.dispatch(Command::SettingsSet {
        part: part.into(),
        key: key.into(),
        value: value.into(),
    })
    .unwrap();
}

fn audio_setting(e: &Engine, key: &str) -> Option<String> {
    e.state()
        .settings("audio")
        .and_then(|s| s.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

#[test]
fn snd_without_a_monitor_device_is_refused_and_not_shown_on() {
    let mut e = Engine::open_in_memory().unwrap();
    let err = set_strip(&mut e, serde_json::json!({"strip": "A", "snd": true})).unwrap_err();
    assert!(err.to_string().contains("Monitor output is not available"));
    assert!(!e.state().strips.get("A").is_some_and(|s| s.snd));
}

#[test]
fn old_pfl_field_still_turns_snd_on() {
    let mut e = Engine::open_in_memory().unwrap();
    set_setting(&mut e, "audio", "monitor", "Desk speakers");
    set_strip(&mut e, serde_json::json!({"strip": "A", "pfl": true})).unwrap();
    assert!(e.state().strips.get("A").is_some_and(|s| s.snd));
}

#[test]
fn a_route_field_is_refused_with_a_message() {
    let mut e = Engine::open_in_memory().unwrap();
    let err = set_strip(
        &mut e,
        serde_json::json!({"strip": "A", "route": "CueOnly"}),
    )
    .unwrap_err();
    assert!(err.to_string().contains("route is gone"), "{err}");
}

#[test]
fn old_cue_device_setting_becomes_the_monitor_device() {
    let path = temp_db("monitor-migration");
    {
        let mut e = Engine::open(&path).unwrap();
        set_setting(&mut e, "audio", "pfl", "Desk speakers");
        e.flush().unwrap();
    }
    {
        let e = Engine::open(&path).unwrap();
        assert_eq!(
            audio_setting(&e, "monitor").as_deref(),
            Some("Desk speakers")
        );
        assert_eq!(audio_setting(&e, "pfl"), None);
        e.flush().unwrap();
    }
    // The old row leaves the database too, so the move happens once.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let old: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM settings_by_part WHERE part = 'ui:audio' AND key = 'pfl'",
            (),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old, 0);
}

#[test]
fn old_strip_rows_load_as_mute_and_snd() {
    let path = temp_db("strip-migration");
    {
        let mut e = Engine::open(&path).unwrap();
        set_setting(&mut e, "ui", "x", "y");
        e.flush().unwrap();
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    for (strip, row) in [
        (
            "A",
            r#"{"gain_db":0.0,"mute":false,"pfl":true,"route":"Out"}"#,
        ),
        (
            "B",
            r#"{"gain_db":0.0,"mute":false,"pfl":false,"route":"CueOnly"}"#,
        ),
        (
            "C",
            r#"{"gain_db":-3.0,"mute":false,"pfl":false,"route":"OutAndCue"}"#,
        ),
        (
            "D",
            r#"{"gain_db":0.0,"mute":false,"pfl":false,"route":"Out"}"#,
        ),
    ] {
        conn.execute(
            "INSERT OR REPLACE INTO settings_by_part (part, key, value) VALUES ('mixer', ?1, ?2)",
            (strip, row),
        )
        .unwrap();
    }
    drop(conn);
    let e = Engine::open(&path).unwrap();
    let s = |k: &str| e.state().strips.get(k).copied().unwrap();
    assert!(s("A").snd && !s("A").mute);
    assert!(s("B").snd && s("B").mute);
    assert!(s("C").snd && !s("C").mute && s("C").gain_db < -2.0);
    assert!(!s("D").snd && !s("D").mute);
}

#[test]
fn a_new_station_has_no_program_device() {
    let e = Engine::open_in_memory().unwrap();
    assert_eq!(
        audio_setting(&e, "out").as_deref(),
        Some(radio_engine::NO_DEVICE)
    );
}

#[test]
fn an_existing_station_without_out_keeps_system_default() {
    let path = temp_db("keep-default-out");
    {
        let mut e = Engine::open(&path).unwrap();
        set_setting(&mut e, "ui", "x", "y");
        e.flush().unwrap();
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "DELETE FROM settings_by_part WHERE part = 'ui:audio' AND key = 'out'",
        (),
    )
    .unwrap();
    drop(conn);
    let e = Engine::open(&path).unwrap();
    assert_eq!(audio_setting(&e, "out"), None);
}

#[test]
fn every_registered_command_deserializes_and_has_typed_arguments() {
    let e = Engine::open_in_memory().unwrap();
    let mut values = serde_json::json!({
        "playlist": 1, "item": 1, "deck": 0, "name": "Test playlist",
        "enabled": true, "fill_pool": false, "on_interrupt": "Resume",
        "items": [], "at": 0, "lock": true, "expect_count": 0, "remove_locked": true,
        "to": 0, "script": "", "pick": "InOrder", "rules": [], "constraints": [],
        "source": {}, "meta": {}, "missing": false, "path": "test.wav",
        "hash": "test", "size": 1, "keep_alive": true, "mode": "Auto",
        "page": 0, "slot": 0, "commands": [], "station": e.state().station,
        "part": "test", "key": "test", "value": true, "color": [0,0,0],
        "duck_db": -6.0, "strip": "A", "since": "2026-11-21T00:00:00Z",
        "until": "2026-11-22T00:00:00Z", "limit": 10,
        "id": 1, "ids": [1]
    });
    // The loudness commands: a measurement and an item trim.
    values["lufs"] = serde_json::json!(-18.0);
    values["db"] = serde_json::json!(-1.5);
    values["start"] = serde_json::json!("2026-11-22T09:00:00");
    values["one_off"] = serde_json::to_value(item(9, "One-off")).unwrap();
    values["address"] = serde_json::json!("https://radio.example.invalid");
    values["document"] = serde_json::json!({
        "version": 1, "slug": "test", "name": "Test", "public_url": "https://radio.example.invalid",
        "icecast": {"host": "radio.example.invalid", "port": 443, "tls": true,
                    "mount": "/test/stream", "user": "source"},
        "stream_url": "https://radio.example.invalid/test/stream", "outputs": []
    });
    let specs = registry();
    let command_schema = schemars::schema_for!(Command).to_value();
    assert_eq!(
        specs.len(),
        command_schema["oneOf"].as_array().unwrap().len()
    );
    for spec in specs {
        let schema = spec.args_schema.as_value();
        let props = schema["properties"].as_object().expect(spec.id);
        let mut args = serde_json::Map::new();
        for key in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            let value = if spec.id == "cart.set" && key == "mode" {
                serde_json::json!("OneShot")
            } else {
                values
                    .get(key)
                    .unwrap_or_else(|| panic!("sample for {}.{key}", spec.id))
                    .clone()
            };
            args.insert(key.into(), value);
        }
        let encoded = if props.is_empty() {
            serde_json::json!({"id": spec.id})
        } else {
            serde_json::json!({"id": spec.id, "args": args})
        };
        let command: Command =
            serde_json::from_value(encoded).unwrap_or_else(|e| panic!("{}: {e}", spec.id));
        assert_eq!(command.id(), spec.id);
        assert_eq!(command.tier(), spec.tier);
        if let Some(args) = serde_json::to_value(command)
            .unwrap()
            .get("args")
            .and_then(|v| v.as_object())
        {
            for key in args.keys() {
                assert!(props.contains_key(key), "{} misses {key}", spec.id);
            }
        }
    }
}

#[test]
fn csv_quotes_formula_prefixes_in_untrusted_tags() {
    let e = Engine::open_in_memory().unwrap();
    for (n, title) in ["=SUM(1,2)", "+test", "-test", "@test", "  =test"]
        .iter()
        .enumerate()
    {
        let row = e.log_start(&entry(1000 + n as i64, title)).unwrap();
        e.log_end(row, Timestamp::from_second(1100).unwrap(), LogStatus::Aired)
            .unwrap();
    }
    let csv = e
        .export_csv(Timestamp::UNIX_EPOCH, Timestamp::from_second(2000).unwrap())
        .unwrap();
    for title in ["'=SUM(1,2)", "'+test", "'-test", "'@test", "'  =test"] {
        assert!(csv.contains(title), "{csv}");
    }
}

#[test]
fn fade_command_distinguishes_default_from_explicit_cut() {
    for (over, expected) in [
        (None, None),
        (
            Some(serde_json::json!({"secs": 0, "nanos": 0})),
            Some(std::time::Duration::ZERO),
        ),
    ] {
        let mut args = serde_json::json!({"deck": 0});
        if let Some(over) = over {
            args["over"] = over;
        }
        let command: Command =
            serde_json::from_value(serde_json::json!({"id": "deck.fade", "args": args})).unwrap();
        let Command::DeckFade { over, .. } = command else {
            panic!("expected deck.fade");
        };
        assert_eq!(over, expected);
    }
}

#[test]
fn acknowledged_alarms_survive_a_reopen_and_cannot_be_undone() {
    let path = temp_db("alarm-ack");
    {
        let mut e = Engine::open(&path).unwrap();
        e.dispatch(Command::AlarmAck { id: 41 }).unwrap();
        e.dispatch(Command::AlarmAckAll {
            ids: vec![41, 7, 9],
        })
        .unwrap();
        assert!(
            e.dispatch(Command::HistoryUndo).is_err(),
            "an acknowledgement is no undo step"
        );
        e.flush().unwrap();
    }
    let e = Engine::open(&path).unwrap();
    for id in [41, 7, 9] {
        assert!(e.state().is_acked(id), "alarm {id} stays acknowledged");
    }
    assert!(!e.state().is_acked(8), "other alarms stay new");
    assert_eq!(e.state().acked_alarms.len(), 3, "each id is kept once");
}

/// A deck past the number in use takes no item: the UI does not show it. The deck is there
/// again once the station uses it.
#[test]
fn a_deck_past_the_number_in_use_takes_no_item() {
    let mut e = Engine::open_in_memory().unwrap();
    e.dispatch(Command::PlaylistAddItems {
        playlist: MAIN,
        items: vec![item(1, "One")],
        at: None,
        lock: true,
    })
    .unwrap();
    let load = Command::DeckLoad {
        deck: 2,
        item: ItemId(1),
    };
    assert!(e.dispatch(load.clone()).is_err(), "two decks by default");
    e.dispatch(Command::SettingsSet {
        part: radio_model::DECKS_PART.into(),
        key: radio_model::DECK_COUNT_KEY.into(),
        value: 3.into(),
    })
    .unwrap();
    e.dispatch(load).unwrap();
}
