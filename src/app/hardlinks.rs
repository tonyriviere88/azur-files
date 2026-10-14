//! How many names each file has, for the folder on screen. See [`crate::hardlinks`].

use super::*;

impl App {
    /// Ask about whatever local NTFS folder each pane is showing, and hand the counts back to the
    /// tabs.
    ///
    /// **The active tab of each pane only**, as [`App::collect_cloud`] does: a background tab asks
    /// when it is switched to.
    ///
    /// **Nor while the Hard links column is hidden**: the count is a walk of the folder's file IDs,
    /// and with nowhere to show it that is work for a tooltip line. The tab is not marked asked, so
    /// putting the column back asks on the next frame. See [`crate::pane::Columns`].
    pub(super) fn collect_hard_links(&mut self) {
        let wanted = self.columns.shown(crate::fs::Column::HardLinks);
        for pane in &mut self.panes {
            let tab = pane.tab_mut();
            let Some(dir) = tab.dir.as_ref() else { continue };
            if !wanted || tab.hard_links_asked {
                continue;
            }
            tab.hard_links_asked = true;
            if crate::hardlinks::wanted(dir, &tab.path) {
                self.hard_links.request(tab.view, dir);
            }
        }

        for answer in self.hard_links.drain() {
            for tab in tabs_mut(&mut self.panes) {
                if tab.view != answer.view {
                    continue;
                }
                let Some(len) = tab.dir.as_ref().map(|dir| dir.len()) else { continue };
                let digits = |n: u32| n.checked_ilog10().unwrap_or(0);
                let widest = tab.hard_links.iter().copied().max().map(digits);
                // Only with counts in it: an answer that only says the walk is over leaves the slots
                // as they are.
                if !answer.counts.is_empty() {
                    scatter(&mut tab.hard_links, len, 0, answer.counts.iter().copied());
                }
                // The column opens on the first count, and widens if a count needs another
                // digit. Re-fitted rather than set, so its width is decided where the others' are.
                let now = tab.hard_links.iter().copied().max().map(digits);
                if now > widest {
                    tab.widths_measured = false;
                }
                if answer.done && tab.sort_by == crate::fs::Column::HardLinks {
                    tab.rebuild_order();
                }
            }
        }
    }
}
