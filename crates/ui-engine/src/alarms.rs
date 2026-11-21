//! The alarms and warnings of the station in plain words, for the Alarms window.
//!
//! Each alarm gets an id that stays the same for the same condition. Each new plan raises
//! its warnings again. Their id does not use the values that change between plans.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_model::{ItemId, PlaylistId, Station};
use radio_playout::{Alarm, RaisedAlarm};
use radio_scheduler::{DropReason, FitResult, Warning};
use radio_ui_egui::view::{AlarmSource, AlarmView, Severity};

/// A plan warning this close to the plan start has no fixed time: it moves with each plan.
const NOW_SLACK: jiff::SignedDuration = jiff::SignedDuration::from_secs(5);

/// The FNV-1a hash of the parts, with a separator between them. It never changes between
/// builds, so an id that the engine stores keeps its meaning.
pub(crate) fn stable_hash(parts: &[&str]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut h = OFFSET;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h ^= 0x1f;
            h = h.wrapping_mul(PRIME);
        }
        for b in p.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(PRIME);
        }
    }
    h
}

/// The id of a plan warning, from its kind, playlist, item and target only.
// A new plan that raises the same warning again gives the same id. `fits` are the fits of
// the same plan: they give the target of a fixed item outside its tolerance.
pub fn warning_id(w: &Warning, now: Timestamp, fits: &[FitResult]) -> u64 {
    let ms = |t: &Timestamp| t.as_millisecond().to_string();
    // A time at the plan start moves with each plan, so it is no target.
    let target = |t: &Timestamp| {
        if *t <= now.checked_add(NOW_SLACK).unwrap_or(now) {
            "now".to_owned()
        } else {
            ms(t)
        }
    };
    let p = |p: &PlaylistId| p.0.to_string();
    let i = |i: &ItemId| i.0.to_string();
    match w {
        Warning::NoEligibleItem { playlist, at } => {
            stable_hash(&["no_eligible_item", &p(playlist), "", &target(at)])
        }
        Warning::DeadAir { at, .. } => stable_hash(&["dead_air", "", "", &target(at)]),
        Warning::FixedConflict {
            playlist, wanted, ..
        } => stable_hash(&["fixed_conflict", &p(playlist), "", &ms(wanted)]),
        Warning::OutsideTolerance { playlist, error } => {
            let fit = fits
                .iter()
                .find(|f| f.playlist == *playlist && f.error == *error && !f.within_tolerance);
            let target = fit.map(|f| ms(&f.target)).unwrap_or_default();
            stable_hash(&["outside_tolerance", &p(playlist), "", &target])
        }
        Warning::LockedMoved { item, .. } => stable_hash(&["locked_moved", "", &i(item), ""]),
        Warning::UnknownTimeZone { tz } => stable_hash(&["unknown_time_zone", "", "", tz]),
        Warning::EntryDropped {
            at, playlist, item, ..
        } => stable_hash(&["entry_dropped", &p(playlist), &i(item), &ms(at)]),
    }
}

/// The id of a playout alarm: its time and its content.
pub fn alarm_id(r: &RaisedAlarm) -> u64 {
    let body = serde_json::to_string(&r.alarm).unwrap_or_default();
    stable_hash(&["alarm", &r.at.as_millisecond().to_string(), &body])
}

/// The names that the texts use: items and playlists of the station, and the clock.
pub(crate) struct Names<'a> {
    pub(crate) station: &'a Station,
    pub(crate) tz: &'a TimeZone,
}

impl Names<'_> {
    fn playlist(&self, id: PlaylistId) -> &str {
        self.station
            .playlists
            .iter()
            .find(|p| p.id == id)
            .map_or("?", |p| p.name.as_str())
    }

    fn item(&self, id: ItemId) -> &str {
        self.station
            .playlists
            .iter()
            .flat_map(|p| &p.items)
            .find(|i| i.id == id)
            .map_or("?", |i| i.title.as_str())
    }

    fn clock(&self, at: Timestamp) -> String {
        at.to_zoned(self.tz.clone())
            .strftime("%H:%M:%S")
            .to_string()
    }
}

fn deck_name(deck: u8) -> char {
    ['A', 'B', 'C', 'D']
        .get(usize::from(deck))
        .copied()
        .unwrap_or('?')
}

