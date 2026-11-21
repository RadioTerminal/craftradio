//! Decks and meters of the mock.

use super::*;

impl MockSource {
    /// End the item on air at `at`, so the next item starts then.
    pub(super) fn skip_on_air(&mut self, at: Timestamp) -> bool {
        let rows = self.current();
        let Some(r) = rows
            .iter()
            .find(|r| r.start.timestamp() <= at && at < end_of(r))
        else {
            return false;
        };
        let op = Op::Skip { id: r.id, at };
        self.paused_at = None;
        self.try_op(op, String::new()).is_ok()
    }

    pub(super) fn deck_command(
        &mut self,
        deck: usize,
        command: DeckCommand,
    ) -> Result<String, String> {
        let name = ['A', 'B', 'C', 'D'].get(deck).copied().unwrap_or('?');
        if deck != 0 {
            return Err(format!(
                "preview — deck {name} is not on air · {NEEDS_ENGINE}"
            ));
        }
        let now = self.last_now;
        match command {
            DeckCommand::PlayPause => {
                if self.paused_at.take().is_some() {
                    Ok("preview — deck A plays again; the timeline kept time".to_owned())
                } else {
                    self.paused_at = Some(now);
                    Ok("preview — deck A paused here only; the timeline keeps time".to_owned())
                }
            }
            DeckCommand::Fade(secs) => {
                self.fade = Some((now, secs));
                Ok(format!(
                    "preview — deck A fades out over {secs:.1} s, then the next item starts"
                ))
            }
            DeckCommand::Skip => {
                self.fade = None;
                if self.skip_on_air(now) {
                    Ok("preview — skipped to the next item (not saved)".to_owned())
                } else {
                    Err("preview — nothing to skip".to_owned())
                }
            }
        }
    }
}

pub(super) fn deck(
    label: char,
    row: Option<&Row>,
    state: DeckState,
    elapsed: Duration,
) -> DeckView {
    match row {
        Some(r) => DeckView {
            label,
            title: r.title.clone(),
            elapsed,
            len: r.len,
            state,
        },
        None => DeckView {
            label,
            title: String::new(),
            elapsed: Duration::ZERO,
            len: Duration::ZERO,
            state: DeckState::Empty,
        },
    }
}

/// Simulated meter levels. Only the playing deck and the output move.
pub(super) fn meters(now: Timestamp, decks: &[DeckView; 4]) -> Vec<MeterView> {
    let t = now.as_millisecond() as f64 / 1000.0;
    let wave = |ch: f64| {
        let l = -14.0 + 5.0 * (t * 1.3 + ch * 0.7).sin() + 3.0 * (t * 3.1 + ch).sin();
        let peak = (l + 2.5 + 1.5 * (t * 0.5 + ch).sin().abs()).min(0.0);
        (l as f32, peak as f32)
    };
    let silent = |label: String| MeterView {
        label,
        level_db: [SILENCE_DB; 2],
        peak_db: [SILENCE_DB; 2],
        gain_db: 0.0,
    };
    let live = |label: String, offset: f64| {
        let (l0, p0) = wave(offset);
        let (l1, p1) = wave(offset + 0.4);
        MeterView {
            label,
            level_db: [l0, l1],
            peak_db: [p0, p1],
            gain_db: 0.0,
        }
    };
    let mut out: Vec<MeterView> = decks
        .iter()
        .map(|d| {
            let label = d.label.to_string();
            if d.state == DeckState::Playing {
                live(label, 0.0)
            } else {
                silent(label)
            }
        })
        .collect();
    out.push(silent("Cart".to_owned()));
    out.push(silent("Live".to_owned()));
    let playing = decks.iter().any(|d| d.state == DeckState::Playing);
    out.push(if playing {
        live("Out".to_owned(), 0.1)
    } else {
        silent("Out".to_owned())
    });
    out
}
