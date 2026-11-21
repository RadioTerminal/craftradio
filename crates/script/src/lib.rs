//! Bounded, isolated evaluation of declarative playlist scripts.
use radio_model::{
    Constraint, FixMode, ItemId, PickOrder, Placement, PlaylistId, Repeat, Rule, Trigger,
};
use rquickjs::{Context, Function, Runtime, Value, function::Rest};
use serde_json::{Value as Json, json};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSet {
    pub pick: PickOrder,
    pub rules: Vec<Rule>,
    pub constraints: Vec<Constraint>,
}
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("syntax error on line {line}: {message}")]
    Syntax { line: u32, message: String },
    #[error("script failed: {0}")]
    Runtime(String),
    #[error("invalid {call}: {reason}")]
    Invalid { call: String, reason: String },
    #[error("script time limit exceeded")]
    TimeLimit,
    #[error("script memory limit exceeded")]
    MemoryLimit,
}
#[derive(Debug, Clone)]
pub struct Limits {
    pub time: Duration,
    pub memory_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            time: Duration::from_millis(200),
            memory_bytes: 16 * 1024 * 1024,
        }
    }
}
fn invalid(call: &str, reason: impl ToString) -> ScriptError {
    ScriptError::Invalid {
        call: call.into(),
        reason: reason.to_string(),
    }
}
fn number(v: &Json, call: &str, max: u64) -> Result<u64, ScriptError> {
    v.as_u64()
        .filter(|n| *n > 0 && *n <= max)
        .ok_or_else(|| invalid(call, "expected a positive integer in range"))
}
fn duration(v: &Json, scale: f64, call: &str) -> Result<Duration, ScriptError> {
    let n = v
        .as_f64()
        .ok_or_else(|| invalid(call, "expected a duration"))?
        * scale;
    Duration::try_from_secs_f64(n)
        .map_err(|_| invalid(call, "duration must be finite, nonnegative and in range"))
}
fn time(v: &Json) -> Result<jiff::civil::Time, ScriptError> {
    v.as_str()
        .unwrap_or("")
        .parse()
        .map_err(|e| invalid("time", e))
}
fn constraint(v: &Json, playlists: &[(String, PlaylistId)]) -> Result<Constraint, ScriptError> {
    let encoded = v
        .as_str()
        .ok_or_else(|| invalid("constraint", "use a constraint helper"))?;
    let (name, args): (String, Vec<Json>) =
        serde_json::from_str(encoded).map_err(|e| invalid("constraint", e))?;
    let a = args.first().unwrap_or(&Json::Null);
    Ok(match name.as_str() {
        "notAdjacentToSelf" => Constraint::NotAdjacentToSelf,
        "notAdjacentTo" => Constraint::NotAdjacentTo(
            playlists
                .iter()
                .find(|(n, _)| Some(n.as_str()) == a.as_str())
                .ok_or_else(|| invalid(&name, "unknown playlist name"))?
                .1,
        ),
        "minGapSameItem" => Constraint::MinGapSameItem(duration(a, 60., &name)?),
        "artistSeparation" => Constraint::ArtistSeparation(duration(a, 60., &name)?),
        "titleSeparation" => Constraint::TitleSeparation(duration(a, 60., &name)?),
        "daypart" => Constraint::Daypart {
            from: time(a)?,
            to: time(args.get(1).unwrap_or(&Json::Null))?,
        },
        "maxPerHour" => Constraint::MaxPerHour(number(a, &name, u32::MAX.into())? as u32),
        "maxPerDay" => Constraint::MaxPerDay(number(a, &name, u32::MAX.into())? as u32),
        _ => return Err(invalid(&name, "unknown constraint")),
    })
}
fn rule(v: &Json, playlists: &[(String, PlaylistId)]) -> Result<Rule, ScriptError> {
    let t = v.get("trigger").unwrap_or(v);
    if ["every", "at", "on"]
        .iter()
        .filter(|k| t.get(**k).is_some())
        .count()
        != 1
    {
        return Err(invalid("rule", "one trigger is required"));
    }
    let trigger = if let Some(e) = t.get("every") {
        match (e.get("tracks"), e.get("minutes")) {
            (Some(n), None) => Trigger::EveryNTracks(number(n, "rule", u32::MAX.into())? as u32),
            (None, Some(n)) => Trigger::EveryNMinutes(number(n, "rule", u32::MAX.into())? as u32),
            _ => return Err(invalid("rule", "every needs tracks or minutes")),
        }
    } else if let Some(a) = t.get("at") {
        if let Some(m) = a.get("minutes") {
            let minutes = m
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or_else(|| invalid("rule", "minutes must be a nonempty array"))?
                .iter()
                .map(|n| {
                    n.as_u64()
                        .filter(|n| *n < 60)
                        .map(|n| n as u8)
                        .ok_or_else(|| invalid("rule", "minute must be 0 through 59"))
                })
                .collect::<Result<_, _>>()?;
            Trigger::AtMinutes(minutes)
        } else {
            let date: jiff::civil::Date = a
                .get("date")
                .and_then(Json::as_str)
                .unwrap_or("1970-01-01")
                .parse()
                .map_err(|e| invalid("rule", e))?;
            let repeat = match a["repeat"].as_str() {
                Some("once") => Repeat::Once,
                Some("daily") => Repeat::Daily,
                Some("weekdays") => Repeat::Weekdays,
                Some("weekly") => Repeat::Weekly,
                _ => return Err(invalid("rule", "unknown repeat")),
            };
            Trigger::At {
                at: date.to_datetime(time(&a["time"])?),
                repeat,
            }
        }
    } else {
        Trigger::ScriptEvent(
            t["on"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid("rule", "event name is required"))?
                .into(),
        )
    };
    let placement = match v.get("place") {
        None | Some(Json::String(_)) if v.get("place").is_none_or(|p| p == "between") => {
            Placement::BetweenTracks
        }
        Some(p) => Placement::Fixed {
            mode: match p["fixed"].as_str() {
                Some("soft") => FixMode::Soft,
                Some("hard") => FixMode::Hard,
                _ => return Err(invalid("rule", "unknown placement")),
            },
            tolerance: duration(&p["tolerance"], 1., "rule")?,
        },
        None => Placement::BetweenTracks,
    };
    let constraints = match v.get("constraints") {
        None => Vec::new(),
        Some(c) => c
            .as_array()
            .ok_or_else(|| invalid("rule", "constraints must be an array"))?
            .iter()
            .map(|c| constraint(c, playlists))
            .collect::<Result<_, _>>()?,
    };
    Ok(Rule {
        trigger,
        placement,
        take: v
            .get("take")
            .map(|n| number(n, "rule", 255))
            .transpose()?
            .unwrap_or(1) as u8,
        constraints,
    })
}
fn pick(args: &[Json]) -> Result<PickOrder, ScriptError> {
    let a = args.first().unwrap_or(&Json::Null);
    let options = args.get(1).unwrap_or(&Json::Null);
    Ok(match a.as_str() {
        Some("in_order") => PickOrder::InOrder,
        Some("least_recently_played") => PickOrder::LeastRecentlyPlayed,
        Some("shuffle") => PickOrder::Shuffle {
            seed: match options.get("seed") {
                None => 0,
                Some(s) => s
                    .as_u64()
                    .or_else(|| s.as_str().and_then(|s| s.parse().ok()))
                    .ok_or_else(|| invalid("pick", "seed must be an unsigned integer"))?,
            },
        },
        Some("weighted") => {
            let weights = &options["weights"];
            let entries: Vec<(String, Json)> = if let Some(object) = weights.as_object() {
                object
                    .iter()
                    .map(|(id, n)| (id.clone(), n.clone()))
                    .collect()
            } else if let Some(array) = weights.as_array() {
                array
                    .iter()
                    .map(|pair| {
                        let id = pair[0].as_str().ok_or_else(|| {
                            invalid("pick", "weight entry needs an item id string")
                        })?;
                        Ok((id.to_owned(), pair[1].clone()))
                    })
                    .collect::<Result<_, ScriptError>>()?
            } else {
                return Err(invalid(
                    "pick",
                    "weights must be an object or ordered pairs",
                ));
            };
            PickOrder::Weighted {
                weights: entries
                    .into_iter()
                    .map(|(id, n)| {
                        Ok((
                            ItemId(id.parse().map_err(|_| invalid("pick", "invalid item id"))?),
                            number(&n, "pick", u32::MAX.into())? as u32,
                        ))
                    })
                    .collect::<Result<_, ScriptError>>()?,
            }
        }
        _ => return Err(invalid("pick", "unknown pick order")),
    })
}

