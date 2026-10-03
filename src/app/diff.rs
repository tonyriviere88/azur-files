//! The folder diff tab: opening one, keeping its comparison current, and drawing its two halves.
//!
//! What is compared, and how, is [`crate::diff`]'s. This is the window's half of it — and most of
//! that is one arrangement: **the right half is a pane of its own that is not in the layout**. It is
//! in [`App::panes`] with [`Pane::twin`] set, and the diff tab on the left carries its id. So a click
//! in it, a key pressed while it has focus, its listing landing, the watcher re-reading it — every
//! one of those finds it the way it finds any listing, by pane, and nothing on those paths had to
//! learn about diffs. What does have to know is here and in the handful of places that treat a pane
//! as a thing in the tree: see [`App::outer`].
//!
//! The twin goes when its tab does. Rather than every way a tab can go (closed, its pane closed, the
//! window put back from settings) having to remember, [`App::tend_diffs`] drops any twin no tab
//! points at, once a frame.

use std::sync::Arc;

use super::*;

impl App {
    /// The pane in the layout that `id` is drawn in: itself, or — for the right half of a folder
    /// diff — the pane whose tab it belongs to.
    ///
    /// For everything that treats a pane as a place in the tree: a new tab, a split, a tab closed
    /// with `Ctrl+W`. The right half has none of those of its own, and a gesture made while it has
    /// the keyboard means the pane the user can see.
    pub(super) fn outer(&self, id: PaneId) -> PaneId {
        self.host_of(id).map_or(id, |(pane, _)| self.panes[pane].id)
    }

    /// Where the diff tab owning twin pane `twin` is: the index of its pane, and of the tab in it.
    pub(super) fn host_of(&self, twin: PaneId) -> Option<(usize, usize)> {
        self.panes.iter().enumerate().find_map(|(at, pane)| {
            pane.tabs
                .iter()
                .position(|tab| tab.diff.as_ref().and_then(|d| d.twin) == Some(twin))
                .map(|tab| (at, tab))
        })
    }

    /// The other half of the diff that pane `id`'s tab is half of, if it is: the host of a twin, or
    /// the twin of a host.
    pub(super) fn diff_partner(&self, id: PaneId) -> Option<PaneId> {
        self.host_of(id)
            .map(|(at, _)| self.panes[at].id)
            .or_else(|| self.panes.iter().find(|p| p.id == id)?.tab().diff.as_ref()?.twin)
    }

    /// An action from the right half of a diff, sent where it belongs.
    ///
    /// The ones that act on the tree of panes — a new tab, the next tab, a split, a folder opened in
    /// a tab of its own — go to the pane the diff is drawn in, because the right half has no tab
    /// strip and no place in the layout: a tab added to it would silently replace the half on screen.
    /// And closing its tab — `Ctrl+W` — closes the diff, which is the tab it is half of.
    pub(super) fn in_the_layout(&self, action: Action) -> Action {
        match action {
            Action::CloseTab { pane, tab } => match self.host_of(pane) {
                Some((at, diff)) => Action::CloseTab {
                    pane: self.panes[at].id,
                    tab: diff,
                },
                None => Action::CloseTab { pane, tab },
            },
            Action::NewTab { pane } => Action::NewTab {
                pane: self.outer(pane),
            },
            Action::NextTab { pane, delta } => Action::NextTab {
                pane: self.outer(pane),
                delta,
            },
            Action::NavigateNewTab { pane, path } => Action::NavigateNewTab {
                pane: self.outer(pane),
                path,
            },
            Action::OpenInSplit { pane, path, side } => Action::OpenInSplit {
                pane: self.outer(pane),
                path,
                side,
            },
            other => other,
        }
    }

    /// Whether `action` is one a diff half has no switch for — preview, console, tiles, lens — and
    /// so is refused rather than done out of sight. Their keys still arrive; this is where they stop.
    /// (Flatten is the tab's own to refuse — see [`crate::pane::Tab::toggle_flat`].)
    pub(super) fn refused_in_a_diff(&self, action: &Action) -> bool {
        let pane = match action {
            Action::TogglePreview(pane) | Action::ToggleConsole(pane) => *pane,
            Action::SetView { pane, .. } | Action::SetLens { pane, .. } => *pane,
            _ => return false,
        };
        self.panes
            .iter()
            .find(|p| p.id == pane)
            .is_some_and(|p| p.tab().diff.is_some())
    }

