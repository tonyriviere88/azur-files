//! What the sync provider says about the folder on screen. See [`crate::shell::cloud`].

use super::*;

impl App {
    /// Ask about whatever synced folder each pane is showing, and hand the answers back to the tabs.
    ///
    /// **The active tab of each pane only**, for the reason [`App::collect_git`] gives: a background
    /// tab asks when it is switched to. Only a listing that is [`crate::fs::Dir::synced`] asks at
    /// all, so every other folder costs one bool per frame.
    pub(super) fn collect_cloud(&mut self) {
        for pane in &mut self.panes {
            let tab = pane.tab_mut();
            let Some(dir) = tab.dir.as_ref() else { continue };
            if tab.cloud_asked
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
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    if tab.view != answer.view {
                        continue;
                    }
                    let Some(len) = tab.dir.as_ref().map(|dir| dir.len()) else { continue };
                    if tab.cloud.len() != len {
                        tab.cloud = vec![None; len];
                    }
                    for &(index, state) in &answer.states {
                        if let Some(slot) = tab.cloud.get_mut(index as usize) {
                            *slot = Some(state);
                        }
                    }
                }
            }
        }
    }
}
