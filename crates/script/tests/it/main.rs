use radio_model::PlaylistId;
use radio_script::{Limits, ScriptError, eval_rules, generate};
use std::time::Duration;

#[test]
fn roundtrip() {
    let playlists = vec![("Ads".into(), PlaylistId(7))];
    let set = eval_rules(
        r#"
        pick("shuffle", {seed: 42});
        rule({at: {minutes: [20, 50]}, take: 3,
              constraints: [notAdjacentToSelf(), maxPerDay(24)]});
        rule({at: {time: "12:30", repeat: "daily"},
              place: {fixed: "hard", tolerance: 2}});
        constraint(notAdjacentTo("Ads"));
    "#,
        &playlists,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(
        eval_rules(&generate(&set, &playlists), &playlists, &Limits::default()).unwrap(),
        set
    );
}
#[test]
fn infinite_loop_hits_time_limit() {
    let limits = Limits {
        time: Duration::from_millis(10),
        ..Limits::default()
    };
    assert!(matches!(
        eval_rules("while(true){}", &[], &limits),
        Err(ScriptError::TimeLimit)
    ));
}
#[test]
fn bad_call_reports_reason() {
    match eval_rules("\nrule({every: {tracks: 0}})", &[], &Limits::default()) {
        Err(ScriptError::Invalid {
            line,
            call,
            field,
            reason,
        }) => {
            assert_eq!(line, 2);
            assert!(field.contains("every.tracks"));
            assert_eq!(call, "rule");
            assert!(reason.contains("positive"));
        }
        result => panic!("unexpected result: {result:?}"),
    }
}
#[test]
fn isolation() {
    for src in [
        "Date.now()",
        "new Date()",
        "Math.random()",
        "fs.readFile('x')",
        "fetch('x')",
    ] {
        assert!(
            matches!(
                eval_rules(src, &[], &Limits::default()),
                Err(ScriptError::Runtime { .. })
            ),
            "{src}"
        );
    }
}
#[test]
fn invalid_calls_and_syntax() {
    for src in [
        "constraint(notAdjacentTo('Missing'))",
        "rule({at:{minutes:[60]}})",
        "rule({every:{minutes:-1}})",
        "rule({on:'x',take:256})",
        "constraint(daypart('25:00','10:00'))",
        "pick('unknown')",
        "try {rule({every:{tracks:0}})} catch(e) {}",
    ] {
        assert!(
            matches!(
                eval_rules(src, &[], &Limits::default()),
                Err(ScriptError::Invalid { .. })
            ),
            "{src}"
        );
    }
    assert!(matches!(
        eval_rules("rule({", &[], &Limits::default()),
        Err(ScriptError::Syntax { line: 1, .. })
    ));
}
#[test]
fn memory_limit() {
    let limits = Limits {
        memory_bytes: 1024 * 1024,
        ..Limits::default()
    };
    assert!(matches!(
        eval_rules(
            "let a=[]; while(true) a.push(new Array(10000).fill('x'));",
            &[],
            &limits
        ),
        Err(ScriptError::MemoryLimit)
    ));
}
#[test]
fn all_variants_roundtrip() {
    for order in [
        "pick('in_order')",
        "pick('least_recently_played')",
        "pick('weighted',{weights:{'12':2,'44':3}})",
    ] {
        let src = format!(
            r#"{order};
            rule({{every:{{tracks:2}}}}); rule({{every:{{minutes:5}}}});
            rule({{on:'pulse', place:{{fixed:'soft',tolerance:0.5}}}});
            rule({{at:{{time:'06:00',date:'2026-11-21',repeat:'once'}}}});
            rule({{at:{{time:'07:00',repeat:'weekdays'}}}});
            rule({{at:{{time:'08:00',repeat:'weekly'}}}});
            constraint(minGapSameItem(2)); constraint(artistSeparation(3));
            constraint(titleSeparation(4)); constraint(daypart('22:00','06:00'));
            constraint(maxPerHour(2));"#
        );
        let set = eval_rules(&src, &[], &Limits::default()).unwrap();
        assert_eq!(
            eval_rules(&generate(&set, &[]), &[], &Limits::default()).unwrap(),
            set
        );
    }
}

#[test]
fn generated_pick_preserves_rust_integer_range_and_weight_order() {
    use radio_model::{ItemId, PickOrder};
    use radio_script::RuleSet;
    for pick in [
        PickOrder::Shuffle { seed: u64::MAX },
        PickOrder::Weighted {
            weights: vec![(ItemId(u64::MAX), 2), (ItemId(1), 4), (ItemId(1), 3)],
        },
    ] {
        let set = RuleSet {
            pick,
            rules: vec![],
            constraints: vec![],
        };
        assert_eq!(
            eval_rules(&generate(&set, &[]), &[], &Limits::default()).unwrap(),
            set
        );
    }
    for src in ["pick()", "rule()", "constraint()", "maxPerDay()"] {
        assert!(matches!(
            eval_rules(src, &[], &Limits::default()),
            Err(ScriptError::Invalid { .. })
        ));
    }
}

#[test]
fn syntax_error_has_source_line() {
    assert!(matches!(
        eval_rules("\n\nrule({", &[], &Limits::default()),
        Err(ScriptError::Syntax { line: 3, .. })
    ));
}

#[test]
fn unknown_key_is_rejected() {
    for (src, key) in [
        ("rule({evry:{tracks:1}})", "evry"),
        ("rule({every:{tracks:1, time:2}})", "every.time"),
        ("rule({at:{minutes:[20],time:'12:00'}})", "at.time"),
        ("rule({trigger:{every:{tracks:1}, typo:2}})", "trigger.typo"),
        (
            "rule({on:'pulse',place:{fixed:'hard',tolerance:1,typo:2}})",
            "place.typo",
        ),
    ] {
        match eval_rules(src, &[], &Limits::default()) {
            Err(ScriptError::Invalid { field, .. }) => assert_eq!(field, key),
            result => panic!("unexpected result: {result:?}"),
        }
    }
}
#[test]
fn runtime_error_has_source_line() {
    assert!(matches!(
        eval_rules("\n\nmissing()", &[], &Limits::default()),
        Err(ScriptError::Runtime { line: 3, .. })
    ));
}
#[test]
fn first_invalid_call_wins() {
    assert!(
        matches!(eval_rules("try {\nrule({every:{tracks:0}})\n} catch(e) {}\ntry {pick('bad')} catch(e) {}", &[], &Limits::default()), Err(ScriptError::Invalid { line: 2, call, .. }) if call == "rule")
    );
}

#[test]
fn invalid_helpers_report_call_line_and_field() {
    for (src, expected_call, expected_field) in [
        ("\nrule()", "rule", "argument"),
        (
            "\nrule({on:'pulse',place:{fixed:'hard',tolerance:-1}})",
            "rule",
            "place.tolerance",
        ),
        (
            "\nrule({at:{time:'25:00',repeat:'daily'}})",
            "rule",
            "at.time",
        ),
        ("\nconstraint(daypart('08:00','25:00'))", "daypart", "to"),
    ] {
        match eval_rules(src, &[], &Limits::default()) {
            Err(error @ ScriptError::Invalid { .. }) => {
                assert!(
                    error
                        .to_string()
                        .starts_with(&format!("line 2: {expected_call}(): {expected_field}:"))
                );
            }
            result => panic!("unexpected result: {result:?}"),
        }
    }
}