    /// `Folder diff...` from the application menu.
    ///
    /// The folder the focused pane is showing goes on the left. The right is **another pane's
    /// folder**, if there is another pane showing a different one — two panes side by side is the
    /// usual way somebody comes to want to compare them — and otherwise the same folder, with its path
    /// bar picked out for a few seconds to say that is where the second one goes. **The keyboard
    /// stays where it was**: see [`crate::diff::Flash`].
    pub(super) fn open_folder_diff(&mut self) {
        let host = self.outer(self.focused);
        let Some(left) = self
            .panes
            .iter()
            .find(|p| p.id == host)
            .map(|p| p.tab().path.clone())
        else {
            return;
        };
        let mut order = Vec::new();
        self.layout.panes(&mut order);
        let other = order
            .iter()
            .filter(|id| **id != host)
            .filter_map(|id| self.panes.iter().find(|p| p.id == *id))
            .map(|p| p.tab().path.clone())
            .find(|path| *path != left);
        self.diff_folders(host, left, other);
    }

    /// A folder diff of `left` against `right` in a new tab of pane `host`, in front and with the
    /// keyboard. No `right` is the same folder twice, with the right half's path bar picked out.
    pub(super) fn diff_folders(&mut self, host: PaneId, left: PathBuf, right: Option<PathBuf>) {
        let (hidden, show) = (self.show_hidden, self.diff_show);
        let mut twin_tab =
            Tab::diff_side(right.clone().unwrap_or_else(|| left.clone()), hidden, show, None);
        if right.is_none() {
            if let Some(side) = twin_tab.diff.as_mut() {
                side.flash = crate::diff::Flash::Wanted;
            }
        }
        let twin = self.spawn_pane(twin_tab);
        if let Some(p) = self.pane_mut(twin) {
            p.twin = true;
        }
        let Some(p) = self.pane_mut(host) else { return };
        p.tabs.push(Tab::diff_side(left, hidden, show, Some(twin)));
        p.show_tab(p.tabs.len() - 1);
        self.focused = host;
    }

    /// Once a frame: drop twins nobody owns, keep the keyboard out of a half nobody can see, and
    /// keep every diff's comparison the comparison of what its two halves are showing.
    ///
    /// **The last is what makes changing either root re-run the diff**, and it is one rule rather
    /// than a hook on each way a root can change: a half's listing is replaced whenever its folder
    /// is — navigated, refreshed, re-read by the watcher, `Ctrl+H` — and a comparison that is not of
    /// the two listings on show is let go of and asked for again. See [`crate::diff::Differ`].
    pub(super) fn tend_diffs(&mut self) {
        // Taken whether or not a diff is still open to take them, so an answer nobody wants does not
        // sit in the channel holding two trees.
        let arrived: Vec<Arc<crate::diff::Comparison>> =
            std::iter::from_fn(|| self.differ.poll()).collect();
        if !self.panes.iter().any(|p| p.twin) {
            return;
        }

        // ---- Twins nobody owns ------------------------------------------
        let owned: Vec<PaneId> = self
            .panes
            .iter()
            .flat_map(|p| p.tabs.iter())
            .filter_map(|tab| tab.diff.as_ref().and_then(|d| d.twin))
            .collect();
        self.panes.retain(|p| !p.twin || owned.contains(&p.id));
        if !self.panes.iter().any(|p| p.id == self.focused) {
            self.focused = self.panes.iter().find(|p| !p.twin).map_or(0, |p| p.id);
        }

        // ---- The keyboard, somewhere it can be seen ---------------------
        if let Some((at, tab)) = self.host_of(self.focused) {
            if self.panes[at].active != tab {
                self.focused = self.panes[at].id;
            }
        }

        // ---- The comparisons --------------------------------------------
        for at in 0..self.panes.len() {
            for index in 0..self.panes[at].tabs.len() {
                let Some(twin) = self.panes[at].tabs[index].diff.as_ref().and_then(|d| d.twin)
                else {
                    continue;
                };
                if let Some(other) = self.panes.iter().position(|p| p.id == twin) {
                    self.tend_diff(at, index, other, &arrived);
                }
            }
        }

        // ---- Where an undrawn twin was --------------------------------------
        //
        // Cleared here and set again by the drawing if it is drawn: a drop is resolved against the
        // last frame's rects, and a half that is not on screen must not go on catching drops at
        // the place it last was.
        for pane in self.panes.iter_mut().filter(|p| p.twin) {
            pane.rect = Rect::NOTHING;
            pane.drop_area = Rect::NOTHING;
            pane.drop_rows.clear();
        }
    }