/// A plan warning in plain words.
pub(crate) fn warning_text(n: &Names<'_>, w: &Warning) -> String {
    match w {
        Warning::NoEligibleItem { playlist, at } => format!(
            "No eligible item in {} at {}",
            n.playlist(*playlist),
            n.clock(*at)
        ),
        Warning::DeadAir { at, gap } => format!(
            "Dead air at {} for {:.1} s: nothing is planned",
            n.clock(*at),
            gap.as_secs_f64()
        ),
        Warning::FixedConflict {
            playlist,
            wanted,
            got,
        } => format!(
            "{} wants {} but starts at {}",
            n.playlist(*playlist),
            n.clock(*wanted),
            n.clock(*got)
        ),
        Warning::OutsideTolerance { playlist, error } => format!(
            "{} is outside its tolerance ({:+.1} s)",
            n.playlist(*playlist),
            error.as_secs_f64()
        ),
        Warning::LockedMoved { item, by } => format!(
            "The locked item \"{}\" moved by {:.1} s",
            n.item(*item),
            by.as_secs_f64()
        ),
        Warning::EntryDropped {
            at,
            playlist,
            item,
            reason,
        } => {
            let (why, rule) = match reason {
                DropReason::AdjacencyAfterMove => ("moved across a fixed event", "an adjacency"),
                DropReason::AdjacencyAfterReplan => {
                    ("left next to its kind by a re-plan", "an adjacency")
                }
                DropReason::RotationAfterTrim => ("moved by a trim", "a rotation"),
            };
            format!(
                "{}: dropped \"{}\" planned at {} ({why}, it would break {rule} rule)",
                n.playlist(*playlist),
                n.item(*item),
                n.clock(*at)
            )
        }
        Warning::UnknownTimeZone { tz } => {
            format!("Unknown time zone '{tz}': local-time rules use UTC")
        }
    }
}

/// A playout alarm in plain words.
pub(crate) fn alarm_text(n: &Names<'_>, a: &Alarm) -> String {
    match a {
        Alarm::ItemSkipped { title, reason, .. } => {
            format!("Could not load \"{title}\": {reason}. The plan skips it")
        }
        Alarm::DeadAir => {
            "Dead air: the program went silent, so playout skipped to the next item".to_owned()
        }
        Alarm::ControlOverrun => {
            "A playout tick took longer than half of the command lead".to_owned()
        }
        Alarm::DeviceLost { name } => {
            format!("Output device lost: {name}. Software pacing keeps Icecast running")
        }
        Alarm::EmergencyFallback { reason } => {
            format!("The emergency fallback folder is playing: {reason}")
        }
        Alarm::StreamReconnecting { item } => format!(
            "The stream \"{}\" has no audio and reconnects; a bed plays under it",
            n.item(*item)
        ),
        Alarm::StreamDown { item } => format!(
            "The stream \"{}\" had no audio for too long, so playout skipped it",
            n.item(*item)
        ),
        Alarm::DeckUnderrun { deck } => {
            format!("Deck {} ran out of decoded audio on air", deck_name(*deck))
        }
        Alarm::ClockStep { by_ms } => {
            format!("The wall clock stepped by {by_ms} ms; the timing re-anchored")
        }
        Alarm::HardEventNotLoaded { item } => format!(
            "The hard event \"{}\" is near, but each deck holds an item on air, so it could not load",
            n.item(*item)
        ),
        Alarm::FixedLate { item, target, by } => format!(
            "\"{}\" started {:.1} s after {}, past its tolerance",
            n.item(*item),
            by.as_secs_f64(),
            n.clock(*target)
        ),
        Alarm::HardEventMissed { item, target } => format!(
            "The hard event \"{}\" at {} did not air: no deck was free",
            n.item(*item),
            n.clock(*target)
        ),
        Alarm::OneOffMissed { title, target, .. } => format!(
            "The one-off \"{title}\" at {} did not air, so it left Upcoming. Add it again if it \
             should still air",
            n.clock(*target)
        ),
        Alarm::OperatorItemEvicted { deck, item } => format!(
            "Deck {} gave up \"{}\" to make room for a hard event",
            deck_name(*deck),
            n.item(*item)
        ),
        Alarm::HardEventLocked { item, target } => format!(
            "The hard event \"{}\" at {} airs as planned; the station change applies after it",
            n.item(*item),
            n.clock(*target)
        ),
        Alarm::HardEventCancelled { item, target } => format!(
            "The hard event \"{}\" at {} does not air: a person removed its item",
            n.item(*item),
            n.clock(*target)
        ),
        Alarm::OutputFallback { reason } => {
            format!("The local output runs on the fallback: {reason}")
        }
        Alarm::LogFailed { reason } => format!("Writing the as-played log failed: {reason}"),
        Alarm::Unsupported { what } => format!("Not available in this build: {what}"),
    }
}

