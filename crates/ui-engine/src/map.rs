//! Conversions from the engine and model types to the view-model of the UI.

use lucide_icons::Icon;
use std::time::Duration;

use radio_engine::State;
use radio_model::{
    Constraint as MConstraint, FixMode, Item, PickOrder, Placement as MPlacement, Playlist,
    PublishMode as MPublish, Repeat, Rule, Source, Trigger as MTrigger,
};
use radio_ui_egui::rules_script;
use radio_ui_egui::view::{
    Constraint, ConstraintKind, FolderSource, ItemMeta, Pick, Placement, PlaylistTab, PublishMode,
    RuleView, Trigger,
};

/// The planned length of an item: its cue points, or its planned length, or zero.
pub(crate) fn item_len(item: &Item) -> Duration {
    match (&item.timing, item.planned_len) {
        (Some(t), _) => t.cue_out.saturating_sub(t.cue_in),
        (None, Some(len)) => len,
        (None, None) => Duration::ZERO,
    }
}

/// "Artist – Title", or the title alone.
pub(crate) fn display_title(item: &Item) -> String {
    match item.artist.as_deref().filter(|a| !a.is_empty()) {
        Some(a) => format!("{a} – {}", item.title),
        None => item.title.clone(),
    }
}

/// The tags of an item, with the metadata from `item.set_meta` first. An empty title or
/// artist takes the value that the timeline shows.
pub(crate) fn effective_meta(
    item: &Item,
    stored: Option<&serde_json::Value>,
) -> radio_model::ItemMeta {
    let mut m = stored
        .and_then(|v| serde_json::from_value::<radio_model::ItemMeta>(v.clone()).ok())
        .unwrap_or_else(|| item.meta.clone());
    if m.title.is_empty() {
        m.title.clone_from(&item.title);
    }
    if m.artist.is_empty()
        && let Some(a) = &item.artist
    {
        m.artist.clone_from(a);
    }
    m
}

/// The view tags of an item. `trim` is the gain trim of a station file, or `None` for an
/// item that takes no trim.
pub(crate) fn view_meta(item: &Item, m: &radio_model::ItemMeta, trim: Option<f32>) -> ItemMeta {
    ItemMeta {
        title: m.title.clone(),
        artist: m.artist.clone(),
        album: m.album.clone(),
        year: m.year.clone(),
        genre: m.genre.clone(),
        comment: m.comment.clone(),
        publish: match m.publish {
            MPublish::FromTags => PublishMode::FromTags,
            MPublish::Custom => PublishMode::Custom,
            MPublish::KeepPrevious => PublishMode::KeepPrevious,
            MPublish::Hide => PublishMode::Hide,
        },
        custom: m.custom.clone(),
        picture: m.picture.clone(),
        keep_alive: match item.source {
            Source::Stream { keep_alive, .. } => Some(keep_alive),
            _ => None,
        },
        gain_trim_db: match item.source {
            Source::File { .. } => trim,
            _ => None,
        },
    }
}

pub(crate) fn model_meta(m: &ItemMeta) -> radio_model::ItemMeta {
    radio_model::ItemMeta {
        title: m.title.clone(),
        artist: m.artist.clone(),
        album: m.album.clone(),
        year: m.year.clone(),
        genre: m.genre.clone(),
        comment: m.comment.clone(),
        publish: match m.publish {
            PublishMode::FromTags => MPublish::FromTags,
            PublishMode::Custom => MPublish::Custom,
            PublishMode::KeepPrevious => MPublish::KeepPrevious,
            PublishMode::Hide => MPublish::Hide,
        },
        custom: m.custom.clone(),
        picture: m.picture.clone(),
    }
}

fn pick_of(p: &PickOrder) -> Pick {
    match p {
        PickOrder::InOrder => Pick::InOrder,
        PickOrder::Shuffle { .. } => Pick::Shuffle,
        PickOrder::Weighted { .. } => Pick::Weighted,
        PickOrder::LeastRecentlyPlayed => Pick::LeastRecentlyPlayed,
    }
}

/// One model rule as a visual row, or `None` when the visual editor cannot show it.
fn rule_view(rule: &Rule) -> Option<RuleView> {
    let mut v = RuleView::new();
    match &rule.trigger {
        MTrigger::EveryNTracks(n) => {
            v.trigger = Trigger::EveryTracks;
            v.tracks = *n;
        }
        MTrigger::AtMinutes(m) => {
            v.trigger = Trigger::AtMinutes;
            v.minutes = m.iter().map(|m| u32::from(*m)).collect();
        }
        MTrigger::At {
            at,
            repeat: Repeat::Daily,
        } => {
            v.trigger = Trigger::Daily;
            v.time = (
                u32::try_from(at.hour()).ok()?,
                u32::try_from(at.minute()).ok()?,
            );
        }
        MTrigger::At { .. } | MTrigger::EveryNMinutes(_) | MTrigger::ScriptEvent(_) => {
            return None;
        }
    }
    match &rule.placement {
        MPlacement::BetweenTracks => v.place = Placement::Between,
        MPlacement::Fixed { mode, tolerance } => {
            v.place = match mode {
                FixMode::Hard => Placement::FixedHard,
                FixMode::Soft => Placement::FixedSoft,
            };
            v.tolerance_s = tolerance.as_secs_f32();
        }
    }
    v.take = u32::from(rule.take);
    for c in &rule.constraints {
        let (kind, n) = match c {
            MConstraint::NotAdjacentToSelf => (ConstraintKind::NotAdjacentToSelf, 0),
            MConstraint::MaxPerDay(n) => (ConstraintKind::MaxPerDay, *n),
            MConstraint::ArtistSeparation(d) => (
                ConstraintKind::ArtistSeparation,
                u32::try_from(d.as_secs() / 60).unwrap_or(u32::MAX),
            ),
            _ => return None,
        };
        v.constraints.push(Constraint { kind, n });
    }
    Some(v)
}

