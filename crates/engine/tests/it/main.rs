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
            fade_in: Duration::ZERO,
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

#[test]
fn unavailable_cue_output_is_rejected_without_showing_pfl_on() {
    let mut e = Engine::open_in_memory().unwrap();
    let result = e.dispatch(Command::MixerSetStrip {
        strip: "A".into(),
        gain_db: None,
        mute: None,
        pfl: Some(true),
        route: None,
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Cue output is not available")
    );
    assert!(!e.state().strips.get("A").is_some_and(|s| s.pfl));
}

#[test]
fn every_registered_command_deserializes_and_has_typed_arguments() {
    let e = Engine::open_in_memory().unwrap();
    let values = serde_json::json!({
        "playlist": 1, "item": 1, "deck": 0, "name": "Test playlist",
        "enabled": true, "fill_pool": false, "on_interrupt": "Resume",
        "items": [], "at": 0, "lock": true, "expect_count": 0, "remove_locked": true,
        "to": 0, "script": "", "pick": "InOrder", "rules": [], "constraints": [],
        "source": {}, "meta": {}, "missing": false, "path": "test.wav",
        "hash": "test", "size": 1, "keep_alive": true, "mode": "Auto",
        "page": 0, "slot": 0, "commands": [], "station": e.state().station,
        "part": "test", "key": "test", "value": true, "color": [0,0,0],
        "duck_db": -6.0, "strip": "A", "since": "2026-11-21T00:00:00Z",
        "until": "2026-11-22T00:00:00Z", "limit": 10
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