    /// One diff's turn of [`App::tend_diffs`]: tab `index` of pane `at` on the left, pane `other` on
    /// the right.
    ///
    /// Current, and nothing to do; or not, and whatever is attached describes listings that are gone —
    /// so it is replaced by an answer that has landed, or let go of while another is asked for.
    fn tend_diff(
        &mut self,
        at: usize,
        index: usize,
        other: usize,
        arrived: &[Arc<crate::diff::Comparison>],
    ) {
        let hidden = self.show_hidden;
        // Only a listing that read. An error is a sentence where the rows would be, and a diff against
        // it would call everything on the other side new.
        fn read(dir: &Option<Arc<crate::fs::Dir>>) -> Option<&Arc<crate::fs::Dir>> {
            dir.as_ref().filter(|d| d.error.is_none())
        }
        let pair = {
            let left = &self.panes[at].tabs[index];
            match (read(&left.dir), read(&self.panes[other].tab().dir)) {
                (Some(l), Some(r)) => {
                    let current = left
                        .diff
                        .as_ref()
                        .and_then(|d| d.comparison.as_ref())
                        .is_some_and(|c| c.of.is(l, r, hidden));
                    if current {
                        return;
                    }
                    Some(crate::diff::Pair {
                        left: l.clone(),
                        right: r.clone(),
                        show_hidden: hidden,
                    })
                }
                // Either half is between folders: nothing to compare, and nothing to keep the old
                // trees alive for.
                _ => None,
            }
        };
        let landed = pair.as_ref().and_then(|p| {
            arrived
                .iter()
                .find(|c| c.of.is(&p.left, &p.right, hidden))
                .cloned()
        });
        if let Some(c) = &landed {
            self.panes[at].tabs[index].title = format!(
                "{} ↔ {}",
                crate::fs::display_name(&c.of.left.path),
                crate::fs::display_name(&c.of.right.path)
            );
        }
        self.panes[other].tab_mut().set_comparison(landed.clone());
        let tab = &mut self.panes[at].tabs[index];
        tab.set_comparison(landed.clone());
        let Some(side) = tab.diff.as_mut() else { return };
        match (pair, landed) {
            (Some(pair), None) => {
                let asked = side
                    .asked
                    .as_ref()
                    .is_some_and(|a| a.is(&pair.left, &pair.right, hidden));
                if !asked {
                    self.differ.ask(pair.clone(), &side.ticket);
                    side.asked = Some(pair);
                }
            }
            _ => side.asked = None,
        }
    }

    /// Which rows both halves of a diff show — the button on its path bar — and so what the next diff
    /// opens showing. Written to the settings. See [`crate::diff::Show`].
    pub(super) fn set_diff_show(&mut self, pane: PaneId, show: crate::diff::Show) {
        self.show_in_diff(pane, show);
        if self.diff_show != show {
            self.diff_show = show;
            self.config_dirty = true;
        }
    }

    /// [`App::set_diff_show`] for this diff alone, leaving the preference as it was — for `--diff=`,
    /// which is a capture run's choice and not the user's.
    ///
    /// Back to the top afterwards, both sides, for the reason a settled filter goes there: row 200 of
    /// the whole tree is not row 200 of what is left of it.
    pub(super) fn show_in_diff(&mut self, pane: PaneId, show: crate::diff::Show) {
        let partner = self.diff_partner(pane);
        for id in std::iter::once(pane).chain(partner) {
            let Some(p) = self.pane_mut(id) else { continue };
            let tab = p.tab_mut();
            let Some(side) = tab.diff.as_mut() else { continue };
            if side.show == show {
                continue;
            }
            side.show = show;
            tab.rebuild_order();
            tab.scroll_y = 0.0;
            tab.scroll_to = Some(0.0);
        }
    }

    /// Follow a twisty clicked in one half of a diff in the other half, so the two trees open and
    /// shut together and a folder on the left can be read against the same folder on the right.
    pub(super) fn follow_collapse(&mut self, pane: PaneId, name: &str, shut: bool) {
        let Some(partner) = self.diff_partner(pane) else { return };
        if let Some(p) = self.pane_mut(partner) {
            p.tab_mut().set_collapsed_named(name, shut);
        }
    }