/// The folder that backs a playlist: the model field, or the source that the engine keeps.
fn folder_source(p: &Playlist, state: &State) -> Option<FolderSource> {
    if let Some(s) = &p.source {
        return Some(FolderSource {
            path: s.path.clone(),
            recursive: s.recursive,
            watch: s.watch,
        });
    }
    let v = state.source(p.id)?;
    let s: radio_model::FolderSource = serde_json::from_value(v.clone()).ok()?;
    Some(FolderSource {
        path: s.path,
        recursive: s.recursive,
        watch: s.watch,
    })
}

/// A playlist as a tab of the UI.
pub(crate) fn tab(p: &Playlist, state: &State) -> PlaylistTab {
    let visual: Option<Vec<RuleView>> = p.rules.iter().map(rule_view).collect();
    let weight_of = |id| match &p.pick {
        PickOrder::Weighted { weights } => weights
            .iter()
            .find(|(i, _)| *i == id)
            .map_or(1, |(_, w)| *w),
        _ => 1,
    };
    let mut tab = PlaylistTab {
        plays: p.plays,
        name: p.name.clone(),
        color: p.color,
        pick: String::new(),
        rules: Vec::new(),
        items: p
            .items
            .iter()
            .map(|i| {
                // A missing file shows as such; playout skips it until it is found again.
                let title = if state.missing.contains(&i.id) {
                    format!(
                        "{} missing · {}",
                        char::from(Icon::TriangleAlert),
                        display_title(i)
                    )
                } else {
                    display_title(i)
                };
                (title, item_len(i))
            })
            .collect(),
        order: pick_of(&p.pick),
        visual: visual.clone().unwrap_or_default(),
        script: String::new(),
        script_by_hand: false,
        source: folder_source(p, state),
        enabled: p.enabled,
        weights: p.items.iter().map(|i| weight_of(i.id)).collect(),
        ids: p.items.iter().map(|i| i.id.0).collect(),
    };
    rules_script::refresh(&mut tab);
    let stored = state.script(p.id).map(str::to_owned);
    match (visual, stored) {
        // The visual editor cannot show every rule, so the stored script is the truth.
        (None, Some(script)) => {
            tab.script = script;
            tab.script_by_hand = true;
        }
        (None, None) => {
            tab.script = format!(
                "// {} has rules that the visual editor cannot show.\n",
                p.name.replace(['\n', '\r'], " ")
            );
            tab.script_by_hand = true;
        }
        (Some(_), Some(script)) if script.trim() != tab.script.trim() => {
            tab.script = script;
            tab.script_by_hand = true;
        }
        (Some(_), _) => {}
    }
    // When the visual rows cannot show the rules, the words come from the model.
    if tab.visual.len() != p.rules.len() {
        tab.rules = p.rules.iter().map(describe_rule).collect();
    }
    tab
}

fn describe_rule(r: &Rule) -> String {
    let when = match &r.trigger {
        MTrigger::EveryNTracks(n) => format!("Every {n} tracks"),
        MTrigger::EveryNMinutes(n) => format!("Every {n} minutes"),
        MTrigger::AtMinutes(m) => {
            let m: Vec<String> = m.iter().map(|m| format!(":{m:02}")).collect();
            m.join(" and ")
        }
        MTrigger::At { at, repeat } => format!("{at} ({repeat:?})").to_lowercase(),
        MTrigger::ScriptEvent(e) => format!("on {e}"),
    };
    let place = match &r.placement {
        MPlacement::BetweenTracks => "between tracks",
        MPlacement::Fixed {
            mode: FixMode::Hard,
            ..
        } => "fixed (hard)",
        MPlacement::Fixed {
            mode: FixMode::Soft,
            ..
        } => "fixed (soft)",
    };
    format!("{when} · {place} · take {}", r.take)
}

/// An empty tab, for a station without a main playlist.
pub(crate) fn empty_tab(name: &str) -> PlaylistTab {
    let mut tab = PlaylistTab {
        plays: radio_model::PlayMode::InSequence,
        name: name.to_owned(),
        color: [0x80; 3],
        pick: String::new(),
        rules: Vec::new(),
        items: Vec::new(),
        order: Pick::InOrder,
        visual: Vec::new(),
        script: String::new(),
        script_by_hand: false,
        source: None,
        enabled: true,
        weights: Vec::new(),
        ids: Vec::new(),
    };
    rules_script::refresh(&mut tab);
    tab
}
