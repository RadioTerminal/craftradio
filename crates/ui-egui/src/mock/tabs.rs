//! The playlists of the mock and their rules.

use super::*;

pub(super) fn rule(
    trigger: Trigger,
    place: Placement,
    take: u32,
    constraints: &[Constraint],
) -> RuleView {
    RuleView {
        trigger,
        place,
        take,
        constraints: constraints.to_vec(),
        ..RuleView::new()
    }
}

pub(super) const fn limit(kind: ConstraintKind, n: u32) -> Constraint {
    Constraint { kind, n }
}

pub(super) fn playlist(
    name: &str,
    color: [u8; 3],
    order: Pick,
    visual: Vec<RuleView>,
    items: Vec<(String, Duration)>,
) -> PlaylistTab {
    let mut tab = PlaylistTab {
        name: name.to_owned(),
        color,
        pick: String::new(),
        rules: Vec::new(),
        items,
        order,
        visual,
        script: String::new(),
        script_by_hand: false,
        enabled: true,
        weights: Vec::new(),
        ids: Vec::new(),
        source: None,
    };
    tab.weights = vec![1; tab.items.len()];
    rules_script::refresh(&mut tab);
    tab
}

pub(super) fn main_tab() -> PlaylistTab {
    let every = RuleView {
        tracks: 1,
        ..rule(
            Trigger::EveryTracks,
            Placement::Between,
            1,
            &[
                limit(ConstraintKind::ArtistSeparation, 3),
                limit(ConstraintKind::NotAdjacentToSelf, 0),
            ],
        )
    };
    playlist("Music", MUSIC, Pick::Shuffle, vec![every], Vec::new())
}

pub(super) fn tabs() -> Vec<PlaylistTab> {
    use ConstraintKind as K;
    use Placement as P;
    use Trigger as T;
    let jingles = rule(
        T::EveryTracks,
        P::Between,
        1,
        &[limit(K::NotAdjacentToSelf, 0)],
    );
    let ads = RuleView {
        minutes: vec![20, 50],
        ..rule(
            T::AtMinutes,
            P::Between,
            3,
            &[limit(K::NeverTrimmed, 0), limit(K::MaxPerDay, 24)],
        )
    };
    let news = RuleView {
        minutes: vec![0],
        tolerance_s: 0.5,
        ..rule(T::AtMinutes, P::FixedHard, 1, &[limit(K::CutMainToFit, 0)])
    };
    let shows = RuleView {
        time: (16, 0),
        ..rule(T::Daily, P::FixedHard, 1, &[limit(K::CutMainToFit, 0)])
    };
    let beds = RuleView {
        tracks: 1,
        ..rule(T::EveryTracks, P::Under, 1, &[])
    };
    vec![
        playlist(
            "Jingles",
            JINGLES,
            Pick::LeastRecentlyPlayed,
            vec![jingles],
            (1..=3)
                .map(|n| (format!("Station ID {n}"), secs(6)))
                .collect(),
        ),
        playlist(
            "Ads",
            ADS,
            Pick::InOrder,
            vec![ads],
            AD_NAMES
                .iter()
                .map(|n| ((*n).to_owned(), secs(30)))
                .collect(),
        ),
        playlist(
            "News",
            NEWS,
            Pick::InOrder,
            vec![news],
            vec![("News".to_owned(), secs(5 * 60))],
        ),
        playlist(
            "Shows",
            SHOWS,
            Pick::InOrder,
            vec![shows],
            vec![("Jazz Hour.mp3".to_owned(), secs(58 * 60 + 40))],
        ),
        playlist(
            "Beds",
            BEDS,
            Pick::Shuffle,
            vec![beds],
            vec![
                ("Bed 1 · Warm Pad".to_owned(), secs(120)),
                ("Bed 2 · Soft Pulse".to_owned(), secs(95)),
                ("Bed 3 · Night Drive".to_owned(), secs(140)),
            ],
        ),
    ]
}
