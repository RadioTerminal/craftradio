//! The generated timeline, the edits on it, and the fit to the fixed items.

use super::*;

/// One edit to the timeline.
#[derive(Clone)]
pub(super) enum Op {
    Insert {
        at: Place,
        id: u64,
        /// The library item behind the row, for its tags.
        item: u64,
        title: String,
        len: Duration,
    },
    Remove(u64),
    Move {
        id: u64,
        at: Place,
    },
    Lock(u64),
    /// End the row early at `at`, so the next item starts then.
    Skip {
        id: u64,
        at: Timestamp,
    },
}

impl MockSource {
    /// Apply the edits to `rows`. It returns false when the rows no longer fit the fixed items.
    pub(super) fn apply_ops(&self, rows: &mut Vec<Row>, ops: &[Op]) -> bool {
        for op in ops {
            match op {
                Op::Insert {
                    at,
                    id,
                    item,
                    title,
                    len,
                } => {
                    let Some(i) = index_of(rows, *at) else {
                        continue;
                    };
                    let Some(base) = rows.get(i.saturating_sub(1)).or(rows.first()).cloned() else {
                        continue;
                    };
                    rows.insert(
                        i,
                        Row {
                            id: *id,
                            playlist: self.main.name.clone(),
                            color: self.main.color,
                            title: title.clone(),
                            len: *len,
                            is_main: true,
                            fixed: None,
                            origin: Origin::Manual,
                            locked: false,
                            meta: self.library_item(*item).map_or_else(
                                || from_title(title, PublishMode::FromTags),
                                |l| l.meta.clone(),
                            ),
                            timing: None,
                            stream: None,
                            ..base
                        },
                    );
                }
                Op::Remove(id) => rows.retain(|r| r.id != *id),
                Op::Move { id, at } => {
                    let Some(from) = rows.iter().position(|r| r.id == *id) else {
                        continue;
                    };
                    let row = rows.remove(from);
                    match index_of(rows, *at) {
                        Some(i) => rows.insert(i, row),
                        None => rows.insert(from, row),
                    }
                }
                Op::Lock(id) => {
                    if let Some(r) = rows.iter_mut().find(|r| r.id == *id) {
                        r.locked = !r.locked;
                    }
                }
                Op::Skip { id, at } => {
                    let Some(i) = rows.iter().position(|r| r.id == *id) else {
                        continue;
                    };
                    let played = remaining(rows[i].start.timestamp(), *at);
                    if played.is_zero() || played >= rows[i].len {
                        continue;
                    }
                    rows[i].len = played;
                    let fixed = rows[i + 1..]
                        .iter()
                        .position(|r| r.fixed.is_some())
                        .map_or(rows.len(), |k| i + 1 + k);
                    let music = rows[i + 1..fixed].iter().any(|r| r.origin == Origin::Main);
                    if !music {
                        // Nothing can fill the gap before the fixed item. A filler track starts now.
                        let list = self.main_tracks.as_ref().unwrap_or(&self.tracks);
                        let pick = (*id as usize) % list.len().max(1);
                        if let Some((title, _)) = list.get(pick) {
                            let mut filler = rows[i].clone();
                            filler.id = id.wrapping_add(1) | MANUAL_ID;
                            filler.title.clone_from(title);
                            filler.meta = from_title(title, PublishMode::FromTags);
                            filler.timing = None;
                            filler.len = Duration::ZERO;
                            filler.origin = Origin::Main;
                            filler.is_main = true;
                            filler.fixed = None;
                            filler.locked = false;
                            filler.start = at.to_zoned(self.tz.clone());
                            rows.insert(i + 1, filler);
                        }
                    }
                }
            }
        }
        fit_all(rows, &self.tz)
    }

    /// The rows of the last snapshot window, with all edits.
    pub(super) fn current(&mut self) -> Vec<Row> {
        let Some(top) = self.last_top.clone() else {
            return Vec::new();
        };
        let mut rows = self.rows_from(&top).to_vec();
        let ops = self.ops.clone();
        self.apply_ops(&mut rows, &ops);
        rows
    }

