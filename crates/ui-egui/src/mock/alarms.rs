//! Sample alarms of the preview: a varied set, at times before the snapshot.

use std::collections::HashSet;

use super::*;
use crate::view::{AlarmSource, AlarmView, Severity};

/// The ids of the sample alarms start here.
const FIRST_ID: u64 = 0xA1A0_0000;

/// A sample alarm. The last field is the index of the row that it concerns.
type Sample = (i64, Severity, AlarmSource, &'static str, Option<usize>);

const SAMPLES: [Sample; 9] = [
    (
        3,
        Severity::Alarm,
        AlarmSource::Playout,
        "Dead air: the program went silent for 4.2 s, so playout skipped to the next item",
        None,
    ),
    (
        9,
        Severity::Warning,
        AlarmSource::Playout,
        "Could not load the item: the file is missing. The plan skips it before air",
        Some(2),
    ),
    (
        14,
        Severity::Warning,
        AlarmSource::Scheduler,
        "The fixed item wants 15:00:00 but starts at 15:00:04",
        Some(4),
    ),
    (
        21,
        Severity::Alarm,
        AlarmSource::Icecast,
        "Icecast: the server refused the connection; it tries again",
        None,
    ),
    (
        26,
        Severity::Warning,
        AlarmSource::Device,
        "Output device lost: USB Audio Interface. Software pacing keeps the stream running",
        None,
    ),
    (
        33,
        Severity::Warning,
        AlarmSource::Library,
        "Cannot read /Volumes/Missing Drive/Music: no such folder",
        None,
    ),
    (
        41,
        Severity::Info,
        AlarmSource::Engine,
        "The wall clock stepped by 120 ms; the timing re-anchored",
        None,
    ),
    (
        48,
        Severity::Info,
        AlarmSource::Script,
        "The rules script of Jingles ran past its time limit; the last rules stay",
        None,
    ),
    (
        55,
        Severity::Warning,
        AlarmSource::Scheduler,
        "No eligible item in Ads at the top of the hour",
        Some(7),
    ),
];

/// The sample alarms at `now`. The rows of the snapshot give the items that they concern.
pub(super) fn sample(now: &Zoned, rows: &[Row], acked: &HashSet<u64>) -> Vec<AlarmView> {
    (FIRST_ID..)
        .zip(SAMPLES)
        .map(|(id, (minutes, severity, source, text, row))| {
            let row = row.and_then(|i| rows.get(i));
            AlarmView {
                id,
                at: now
                    .checked_sub(SignedDuration::from_mins(minutes))
                    .unwrap_or_else(|_| now.clone()),
                severity,
                source,
                text: text.to_owned(),
                item: row.map(|r| r.id),
                playlist: None,
                concerns: row.map(|r| r.title.clone()),
                acked: acked.contains(&id),
            }
        })
        .collect()
}

impl MockSource {
    /// Acknowledge sample alarms. It returns the number of new acknowledgements.
    pub(super) fn ack_alarms(&mut self, ids: &[u64]) -> Result<usize, String> {
        Ok(ids
            .iter()
            .filter(|id| self.acked_alarms.insert(**id))
            .count())
    }
}
