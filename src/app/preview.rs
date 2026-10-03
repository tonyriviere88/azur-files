//! Deciding what the preview panel should be showing, and collecting it when it arrives.
//!
//! The panel follows the keyboard, debounced by [`crate::ui::preview::FOLLOW_DELAY`] — holding
//! the down arrow through a folder of photographs must not decode thirty of them.

use super::*;

impl App {
    /// Hand finished reads to the panels that asked, and keep each one pointed at its own
    /// keyboard.
    ///
    /// A panel **follows the selection** rather than being told once: you open it, then arrow
    /// down the folder and look at each file in turn without touching the shortcut again. That is
    /// only bearable because it waits — see [`crate::ui::preview::FOLLOW_DELAY`] — so holding the
    /// arrow key down decodes the file you stop on and not the thirty on the way past.
    ///
    /// **Every open panel is followed, not only the focused one.** Two panes side by side each
    /// showing a build of the same DLL is the case the panel is inside the pane *for*, and a
    /// preview that only tracked the pane with the keyboard would go stale in the other one the
    /// moment you clicked across to compare them.
    pub(super) fn collect_previews(&mut self, ctx: &egui::Context, now: f64) {
        // Collected first, so the reader is not borrowed while the panels are written to. The
        // payload goes to the one panel waiting for that token and nowhere else: it is a decoded
        // picture or a walked graph, so there is one of it and no copy to hand round.
        for loaded in self.previews.drain().collect::<Vec<_>>() {
            let wanted = self
                .panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .find(|tab| tab.preview.wants(loaded.token));
            if let Some(tab) = wanted {
                tab.preview.arrived(loaded.token, loaded.payload, ctx);
            }
        }

        // What each open panel should be looking at, worked out before anything is borrowed
        // mutably: `selected_preview` reads the tab, and asking for a read writes to it.
        type Wanted = (PaneId, usize, Option<crate::preview::Ask>);
        let wanted: Vec<Wanted> = self
            .panes
            .iter()
            .flat_map(|pane| {
                pane.tabs
                    .iter()
                    .enumerate()
                    .filter(|(at, tab)| tab.preview.open && *at == pane.active)
                    .map(|(at, tab)| (pane.id, at, Self::selected_preview(tab, self.preview.diff)))
                    .collect::<Vec<_>>()
            })
            .collect();

        let mut soonest: Option<f64> = None;
        for (id, at, what) in wanted {
            let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) else {
                continue;
            };
            let Some(tab) = pane.tabs.get_mut(at) else {
                continue;
            };
            tab.preview.follow(what, now);
            let (ready, left) = tab.preview.settle(now);
            if let Some(ask) = ready {
                let token = self.previews.request(&ask);
                if let Some(tab) = self
                    .panes
                    .iter_mut()
                    .find(|p| p.id == id)
                    .and_then(|p| p.tabs.get_mut(at))
                {
                    tab.preview.asked(ask, token);
                }
            }
            if let Some(left) = left {
                soonest = Some(soonest.map_or(left, |soonest: f64| soonest.min(left)));
            }
        }
        // And the frame that would notice the wait is up. Nothing else would ask for it: this
        // program is idle between events, so a deadline nobody books a frame for is a deadline
        // that arrives the next time something unrelated happens to want one. The *soonest* of
        // them, since one frame serves every panel that is waiting.
        if let Some(left) = soonest {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }

