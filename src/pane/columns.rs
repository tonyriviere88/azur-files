//! Which columns the details view shows, and in what order, left to right.
//!
//! **One answer for the window**, like [`crate::fs::Sort`]: how you like a listing laid out is a
//! habit, not a fact about a folder, so every pane draws the same arrangement and the settings file
//! keeps it as two lines — `columns=` for the order and `columns_hidden=` for what is taken off.
//!
//! A column being *hidden* is not the same as a column being *absent*. Status, Hard links and
//! Keywords are also left out of any folder they have nothing to say about — see
//! [`crate::ui::filelist::measure_columns`] — and that is decided per listing; this is the user's
//! choice on top of it. A hidden column costs nothing: its width resolves to zero, and the
//! background work that fills it in is never asked for — see [`crate::app`]'s `collect_cloud` and
//! `collect_hard_links`.
//!
//! **Name is always shown.** A row with no name is not a row anybody can act on, and it is the
//! column the others give way to; [`Columns::set_shown`] refuses it rather than trusting every
//! caller to.

use crate::fs::Column;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Columns {
    /// Left to right. Always every [`Column`] exactly once — see [`Self::set_order`], which is the
    /// only way anything outside this type writes it.
    order: [Column; Column::COUNT],
    /// Taken off from the header's right-click menu, indexed by [`Column::index`].
    hidden: [bool; Column::COUNT],
}

impl Default for Columns {
    /// Every column, in the order they were drawn before any of this could be changed.
    fn default() -> Self {
        Self {
            order: Column::ALL,
            hidden: [false; Column::COUNT],
        }
    }
}

impl Columns {
    /// Left to right, hidden ones included.
    pub fn order(&self) -> [Column; Column::COUNT] {
        self.order
    }

    pub fn shown(&self, column: Column) -> bool {
        !self.hidden[column.index()]
    }

    /// The columns taken off, in [`Column::ALL`] order.
    pub fn hidden(&self) -> impl Iterator<Item = Column> + '_ {
        Column::ALL.into_iter().filter(|column| !self.shown(*column))
    }

    /// Show a column or hide it. Whether anything changed, which is whether the settings need
    /// writing. Name cannot be hidden, and asking is not a change.
    pub fn set_shown(&mut self, column: Column, shown: bool) -> bool {
        if column == Column::Name {
            return false;
        }
        let was = self.shown(column);
        self.hidden[column.index()] = !shown;
        was != shown
    }

    /// Arrange the columns in `order`, which may name any of them in any number.
    ///
    /// The same rule as [`crate::ui::sidebar::Sections::set_order`]: a repeat is dropped, and a
    /// column the order **does not mention** — a file written before that column existed, or one
    /// edited by hand — goes back beside the one it follows in [`Column::ALL`].
    pub fn set_order(&mut self, order: impl IntoIterator<Item = Column>) {
        let mut list: Vec<Column> = Vec::with_capacity(Column::COUNT);
        for column in order {
            if !list.contains(&column) {
                list.push(column);
            }
        }
        for (at, column) in Column::ALL.into_iter().enumerate() {
            if list.contains(&column) {
                continue;
            }
            let after = Column::ALL[..at]
                .iter()
                .rev()
                .find_map(|before| list.iter().position(|had| had == before));
            list.insert(after.map_or(0, |index| index + 1), column);
        }
        self.order.copy_from_slice(&list);
    }

    /// Take `from` out and put it on one side of `to`. Whether anything moved.
    pub fn move_beside(&mut self, from: Column, to: Column, after: bool) -> bool {
        if from == to {
            return false;
        }
        let before = self.order;
        let mut list: Vec<Column> = self.order.into_iter().filter(|c| *c != from).collect();
        let Some(at) = list.iter().position(|c| *c == to) else {
            return false;
        };
        list.insert(at + usize::from(after), from);
        self.order.copy_from_slice(&list);
        self.order != before
    }

    /// Whether there is anything for [`Self::reset`] to undo.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Every column back, in the first-run order. Whether anything changed.
    pub fn reset(&mut self) -> bool {
        let changed = !self.is_default();
        *self = Self::default();
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Column::*;

    #[test]
    fn name_cannot_be_hidden() {
        let mut columns = Columns::default();
        assert!(!columns.set_shown(Name, false));
        assert!(columns.shown(Name));
        assert!(columns.set_shown(HardLinks, false));
        assert!(!columns.shown(HardLinks));
        // Already hidden: nothing changed, so nothing to save.
        assert!(!columns.set_shown(HardLinks, false));
        assert_eq!(columns.hidden().collect::<Vec<_>>(), [HardLinks]);
    }

    #[test]
    fn a_dragged_column_lands_beside_the_one_it_is_dropped_on() {
        let mut columns = Columns::default();
        assert!(columns.move_beside(Modified, Name, false));
        assert_eq!(columns.order(), [Modified, Name, Keywords, Status, HardLinks, Size, Type]);
        assert!(columns.move_beside(Name, Type, true));
        assert_eq!(columns.order(), [Modified, Keywords, Status, HardLinks, Size, Type, Name]);
        assert!(!columns.move_beside(Name, Type, true));
        assert!(!columns.move_beside(Size, Size, false));
        assert!(columns.reset());
        assert!(columns.is_default());
        assert!(!columns.reset());
    }

    /// A file that names some columns and not others: the rest go back beside their neighbours,
    /// and a repeat counts once.
    #[test]
    fn an_order_that_misses_a_column_keeps_it_beside_its_neighbour() {
        let mut columns = Columns::default();
        columns.set_order([Modified, Size, Name, Size]);
        assert_eq!(columns.order(), [Modified, Size, Type, Name, Keywords, Status, HardLinks]);
        columns.set_order([]);
        assert_eq!(columns.order(), Column::ALL);
    }
}
