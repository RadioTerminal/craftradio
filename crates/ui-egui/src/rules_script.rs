//! Write a playlist's rules as a declarative JavaScript script.
//!
//! A rule is a script. The visual editor is only a way to write that script.

use std::fmt::Write as _;

use crate::view::{Placement, PlaylistTab, RuleView, Trigger};

/// The rules of `tab` as JavaScript: one `pick(..)` call, then one `rule(..)` call per rule.
pub fn generate(tab: &PlaylistTab) -> String {
    // A name with a line break would end the comment early.
    let name = tab.name.replace(['\n', '\r'], " ");
    let mut out = format!("// {name} — rules\npick(\"{}\");\n", tab.order.key());
    for rule in &tab.visual {
        out.push_str(&rule_call(rule));
    }
    out
}

/// Update the words and the script of `tab` from its visual rows.
/// This keeps a hand-edited script.
pub fn refresh(tab: &mut PlaylistTab) {
    tab.pick = tab.order.label().to_owned();
    tab.rules = tab.visual.iter().map(RuleView::describe).collect();
    if !tab.script_by_hand {
        tab.script = generate(tab);
    }
}

fn rule_call(rule: &RuleView) -> String {
    let when = match rule.trigger {
        Trigger::EveryTracks => format!("every: {{ tracks: {} }}", rule.tracks),
        Trigger::AtMinutes => {
            let m: Vec<String> = rule.minutes.iter().map(u32::to_string).collect();
            format!("at: {{ minutes: [{}] }}", m.join(", "))
        }
        Trigger::Daily => format!(
            "at: {{ time: \"{:02}:{:02}\", repeat: \"daily\" }}",
            rule.time.0, rule.time.1
        ),
    };
    let place = match rule.place {
        Placement::Between => "\"between\"".to_owned(),
        Placement::Under => "\"under\"".to_owned(),
        Placement::FixedHard | Placement::FixedSoft => {
            let kind = if rule.place == Placement::FixedHard {
                "hard"
            } else {
                "soft"
            };
            format!(
                "{{ fixed: \"{kind}\", tolerance: {:.1} }}",
                rule.tolerance_s
            )
        }
    };
    let mut call = format!("rule({{ {when}, place: {place}, take: {}", rule.take);
    if !rule.constraints.is_empty() {
        let list: Vec<String> = rule
            .constraints
            .iter()
            .map(|c| {
                if c.kind.has_value() {
                    format!("{}({})", c.kind.key(), c.n)
                } else {
                    format!("{}()", c.kind.key())
                }
            })
            .collect();
        let _ = write!(call, ",\n       constraints: [{}]", list.join(", "));
    }
    call.push_str(" });\n");
    call
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Constraint, ConstraintKind, Pick};

    #[test]
    fn the_script_has_one_rule_call_per_visual_rule() {
        let mut minutes = RuleView::new();
        minutes.trigger = Trigger::AtMinutes;
        minutes.minutes = vec![20, 50];
        minutes.constraints = vec![
            Constraint {
                kind: ConstraintKind::NotAdjacentToSelf,
                n: 0,
            },
            Constraint {
                kind: ConstraintKind::MaxPerDay,
                n: 24,
            },
        ];
        let mut daily = RuleView::new();
        daily.trigger = Trigger::Daily;
        daily.place = Placement::FixedHard;
        let visual = vec![minutes, RuleView::new(), daily];
        let tab = PlaylistTab {
            name: "Ads".to_owned(),
            color: [0, 0, 0],
            pick: String::new(),
            rules: Vec::new(),
            items: Vec::new(),
            order: Pick::Shuffle,
            visual,
            script: String::new(),
            script_by_hand: false,
            source: None,
            enabled: true,
            weights: Vec::new(),
        };

        let script = generate(&tab);
        assert_eq!(script.matches("rule(").count(), tab.visual.len());
        assert_eq!(script.matches("pick(").count(), 1);
        assert!(
            script.contains("maxPerDay(24)"),
            "constraints carry their numbers:\n{script}"
        );
    }
}