    /// Keep `op` when the rows still fit with it.
    pub(super) fn try_op(&mut self, op: Op) -> Result<usize, String> {
        let Some(top) = self.last_top.clone() else {
            return Err("preview — no timeline yet".to_owned());
        };
        let mut rows = self.rows_from(&top).to_vec();
        let mut ops = self.ops.clone();
        ops.push(op);
        if self.apply_ops(&mut rows, &ops) {
            self.ops = ops;
            Ok(1)
        } else {
            Err("preview — that does not fit before the next fixed item".to_owned())
        }
    }

    pub(super) fn rows_from(&mut self, top: &Zoned) -> &[Row] {
        let key = top.timestamp();
        let stale = self.cache.as_ref().is_none_or(|(k, _)| *k != key);
        if stale {
            let mut rows = Vec::new();
            let mut hour = top
                .checked_sub(PAST_HOURS.hours())
                .unwrap_or_else(|_| top.clone());
            for _ in 0..PAST_HOURS + HOURS {
                let Ok(next) = hour.checked_add(1.hour()) else {
                    break;
                };
                self.fill_hour(&hour, &next, &mut rows);
                hour = next;
            }
            self.cache = Some((key, rows));
        }
        self.cache.as_ref().map_or(&[], |(_, rows)| rows.as_slice())
    }

    /// Fill one hour. A fixed item opens it, then music, jingles and ads run to the next hour.
    pub(super) fn fill_hour(&self, hour: &Zoned, next: &Zoned, rows: &mut Vec<Row>) {
        let end = next.timestamp();
        let mut t = hour.timestamp();
        let push = |rows: &mut Vec<Row>, t: &mut Timestamp, item: Item| {
            let start = t.to_zoned(self.tz.clone());
            let Ok(span) = SignedDuration::try_from(item.len) else {
                return;
            };
            let main = item.playlist == "Music";
            *t += span;
            rows.push(Row {
                id: start.timestamp().as_millisecond().unsigned_abs(),
                start,
                playlist: if main {
                    self.main.name.clone()
                } else {
                    item.playlist.to_owned()
                },
                color: if main { self.main.color } else { item.color },
                len: item.len,
                is_main: main,
                fixed: item.fixed,
                origin: if main { Origin::Main } else { Origin::Rule },
                locked: false,
                meta: from_title(&item.title, publish_for(item.playlist)),
                timing: None,
                stream: None,
                title: item.title,
            });
        };

        let opener = if hour.hour() == SHOW_HOUR {
            Item::fixed("Shows", SHOWS, "Jazz Hour.mp3", secs(58 * 60 + 40), 0.8)
        } else {
            Item::fixed("News", NEWS, "News", secs(5 * 60), -0.4)
        };
        push(rows, &mut t, opener);

        let hour_no = hour.timestamp().as_second().div_euclid(3600);
        let mut track = (hour_no.rem_euclid(TRACKS as i64) as usize * 17) % TRACKS;
        let mut since_jingle = 0;
        let mut jingle_no = hour_no.rem_euclid(3) as usize;
        let mut breaks_done = [false; AD_MINUTES.len()];
        let mut ad_no = 0;
        let mut ads_due = false;

        while t < end {
            let left = remaining(t, end);
            if ads_due {
                ads_due = false;
                let ad_len = secs(30);
                if left >= ad_len * ADS_PER_BREAK as u32 {
                    for _ in 0..ADS_PER_BREAK {
                        let name = AD_NAMES[ad_no % AD_NAMES.len()];
                        let title = format!("Ad {} · {name}", ad_no % ADS_PER_BREAK + 1);
                        push(rows, &mut t, Item::new("Ads", ADS, title, ad_len));
                        ad_no += 1;
                    }
                }
                continue;
            }
            if since_jingle == TRACKS_PER_JINGLE {
                since_jingle = 0;
                let len = secs(6);
                if left >= len {
                    let title = format!("Station ID {}", jingle_no % 3 + 1);
                    push(rows, &mut t, Item::new("Jingles", JINGLES, title, len));
                    jingle_no += 1;
                }
                continue;
            }
            let list = self.main_tracks.as_ref().unwrap_or(&self.tracks);
            let Some((title, len)) = list.get(track % list.len().max(1)) else {
                break;
            };
            track += 1;
            since_jingle += 1;
            // The last track before the next hour ends early, so the fixed item starts on time.
            push(
                rows,
                &mut t,
                Item::new("Music", MUSIC, title.clone(), (*len).min(left)),
            );
            let minute = t.to_zoned(self.tz.clone()).minute();
            let same_hour = t < end;
            for (done, at) in breaks_done.iter_mut().zip(AD_MINUTES) {
                if same_hour && !*done && minute >= at {
                    *done = true;
                    ads_due = true;
                }
            }
        }
    }
}

