//! What the sync provider says about the folder on screen. See [`crate::shell::cloud`].

use super::*;

impl App {
    /// Ask about whatever synced folder each pane is showing, and hand the answers back to the tabs.
    ///
    /// **The active tab of each pane only**, for the reason [`App::collect_git`] gives: a background
    /// tab asks when it is switched to. Only a listing that is [`crate::fs::Dir::synced`] asks at
    /// all, so every other folder costs one bool per frame.
    ///
    /// **Nor while the Status column is hidden**, which is the only thing on screen the answers are
    /// for. The tab is not marked asked either, so putting the column back asks on the next frame.
    /// See [`crate::pane::Columns`].
    pub(super) fn collect_cloud(&mut self) {
        let wanted = self.columns.shown(crate::fs::Column::Status);
        for pane in &mut self.panes {
            let tab = pane.tab_mut();
            let Some(dir) = tab.dir.as_ref() else { continue };
            if !wanted
                || tab.cloud_asked
                || !dir.synced
                || crate::fs::is_synthetic(&tab.path)
                || crate::archive::is_virtual_location(&tab.path)
            {
                continue;
            }
            tab.cloud_asked = true;
            self.cloud.request(tab.view, dir);
        }

        for answer in self.cloud.drain() {
            for tab in tabs_mut(&mut self.panes) {
                if tab.view != answer.view {
                    continue;
                }
                let Some(len) = tab.dir.as_ref().map(|dir| dir.len()) else { continue };
                let states = answer.states.iter().map(|&(index, state)| (index, Some(state)));
                scatter(&mut tab.cloud, len, None, states);
            }
        }
    }
}