    /// What this tab's preview panel should be showing, if anything.
    ///
    /// **Two pictures selected at once is a comparison**, and that is the one case where the
    /// *selection* rather than the cursor decides: picking a second image is a deliberate act with
    /// an obvious meaning, and no other pair of files has one. Everything else is the cursor's
    /// answer — the cursor is where the keyboard is, it follows a click as well, and a preview is
    /// about one file, so a selection of thirty has nothing to show.
    ///
    /// **And a picture git has a different version of is a comparison too**, when the panel's diff
    /// toggle is on: a `.png` has no lines to put a red band behind, so what "show me what changed"
    /// means for one is the two versions and the difference between them — which is the view two
    /// selected pictures already get. `diffing` is that toggle, and it is read here rather than in the
    /// panel because it changes *what is read*: turning it off has to be a different [`Ask`], or the
    /// panel would go on showing the comparison it is holding.
    pub(super) fn selected_preview(tab: &Tab, diffing: bool) -> Option<crate::preview::Ask> {
        use crate::preview::{kind_of, Ask, Kind};

        let dir = tab.dir.as_ref()?;
        let kind_at = |row: usize| -> Option<Kind> {
            let entry = tab.entry_at(row)?;
            // From the name alone, which costs nothing to ask about a row. A file that passes and
            // turns out to be something else says so in the panel rather than being refused here.
            kind_of(
                dir.leaf(entry),
                dir.ext(entry),
                dir.entries[entry].is_dir(),
            )
        };

        if tab.selected_count == 2 {
            // In display order, so the left-hand or upper view is the upper row — the pair reads
            // the way the listing above it does rather than the way the clicks happened to land.
            let two: Vec<usize> = (0..tab.order.len())
                .filter(|&row| tab.is_selected(row))
                .take(3)
                .collect();
            if let [a, b] = two[..] {
                if kind_at(a) == Some(Kind::Picture) && kind_at(b) == Some(Kind::Picture) {
                    return Some(Ask::Pair(tab.target_at(a)?, tab.target_at(b)?));
                }
            }
        }
        let at = tab.cursor?;
        let kind = kind_at(at)?;
        let path = tab.target_at(at)?;
        if diffing && kind == Kind::Picture && Self::changed_here(tab) {
            return Some(Ask::AgainstHead(path));
        }
        Some(Ask::One(path, kind))
    }

    /// Whether any preview has been asked for and has not come back yet.
    ///
    /// For `--shot --preview`, which would otherwise photograph the word `Reading…`: a decode or a
    /// dependency walk takes tens of milliseconds, which is several frames.
    pub fn preview_pending(&self) -> bool {
        self.panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .any(|tab| tab.preview.busy())
    }

    /// What the drop highlight should cover in this pane, if anything.
    ///
    /// The folder row under the pointer when there is one, and the listing otherwise — which
    /// mirrors where the drop will actually go, since a row is published as a drop zone of its own
    /// and wins over the listing it sits in. A listing-wide highlight over a subfolder would
    /// promise the wrong destination.
    pub(super) fn preview_rect(&self, pane: PaneId, scale: f32) -> Option<Rect> {
        let (x, y) = self.drop_hover?;
        let pane = self.panes.iter().find(|p| p.id == pane)?;
        if pane.tab().path.as_os_str().is_empty() {
            return None;
        }
        // The hover point arrives in physical pixels, as the drop zones are published.
        let scale = scale.max(0.01);
        let at = egui::pos2(x as f32 / scale, y as f32 / scale);
        if !pane.drop_area.contains(at) {
            return None;
        }
        Some(
            pane.drop_rows
                .iter()
                .find(|(row, _)| row.contains(at))
                .map(|(row, _)| row.intersect(pane.drop_area))
                .unwrap_or(pane.drop_area),
        )
    }

    /// [`App::preview_rect`], for the test that checks which rect it picks.
    #[cfg(test)]
    pub fn preview_rect_for_tests(&self, pane: PaneId, scale: f32) -> Option<Rect> {
        self.preview_rect(pane, scale)
    }

    /// What the drop highlight should cover in the Bookmarks section, when a drag is over it.
    ///
    /// **The group under the pointer when there is one, and the whole section otherwise** —
    /// which mirrors where the drop will actually go, since a group is published as a zone of its
    /// own and wins over the section it sits in. The same rule, and the same reason, as
    /// [`Self::preview_rect`] in a listing: a section-wide highlight over a group would promise
    /// the wrong destination.
    ///
    /// A group means its name *and* the bookmarks under it, which is what makes the highlight a
    /// picture of the thing being joined rather than of the row it was aimed at.
    ///
    /// Between two bookmarks there is nothing to promise — pinning appends — which is also why
    /// the pointer is answered `LINK` rather than copy or move.
    pub(super) fn bookmarks_preview(&self, scale: f32) -> Option<Rect> {
        let (x, y) = self.drop_hover?;
        let rect = self.bookmarks_rect?;
        let at = egui::pos2(x as f32 / scale.max(0.01), y as f32 / scale.max(0.01));
        if !rect.contains(at) {
            return None;
        }
        Some(
            self.bookmark_rows
                .iter()
                .find(|(row, _)| row.contains(at))
                .map(|(row, _)| row.intersect(rect))
                .unwrap_or(rect),
        )
    }
}