pub(super) fn remaining(from: Timestamp, to: Timestamp) -> Duration {
    Duration::try_from(to.duration_since(from)).unwrap_or_default()
}

pub(super) struct Item {
    playlist: &'static str,
    color: [u8; 3],
    title: String,
    len: Duration,
    fixed: Option<FitView>,
}

impl Item {
    pub(super) fn new(
        playlist: &'static str,
        color: [u8; 3],
        title: String,
        len: Duration,
    ) -> Self {
        Self {
            playlist,
            color,
            title,
            len,
            fixed: None,
        }
    }

    pub(super) fn fixed(
        playlist: &'static str,
        color: [u8; 3],
        title: &str,
        len: Duration,
        err: f32,
    ) -> Self {
        Self {
            fixed: Some(FitView {
                error_s: err,
                ok: true,
            }),
            ..Self::new(playlist, color, title.to_owned(), len)
        }
    }
}

/// The end instant of a row.
pub(super) fn end_of(r: &Row) -> Timestamp {
    let start = r.start.timestamp();
    SignedDuration::try_from(r.len).map_or(start, |d| start + d)
}

/// Where a place is in `rows`, as an index to insert at.
pub(super) fn index_of(rows: &[Row], at: Place) -> Option<usize> {
    match at {
        Place::Before(id) => rows.iter().position(|r| r.id == id),
        Place::After(id) => rows.iter().position(|r| r.id == id).map(|i| i + 1),
        Place::End => Some(rows.len()),
    }
}

/// The mock may shorten or lengthen this row to fit: a main row that nobody locked.
/// The mock tries the last rows of an hour first, so past rows rarely change.
pub(super) fn adjustable(r: &Row) -> bool {
    r.origin == Origin::Main && !r.locked
}

/// Make every fixed row start on its time again. The mock cuts or lengthens future main rows,
/// then sets each start to the end of the row before. It returns false when a gap stays.
pub(super) fn fit_all(rows: &mut Vec<Row>, tz: &TimeZone) -> bool {
    let mut ok = true;
    let mut p = 0;
    while p < rows.len() {
        let next_fixed = |rows: &[Row]| {
            rows.iter()
                .skip(p + 1)
                .position(|r| r.fixed.is_some())
                .map(|k| p + 1 + k)
        };
        let Some(f) = next_fixed(rows) else {
            let n = rows.len();
            retime(rows, p, n, tz);
            break;
        };
        let target = rows[f].start.timestamp();
        let begin = end_of(&rows[p]);
        let have: Duration = rows[p + 1..f].iter().map(|r| r.len).sum();
        let want = Duration::try_from(target.duration_since(begin)).unwrap_or_default();
        if have > want {
            let mut owed = have - want;
            for r in rows[p + 1..f].iter_mut().rev() {
                if owed.is_zero() {
                    break;
                }
                if adjustable(r) {
                    let cut = owed.min(r.len);
                    r.len -= cut;
                    owed -= cut;
                }
            }
            ok &= owed.is_zero();
        } else if have < want {
            match rows[p + 1..f].iter_mut().rev().find(|r| adjustable(r)) {
                Some(r) => r.len += want - have,
                None => ok = false,
            }
        }
        let mut i = p + 1;
        while i < rows.len() && rows[i].fixed.is_none() {
            if rows[i].len.is_zero() {
                rows.remove(i);
            } else {
                i += 1;
            }
        }
        let f = next_fixed(rows).unwrap_or(rows.len());
        retime(rows, p, f, tz);
        p = f;
    }
    ok
}

/// Set the start of each row in `from + 1 .. to` to the end of the row before it.
pub(super) fn retime(rows: &mut [Row], from: usize, to: usize, tz: &TimeZone) {
    for j in from + 1..to.min(rows.len()) {
        let start = end_of(&rows[j - 1]);
        rows[j].start = start.to_zoned(tz.clone());
    }
}

/// A made-up length for a file that nobody decoded yet. The same name always gives the same length.
pub(super) fn estimated_len(name: &str) -> Duration {
    let hash = name
        .bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    secs(150 + hash % 151)
}
