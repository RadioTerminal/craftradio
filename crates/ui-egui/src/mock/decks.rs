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
        self.try_op(op).is_ok()
    }

    pub(super) fn deck_command(
        &mut self,
        deck: usize,
        command: DeckCommand,
    ) -> Result<usize, String> {
        let name = ['A', 'B', 'C', 'D'].get(deck).copied().unwrap_or('?');
        if deck != 0 {
            return Err(format!(
                "preview — deck {name} is not on air · {NEEDS_ENGINE}"
            ));
        }
        let now = self.last_now;
        match command {
            DeckCommand::PlayPause => {
                if self.paused_at.take().is_none() {
                    self.paused_at = Some(now);
                }
                Ok(1)
            }
            DeckCommand::Fade(secs) => {
                self.fade = Some((now, secs));
                Ok(1)
            }
            DeckCommand::Skip => {
                self.fade = None;
                if self.skip_on_air(now) {
                    Ok(1)
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
            start: Some(r.start.clone()),
            playlist: r.playlist.clone(),
            playlist_color: r.color,
            hard: r.fixed.is_some() && r.playlist == "News",
            waiting: false,
            item: None,
        },
        None => DeckView {
            label,
            title: String::new(),
            elapsed: Duration::ZERO,
            len: Duration::ZERO,
            state: DeckState::Empty,
            start: None,
            playlist: String::new(),
            playlist_color: [0x80; 3],
            hard: false,
            waiting: false,
            item: None,
        },
    }
}

/// Simulated meter levels. Only the playing deck and the output move.
pub(super) fn meters(
    now: Timestamp,
    decks: &[DeckView; 4],
    strips: &HashMap<String, Strip>,
) -> Vec<MeterView> {
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
        mute: false,
        pfl: false,
        route: Route::Out,
    };
    let live = |label: String, offset: f64| {
        let (l0, p0) = wave(offset);
        let (l1, p1) = wave(offset + 0.4);
        MeterView {
            label,
            level_db: [l0, l1],
            peak_db: [p0, p1],
            gain_db: 0.0,
            mute: false,
            pfl: false,
            route: Route::Out,
        }
    };
    let mut out: Vec<MeterView> = decks
        .iter()
        .map(|d| {
            let label = d.label.to_string();
            if matches!(d.state, DeckState::Playing | DeckState::Fading) {
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
    for m in &mut out {
        let st = strips.get(&m.label).copied().unwrap_or_default();
        m.gain_db = st.gain_db;
        m.mute = st.mute;
        m.pfl = st.pfl;
        m.route = st.route;
        if st.mute {
            m.level_db = [SILENCE_DB; 2];
            m.peak_db = [SILENCE_DB; 2];
        }
    }
    out
}
