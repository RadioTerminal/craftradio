//! Decks and meters of the mock.

use super::*;
use radio_model::{GainSource, ItemGain, Loudness};

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
        let now = self.last_now;
        // Deck A holds the row on air and B to D the next rows. Playing deck B to D skips
        // the rows before it.
        if let DeckCommand::Play(_) = command {
            self.fade = None;
            let played = (0..deck).all(|_| self.skip_on_air(now));
            return if deck == 0 || played {
                Ok(1)
            } else {
                Err(format!("preview — deck {name} has nothing to play"))
            };
        }
        if deck != 0 {
            return Err(format!(
                "preview — deck {name} is not on air · {NEEDS_ENGINE}"
            ));
        }
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
            DeckCommand::Play(_) => Ok(1),
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
            gain: None,
            cover: None,
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
            gain: None,
            cover: None,
        },
    }
}

/// A sample item gain for deck `i` of the preview: a measured cut, a tag, a clamped
/// boost and a file without loudness. The preview plays nothing, so the deck tooltip
/// says so.
pub(super) fn sample_gain(i: usize, settings: &radio_model::LoudnessSettings) -> ItemGain {
    let known = match i {
        0 => Some((
            Loudness {
                lufs: -14.8,
                peak_db: Some(-0.3),
            },
            GainSource::Measured,
        )),
        1 => Some((
            Loudness {
                lufs: -15.9,
                peak_db: Some(-1.2),
            },
            GainSource::Tag,
        )),
        2 => Some((
            Loudness {
                lufs: -27.0,
                peak_db: Some(-4.5),
            },
            GainSource::Measured,
        )),
        _ => None,
    };
    settings.gain(known, 0.0)
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
    let silent = |strip: String| MeterView {
        label: strip_label(&strip),
        strip,
        level_db: [SILENCE_DB; 2],
        peak_db: [SILENCE_DB; 2],
        gain_db: 0.0,
        mute: false,
        snd: false,
    };
    let live = |strip: String, offset: f64| {
        let (l0, p0) = wave(offset);
        let (l1, p1) = wave(offset + 0.4);
        MeterView {
            label: strip_label(&strip),
            strip,
            level_db: [l0, l1],
            peak_db: [p0, p1],
            gain_db: 0.0,
            mute: false,
            snd: false,
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
        let st = strips.get(&m.strip).copied().unwrap_or_default();
        m.gain_db = st.gain_db;
        m.mute = st.mute;
        m.snd = st.snd;
        if st.mute {
            m.level_db = [SILENCE_DB; 2];
            m.peak_db = [SILENCE_DB; 2];
        }
    }
    out
}

/// The name on a strip: Out shows as Program.
fn strip_label(strip: &str) -> String {
    if strip == crate::view::PROGRAM_STRIP {
        "Program".to_owned()
    } else {
        strip.to_owned()
    }
}

/// The output strips of the preview: it plays nothing, so no output is live.
/// The output strips of the preview. Off air, Icecast and WebRTC say so, as the station's do.
pub(super) fn output_meters(on_air: bool) -> Vec<crate::view::OutputMeterView> {
    use crate::view::{OutputKind, OutputMeterView};
    let air = |note| if on_air { note } else { "Off air" };
    [
        (OutputKind::Device, "Sound card", "Preview — no audio"),
        (OutputKind::Icecast, "Icecast", air("Not connected")),
        (OutputKind::WebRtc, "WebRTC", air("Off")),
    ]
    .into_iter()
    .map(|(kind, label, note)| OutputMeterView {
        kind,
        label: label.to_owned(),
        live: false,
        note: note.to_owned(),
        level_db: [SILENCE_DB; 2],
        peak_db: [SILENCE_DB; 2],
    })
    .collect()
}
