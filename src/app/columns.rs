//! The details view's columns being shown, hidden or put back. See [`crate::pane::Columns`].

use super::*;
use crate::fs::Column;

impl App {
    /// After a column has been shown or hidden, in every tab of every pane.
    ///
    /// - **Re-fitted**, so a column put back opens at the width its contents want rather than at
    ///   whatever was last stored for it, and Keywords is measured only while it is on show.
    /// - **A tab sorted by a column that has gone is sorted by Name.** An order nobody can see the
    ///   reason for reads as no order at all — and for Status and Hard links it would be an order
    ///   over answers that are no longer being asked for. The window's preference follows, so the
    ///   next tab does not open sorted by it either.
    ///
    /// Nothing is asked for here: a Status or Hard links column put back is asked about by the
    /// tab's next frame, because a hidden one never marked itself asked. See [`App::collect_cloud`]
    /// and [`App::collect_hard_links`].
    pub(super) fn columns_changed(&mut self) {
        let columns = self.columns;
        for pane in &mut self.panes {
            for tab in pane.tabs.iter_mut() {
                tab.widths_measured = false;
                if !columns.shown(tab.sort_by) {
                    tab.sort_by_column(Column::Name);
                }
            }
        }
        if !columns.shown(self.sort.by) {
            self.sort = crate::fs::Sort {
                by: Column::Name,
                ascending: Column::Name.starts_ascending(),
            };
        }
    }
}