/// Evaluate a script on the calling thread. Call this away from audio and scheduler threads.
pub fn eval_rules(
    src: &str,
    playlists: &[(String, PlaylistId)],
    limits: &Limits,
) -> Result<RuleSet, ScriptError> {
    let runtime = Runtime::new().map_err(|_| ScriptError::MemoryLimit)?;
    runtime.set_memory_limit(limits.memory_bytes);
    let start = Instant::now();
    let expired = Arc::new(AtomicBool::new(false));
    let flag = expired.clone();
    let budget = limits.time;
    runtime.set_interrupt_handler(Some(Box::new(move || {
        let hit = start.elapsed() >= budget;
        if hit {
            flag.store(true, Ordering::Relaxed);
        }
        hit
    })));
    let context = Context::full(&runtime).map_err(|_| ScriptError::MemoryLimit)?;
    let set = Rc::new(RefCell::new(RuleSet {
        pick: PickOrder::InOrder,
        rules: vec![],
        constraints: vec![],
    }));
    let failure = Rc::new(RefCell::new(None));
    context.with(|ctx| -> Result<(), ScriptError> {
        for name in [
            "pick",
            "rule",
            "constraint",
            "notAdjacentToSelf",
            "notAdjacentTo",
            "minGapSameItem",
            "artistSeparation",
            "titleSeparation",
            "daypart",
            "maxPerHour",
            "maxPerDay",
        ] {
            let set = set.clone();
            let failure = failure.clone();
            let playlists = playlists.to_vec();
            let f = Function::new(
                ctx.clone(),
                move |args: Rest<Value<'_>>| -> rquickjs::Result<String> {
                    let ctx = match args.0.first() {
                        Some(v) => v.ctx().clone(),
                        None if name == "notAdjacentToSelf" => {
                            return Ok(json!([name, []]).to_string());
                        }
                        None => {
                            *failure.borrow_mut() = Some(invalid(name, "an argument is required"));
                            return Err(rquickjs::Error::new_from_js_message(
                                "undefined",
                                "argument",
                                "an argument is required",
                            ));
                        }
                    };
                    let mut values = Vec::new();
                    for v in args.0 {
                        let s = ctx
                            .json_stringify(v)?
                            .ok_or_else(|| {
                                rquickjs::Exception::throw_message(&ctx, "argument is not JSON")
                            })?
                            .to_string()?;
                        values.push(serde_json::from_str::<Json>(&s).map_err(|_| {
                            rquickjs::Exception::throw_message(&ctx, "invalid JSON")
                        })?);
                    }
                    let result = match name {
                        "pick" => pick(&values).map(|p| set.borrow_mut().pick = p),
                        "rule" => rule(values.first().unwrap_or(&Json::Null), &playlists)
                            .map(|r| set.borrow_mut().rules.push(r)),
                        "constraint" => {
                            constraint(values.first().unwrap_or(&Json::Null), &playlists)
                                .map(|c| set.borrow_mut().constraints.push(c))
                        }
                        _ => {
                            let s = json!([name, values]).to_string();
                            match constraint(&Json::String(s.clone()), &playlists) {
                                Ok(_) => return Ok(s),
                                Err(e) => Err(e),
                            }
                        }
                    };
                    if let Err(e) = result {
                        *failure.borrow_mut() = Some(e);
                        return Err(rquickjs::Exception::throw_message(
                            &ctx,
                            "invalid script call",
                        ));
                    }
                    Ok(String::new())
                },
            )
            .map_err(|e| ScriptError::Runtime(e.to_string()))?;
            ctx.globals()
                .set(name, f)
                .map_err(|e| ScriptError::Runtime(e.to_string()))?;
        }
        // Remove all clock and random entry points, including constructor access through prototypes.
        ctx.eval::<(), _>("delete globalThis.Date; delete Math.random;")
            .map_err(|e| ScriptError::Runtime(e.to_string()))?;
        if let Err(e) = ctx.eval::<(), _>(src) {
            if expired.load(Ordering::Relaxed) {
                return Err(ScriptError::TimeLimit);
            }
            if let Some(e) = failure.borrow_mut().take() {
                return Err(e);
            }
            let caught = ctx.catch();
            let object = caught.as_object();
            let message = object
                .and_then(|o| o.get::<_, String>("message").ok())
                .unwrap_or_else(|| format!("{caught:?}: {e}"));
            if message.contains("out of memory") {
                return Err(ScriptError::MemoryLimit);
            }
            if object
                .and_then(|o| o.get::<_, String>("name").ok())
                .as_deref()
                == Some("SyntaxError")
            {
                let line = object
                    .and_then(|o| o.get::<_, u32>("lineNumber").ok())
                    .or_else(|| {
                        object
                            .and_then(|o| o.get::<_, String>("stack").ok())
                            .and_then(|s| {
                                s.split("eval_script:")
                                    .nth(1)
                                    .and_then(|s| s.split(':').next())
                                    .and_then(|s| s.parse().ok())
                            })
                    })
                    .unwrap_or(1);
                return Err(ScriptError::Syntax { line, message });
            }
            return Err(ScriptError::Runtime(message));
        }
        if let Some(e) = failure.borrow_mut().take() {
            return Err(e);
        }
        Ok(())
    })?;
    Ok(set.borrow().clone())
}
fn constraint_script(c: &Constraint, playlists: &[(String, PlaylistId)]) -> String {
    match c {
        Constraint::NotAdjacentToSelf => "notAdjacentToSelf()".into(),
        Constraint::NotAdjacentTo(id) => format!(
            "notAdjacentTo({})",
            json!(
                playlists
                    .iter()
                    .find(|(_, p)| p == id)
                    .map(|(n, _)| n.as_str())
                    .unwrap_or("")
            )
        ),
        Constraint::MinGapSameItem(d) => format!("minGapSameItem({})", d.as_secs_f64() / 60.),
        Constraint::ArtistSeparation(d) => format!("artistSeparation({})", d.as_secs_f64() / 60.),
        Constraint::TitleSeparation(d) => format!("titleSeparation({})", d.as_secs_f64() / 60.),
        Constraint::Daypart { from, to } => format!(
            "daypart({}, {})",
            json!(from.to_string()),
            json!(to.to_string())
        ),
        Constraint::MaxPerHour(n) => format!("maxPerHour({n})"),
        Constraint::MaxPerDay(n) => format!("maxPerDay({n})"),
    }
}
/// Produce a canonical declarative script. Referenced playlist ids must have unique names.
pub fn generate(set: &RuleSet, playlists: &[(String, PlaylistId)]) -> String {
    let mut out = match &set.pick {
        PickOrder::InOrder => "pick(\"in_order\");\n".into(),
        PickOrder::LeastRecentlyPlayed => "pick(\"least_recently_played\");\n".into(),
        PickOrder::Shuffle { seed } => format!(
            "pick(\"shuffle\", {{seed: {}}});\n",
            json!(seed.to_string())
        ),
        PickOrder::Weighted { weights } => {
            let entries = weights
                .iter()
                .map(|(id, n)| format!("[{}, {n}]", json!(id.0.to_string())))
                .collect::<Vec<_>>()
                .join(",");
            format!("pick(\"weighted\", {{weights: [{entries}]}});\n")
        }
    };
    for r in &set.rules {
        let trigger = match &r.trigger {
            Trigger::EveryNTracks(n) => json!({"every": {"tracks": n}}),
            Trigger::EveryNMinutes(n) => json!({"every": {"minutes": n}}),
            Trigger::AtMinutes(m) => json!({"at": {"minutes": m}}),
            Trigger::At { at, repeat } => {
                json!({"at": {"date": at.date().to_string(), "time": at.time().to_string(), "repeat": match repeat { Repeat::Once => "once", Repeat::Daily => "daily", Repeat::Weekdays => "weekdays", Repeat::Weekly => "weekly" }}})
            }
            Trigger::ScriptEvent(s) => json!({"on": s}),
        };
        let place = match r.placement {
            Placement::BetweenTracks => json!("between"),
            Placement::Fixed { mode, tolerance } => {
                json!({"fixed": if mode == FixMode::Hard {"hard"} else {"soft"}, "tolerance": tolerance.as_secs_f64()})
            }
        };
        let constraints = r
            .constraints
            .iter()
            .map(|c| constraint_script(c, playlists))
            .collect::<Vec<_>>()
            .join(", ");
        let mut fields = trigger.to_string();
        fields.pop();
        out.push_str(&format!(
            "rule({fields}, place: {place}, take: {}, constraints: [{constraints}]}});\n",
            r.take
        ));
    }
    for c in &set.constraints {
        out.push_str(&format!(
            "constraint({});\n",
            constraint_script(c, playlists)
        ));
    }
    out
}
