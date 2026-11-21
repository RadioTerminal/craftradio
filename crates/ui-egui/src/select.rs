//! Row selection for every list. A click selects a row. Command adds or removes one row.
//! Shift selects a range. Command+A selects every row on show.

use crate::app::step;

/// Select after a click on row `id`. Command adds or removes one row. Shift selects the
/// range of `order` from the anchor. A plain click selects the row alone and returns true.
pub(crate) fn click(
    ids: &mut Vec<u64>,
    anchor: &mut Option<u64>,
    id: u64,
    m: egui::Modifiers,
    order: &[u64],
) -> bool {
    if m.command {
        if let Some(i) = ids.iter().position(|s| *s == id) {
            ids.remove(i);
        } else {
            ids.push(id);
        }
        *anchor = Some(id);
        return false;
    }
    if m.shift {
        let a = anchor.and_then(|a| order.iter().position(|o| *o == a));
        let b = order.iter().position(|o| *o == id);
        if let (Some(a), Some(b)) = (a, b) {
            *ids = order[a.min(b)..=a.max(b)].to_vec();
            return false;
        }
    }
    *ids = vec![id];
    *anchor = Some(id);
    true
}

/// The selected rows of one list, and the anchor of a Shift range.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub ids: Vec<u64>,
    pub anchor: Option<u64>,
}

impl Selection {
    pub fn contains(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    /// Select after a click on row `id`, with the modifiers of the click.
    pub fn click(&mut self, id: u64, m: egui::Modifiers, order: &[u64]) {
        click(&mut self.ids, &mut self.anchor, id, m, order);
    }

    /// Select every row of `order`, the rows on show.
    pub fn all(&mut self, order: &[u64]) {
        order.clone_into(&mut self.ids);
        self.anchor = order.first().copied();
    }

    /// Move the selection `by` rows for the arrow keys. It returns the index of the new row.
    pub fn step(&mut self, order: &[u64], by: i32) -> Option<usize> {
        let id = step(order, self.ids.last().copied(), by)?;
        self.ids = vec![id];
        self.anchor = Some(id);
        order.iter().position(|o| *o == id)
    }

    /// The rows that a menu on row `id` acts on: the selection when it holds `id`, else `id`.
    pub fn targets(&self, id: u64) -> Vec<u64> {
        if self.contains(id) {
            self.ids.clone()
        } else {
            vec![id]
        }
    }

    /// The selected rows in the order of `order`. Rows that are not on show drop out.
    pub fn in_order(&self, order: &[u64]) -> Vec<u64> {
        order
            .iter()
            .filter(|id| self.ids.contains(id))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;
    use egui::Modifiers;

    const ORDER: [u64; 5] = [10, 20, 30, 40, 50];

    #[test]
    fn shift_selects_a_range_from_the_anchor_and_command_toggles_one_row() {
        let mut s = Selection::default();
        s.click(20, Modifiers::NONE, &ORDER);
        s.click(40, Modifiers::SHIFT, &ORDER);
        assert_eq!(s.in_order(&ORDER), vec![20, 30, 40]);
        s.click(30, Modifiers::COMMAND, &ORDER);
        assert_eq!(s.in_order(&ORDER), vec![20, 40]);
        s.click(50, Modifiers::NONE, &ORDER);
        assert_eq!(s.ids, vec![50]);
    }

    #[test]
    fn select_all_takes_only_the_rows_on_show() {
        let mut s = Selection::default();
        s.click(99, Modifiers::NONE, &[99]);
        s.all(&ORDER);
        assert_eq!(s.in_order(&ORDER), ORDER.to_vec());
        assert!(!s.contains(99), "a row that is not on show drops out");
    }
}