    /// A pane whose diff tab is showing: the two halves, side by side, each a path bar over a tree.
    ///
    /// Half each and a seam between, the seam being what shows between any two panes. No preview
    /// and no console — see [`breadcrumb::show`] for why the bar is short as well.
    pub(super) fn diff_pane(&mut self, ui: &mut Ui, t: &Theme, host: PaneId, twin: PaneId, rect: Rect) {
        use egui::emath::GuiRounding as _;
        let middle = rect.center().x.round_to_pixels(ui.painter().pixels_per_point());
        let left = Rect::from_min_max(rect.min, pos2(middle, rect.bottom()));
        let right = Rect::from_min_max(pos2(middle + crate::ui::SEAM, rect.top()), rect.max);
        // Once between the halves as well as after them: a scroll of the left is known the moment
        // it has been drawn, and following it before the right is drawn puts both on screen in the
        // same frame. A scroll of the right can only be followed on the next one.
        self.diff_half(ui, t, host, left);
        self.sync_scroll(host, twin);
        self.diff_half(ui, t, twin, right);
        self.sync_scroll(host, twin);
    }

    /// Keep the two halves scrolled to the same place in the tree.
    ///
    /// **Whichever side moved leads**, and "moved" is its offset having changed since the last frame
    /// — by the wheel, the scrollbar, the keyboard's cursor, a collapse. The row at its top is looked
    /// up by name on the other side ([`crate::diff::Lookup`]), and the other side is scrolled to put
    /// that row, or the nearest one before it, at its top — with the same fraction of a row showing.
    ///
    /// The side that was scrolled for it is marked [`crate::diff::Side::steered`], so that when its
    /// offset changes on the next frame — to what it was sent, or to less, where its listing is too
    /// short to go that far — it is not taken for a scroll of its own and sent back. Without that the
    /// two would chase each other a row at a time.
    fn sync_scroll(&mut self, host: PaneId, twin: PaneId) {
        use crate::pane::ROW_HEIGHT;
        let (Some(h), Some(w)) = (
            self.panes.iter().position(|p| p.id == host),
            self.panes.iter().position(|p| p.id == twin),
        ) else {
            return;
        };
        let moved = |tab: &mut Tab| -> bool {
            let Some(side) = tab.diff.as_mut() else { return false };
            let moved = !side.steered && side.seen != tab.scroll_y;
            side.steered = false;
            side.seen = tab.scroll_y;
            moved
        };
        let led_by_left = moved(self.panes[h].tab_mut());
        let led_by_right = moved(self.panes[w].tab_mut());
        let (from, to) = match (led_by_left, led_by_right) {
            (true, _) => (h, w),
            (false, true) => (w, h),
            _ => return,
        };

        // What is at the top of the side that moved.
        let leader = self.panes[from].tab();
        let Some(dir) = leader.dir.clone() else { return };
        let top = (leader.scroll_y / ROW_HEIGHT).floor().max(0.0);
        let within = leader.scroll_y - top * ROW_HEIGHT;
        let Some(entry) = leader.entry_at(top as usize) else { return };
        let name = dir.name(entry);

        // And where that is on the other: its own `Arc`s, because the lookup it keeps is written
        // while the comparison it reads is borrowed from the same side.
        let follower = self.panes[to].tab_mut();
        let Some(other) = follower.dir.clone() else { return };
        let Tab {
            diff,
            order,
            order_gen,
            scroll_to,
            ..
        } = follower;
        let Some(side) = diff.as_mut() else { return };
        let Some(comparison) = side.comparison.clone() else { return };
        let (of, half) = comparison.half(side.is_left());
        if !Arc::ptr_eq(of, &other) {
            return;
        }
        let Some(position) = side.lookup.position_of(half, &other, order, *order_gen, name) else {
            return;
        };
        let offset = position as f32 * ROW_HEIGHT + within;
        if (offset - side.seen).abs() >= 0.5 {
            *scroll_to = Some(offset);
            side.steered = true;
        }
    }

    /// One half: [`App::pane`], less the preview panel and the console.
    fn diff_half(&mut self, ui: &mut Ui, t: &Theme, id: PaneId, rect: Rect) {
        let Some(index) = self.panes.iter().position(|p| p.id == id) else {
            return;
        };
        self.panes[index].rect = rect;
        self.surface(ui, t, rect);
        if rect.width() < 96.0 || rect.height() < 48.0 {
            self.panes[index].drop_area = Rect::NOTHING;
            self.panes[index].drop_rows.clear();
            return;
        }
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), breadcrumb::HEIGHT));
        let floor = Rect::from_min_max(
            pos2(rect.left(), rect.bottom() - crate::ui::filelist::STATUS_HEIGHT),
            rect.max,
        );
        let list = Rect::from_min_max(bar.left_bottom(), pos2(rect.right(), floor.top()));

        let focused = id == self.focused;
        let mut child = self.claimed(ui, id, rect);
        let outcome = self.listing(&mut child, t, id, index, bar, list, floor, focused, false, 0.0);
        self.landed(ui, t, id, index, outcome);
    }
}