fn alarm_severity(a: &Alarm) -> Severity {
    match a {
        Alarm::DeadAir
        | Alarm::DeviceLost { .. }
        | Alarm::EmergencyFallback { .. }
        | Alarm::StreamDown { .. }
        | Alarm::HardEventNotLoaded { .. }
        | Alarm::HardEventMissed { .. }
        | Alarm::OneOffMissed { .. }
        | Alarm::LogFailed { .. } => Severity::Alarm,
        Alarm::ItemSkipped { .. }
        | Alarm::ControlOverrun
        | Alarm::StreamReconnecting { .. }
        | Alarm::DeckUnderrun { .. }
        | Alarm::FixedLate { .. }
        | Alarm::OperatorItemEvicted { .. }
        | Alarm::HardEventLocked { .. }
        | Alarm::HardEventCancelled { .. }
        | Alarm::OutputFallback { .. } => Severity::Warning,
        Alarm::ClockStep { .. } | Alarm::Unsupported { .. } => Severity::Info,
    }
}

fn alarm_source(a: &Alarm) -> AlarmSource {
    match a {
        Alarm::DeviceLost { .. } | Alarm::OutputFallback { .. } => AlarmSource::Device,
        Alarm::ControlOverrun
        | Alarm::ClockStep { .. }
        | Alarm::LogFailed { .. }
        | Alarm::Unsupported { .. } => AlarmSource::Engine,
        _ => AlarmSource::Playout,
    }
}

fn alarm_item(a: &Alarm) -> Option<ItemId> {
    match a {
        Alarm::ItemSkipped { item, .. }
        | Alarm::StreamReconnecting { item }
        | Alarm::StreamDown { item }
        | Alarm::HardEventNotLoaded { item }
        | Alarm::FixedLate { item, .. }
        | Alarm::HardEventMissed { item, .. }
        | Alarm::OneOffMissed { item, .. }
        | Alarm::OperatorItemEvicted { item, .. }
        | Alarm::HardEventLocked { item, .. }
        | Alarm::HardEventCancelled { item, .. } => Some(*item),
        _ => None,
    }
}

fn warning_target(w: &Warning) -> (Option<PlaylistId>, Option<ItemId>) {
    match w {
        Warning::NoEligibleItem { playlist, .. }
        | Warning::FixedConflict { playlist, .. }
        | Warning::OutsideTolerance { playlist, .. } => (Some(*playlist), None),
        Warning::LockedMoved { item, .. } => (None, Some(*item)),
        Warning::EntryDropped { playlist, item, .. } => (Some(*playlist), Some(*item)),
        Warning::DeadAir { .. } | Warning::UnknownTimeZone { .. } => (None, None),
    }
}

/// The name of the item or else the playlist, for the Concerns column.
fn concerns(n: &Names<'_>, playlist: Option<PlaylistId>, item: Option<ItemId>) -> Option<String> {
    item.map(|i| n.item(i).to_owned())
        .or_else(|| playlist.map(|p| n.playlist(p).to_owned()))
}

/// The view of a playout alarm.
pub(crate) fn from_alarm(n: &Names<'_>, r: &RaisedAlarm) -> AlarmView {
    let item = alarm_item(&r.alarm);
    AlarmView {
        id: alarm_id(r),
        at: r.at.to_zoned(n.tz.clone()),
        severity: alarm_severity(&r.alarm),
        source: alarm_source(&r.alarm),
        text: alarm_text(n, &r.alarm),
        // The first row of an item has the id of the item.
        item: item.map(|i| i.0),
        playlist: None,
        concerns: concerns(n, None, item),
        acked: false,
    }
}

/// The view of a plan warning. The source first saw it at `seen`.
pub(crate) fn from_warning(
    n: &Names<'_>,
    w: &Warning,
    (now, fits): (Timestamp, &[FitResult]),
    seen: Timestamp,
) -> AlarmView {
    let (playlist, item) = warning_target(w);
    AlarmView {
        id: warning_id(w, now, fits),
        at: seen.to_zoned(n.tz.clone()),
        severity: if matches!(w, Warning::DeadAir { .. }) {
            Severity::Alarm
        } else {
            Severity::Warning
        },
        source: AlarmSource::Scheduler,
        text: warning_text(n, w),
        item: item.map(|i| i.0),
        playlist: playlist.map(|p| p.0),
        concerns: concerns(n, playlist, item),
        acked: false,
    }
}

/// An alarm from a state of the station, such as a failed save. Its id comes from `kind`
/// and `key`, so it stays the same while the state lasts.
pub(crate) fn from_state(
    kind: &str,
    key: &str,
    severity: Severity,
    source: AlarmSource,
    text: String,
    at: jiff::Zoned,
) -> AlarmView {
    AlarmView {
        id: stable_hash(&["state", kind, key]),
        at,
        severity,
        source,
        text,
        item: None,
        playlist: None,
        concerns: None,
        acked: false,
    }
}

/// The first characters that make a spreadsheet read a cell as a formula.
const FORMULA_START: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// A field for CSV. A field with a comma, a quote or a line break goes in quotes.
// Titles, paths and stream tags come from outside the station. A field that starts like a
// formula gets a quote mark first and always goes in quotes, so a spreadsheet shows it as text.
fn csv_field(f: &str) -> String {
    if f.starts_with(FORMULA_START) {
        format!("\"'{}\"", f.replace('"', "\"\""))
    } else if f.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_owned()
    }
}

/// The alarms as CSV, with the columns time, severity, source, message, concerns and
/// acknowledged. Times are UTC in RFC 3339, as in the as-played export.
pub(crate) fn csv(rows: &[AlarmView]) -> String {
    let mut out = String::from("time,severity,source,message,concerns,acknowledged\n");
    for a in rows {
        let fields = [
            a.at.timestamp().to_string(),
            a.severity.label().to_owned(),
            a.source.label().to_owned(),
            a.text.clone(),
            a.concerns.clone().unwrap_or_default(),
            a.acked.to_string(),
        ];
        let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::SignedDuration;

    fn t(s: &str) -> Timestamp {
        s.parse().expect("a timestamp")
    }

    fn fit(playlist: PlaylistId, target: &str, error_ms: i64) -> FitResult {
        FitResult {
            playlist,
            target: t(target),
            planned: t(target),
            error: SignedDuration::from_millis(error_ms),
            within_tolerance: false,
            steps: Vec::new(),
        }
    }

    /// The plan runs again every second, with new numbers in its warnings. Each warning
    /// keeps its id, so an acknowledgement sticks.
    #[test]
    fn a_recomputed_plan_warning_keeps_its_id() {
        let now = t("2026-11-21T14:00:00Z");
        let later = t("2026-11-21T14:00:01Z");
        let news = PlaylistId(3);
        let late = |ms| Warning::OutsideTolerance {
            playlist: news,
            error: SignedDuration::from_millis(ms),
        };
        let fits_now = [fit(news, "2026-11-21T15:00:00Z", 1800)];
        let fits_later = [fit(news, "2026-11-21T15:00:00Z", 2400)];
        assert_eq!(
            warning_id(&late(1800), now, &fits_now),
            warning_id(&late(2400), later, &fits_later),
            "a late fixed item keeps its id when its error changes"
        );
        // Two late fixed items of one playlist are two conditions.
        let next_hour = [fit(news, "2026-11-21T16:00:00Z", 1800)];
        assert_ne!(
            warning_id(&late(1800), now, &fits_now),
            warning_id(&late(1800), now, &next_hour)
        );
        let conflict = |got| Warning::FixedConflict {
            playlist: news,
            wanted: t("2026-11-21T15:00:00Z"),
            got: t(got),
        };
        assert_eq!(
            warning_id(&conflict("2026-11-21T15:00:02Z"), now, &[]),
            warning_id(&conflict("2026-11-21T15:00:03Z"), later, &[])
        );
        let other = Warning::OutsideTolerance {
            playlist: PlaylistId(4),
            error: SignedDuration::from_millis(1800),
        };
        assert_ne!(
            warning_id(&late(1800), now, &fits_now),
            warning_id(
                &other,
                now,
                &[fit(PlaylistId(4), "2026-11-21T15:00:00Z", 1800)]
            )
        );
    }

    /// A real plan, made again one second later as playout does it: the item on air stays
    /// pinned. The dead air after it keeps its id.
    #[test]
    fn the_warnings_of_a_plan_made_again_keep_their_ids() {
        use radio_model::{Item, OnInterrupt, PickOrder, Playlist, Source};
        use radio_scheduler::{PlanInput, PlanOptions, PlayHistory, plan};
        let item = Item {
            id: ItemId(1),
            meta: radio_model::ItemMeta::default(),
            title: "Only Song".into(),
            artist: None,
            source: Source::Stream {
                url: "http://example.invalid/s".into(),
                keep_alive: false,
            },
            timing: None,
            planned_len: Some(std::time::Duration::from_secs(180)),
            movable: true,
            trimmable: true,
            locked: false,
            valid_from: None,
            valid_to: None,
            // It airs once, so the plan runs out after it: dead air follows.
            max_per_day: Some(1),
        };
        let station = Station {
            tz: "UTC".into(),
            playlists: vec![Playlist {
                id: PlaylistId(1),
                name: "Main".into(),
                color: [0x3b, 0x82, 0xf6],
                enabled: true,
                is_main: true,
                is_fill_pool: false,
                role: radio_model::PlaylistRole::User,
                on_interrupt: OnInterrupt::Resume,
                priority: 0,
                pick: PickOrder::InOrder,
                items: vec![item],
                rules: vec![],
                constraints: vec![],
                source: None,
            }],
        };
        let history = PlayHistory::new();
        let run = |now: Timestamp, pinned: &[radio_scheduler::Entry]| {
            plan(
                &PlanInput {
                    station: &station,
                    history: &history,
                    now,
                    horizon: std::time::Duration::from_secs(3600),
                    pinned,
                },
                &PlanOptions::default(),
                None,
            )
        };
        let now = t("2026-11-21T14:00:00Z");
        let first = run(now, &[]);
        let on_air: Vec<_> = first.entries.iter().take(1).cloned().collect();
        let later = now
            .checked_add(SignedDuration::from_secs(1))
            .expect("a time");
        let again = run(later, &on_air);
        let ids = |tl: &radio_scheduler::Timeline, at| {
            let mut v: Vec<u64> = tl
                .warnings
                .iter()
                .map(|w| warning_id(w, at, &tl.fits))
                .collect();
            v.sort_unstable();
            v
        };
        assert!(
            !first.warnings.is_empty(),
            "the plan has warnings: {first:?}"
        );
        assert_eq!(ids(&first, now), ids(&again, later));
    }

    /// A message that starts like a formula comes out as quoted text. An ordinary field stays.
    #[test]
    fn csv_neutralises_fields_that_start_like_a_formula() {
        assert_eq!(
            csv_field("=HYPERLINK(\"http://x\",\"y\")"),
            "\"'=HYPERLINK(\"\"http://x\"\",\"\"y\"\")\""
        );
        assert_eq!(csv_field("@SUM(A1:A9)"), "\"'@SUM(A1:A9)\"");
        assert_eq!(csv_field("+1+1"), "\"'+1+1\"");
        assert_eq!(csv_field("-2"), "\"'-2\"");
        assert_eq!(csv_field("\tx"), "\"'\tx\"");
        assert_eq!(csv_field("Dead air at 14:03:00"), "Dead air at 14:03:00");
        // Every column gets the same treatment, the concerns column too.
        let mut a = from_state(
            "watch",
            "",
            Severity::Warning,
            AlarmSource::Library,
            "=cmd|' /C calc'!A0".to_owned(),
            t("2026-11-21T14:00:00Z").to_zoned(TimeZone::UTC),
        );
        a.concerns = Some("@evil".to_owned());
        let out = csv(&[a]);
        let row = out.lines().nth(1).expect("a row");
        assert!(row.contains(",\"'=cmd|' /C calc'!A0\","), "{row}");
        assert!(row.ends_with(",\"'@evil\",false"), "{row}");
    }

    #[test]
    fn csv_quotes_commas_quotes_and_line_breaks() {
        let a = from_state(
            "persist",
            "",
            Severity::Alarm,
            AlarmSource::Engine,
            "disk full, \"retry\"\nlater".to_owned(),
            t("2026-11-21T14:00:00Z").to_zoned(TimeZone::UTC),
        );
        let out = csv(&[a]);
        assert!(out.starts_with("time,severity,source,message,concerns,acknowledged\n"));
        assert!(out.contains("\"disk full, \"\"retry\"\"\nlater\""));
    }
}
