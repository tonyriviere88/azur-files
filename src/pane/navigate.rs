//! Where a tab points, and the flatten and grouping that change what "here" means.

use super::*;

impl Tab {
    /// Go somewhere, recording it in the history.
    pub fn navigate(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if path == self.path {
            return;
        }
        // Anything you do after going Back replaces the forward trail.
        self.history.truncate(self.at + 1);
        self.history.push(path.clone());
        self.at = self.history.len() - 1;
        // A history that grows all session is a leak nobody notices until it is
        // one; 256 places back is more than anyone walks.
        if self.history.len() > 256 {
            let excess = self.history.len() - 256;
            self.history.drain(..excess);
            self.at -= excess;
        }
        self.go_to(path);
    }

    pub fn can_go_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.at + 1 < self.history.len()
    }

    pub fn go_back(&mut self) {
        if self.can_go_back() {
            self.at -= 1;
            let path = self.history[self.at].clone();
            // Where you were is highlighted by `go_to`, from the trail, for every arrival rather
            // than only for the one that lands on the parent.
            self.go_to(path);
        }
    }

    pub fn go_forward(&mut self) {
        if self.can_go_forward() {
            self.at += 1;
            let path = self.history[self.at].clone();
            self.go_to(path);
        }
    }

    /// Up one level. The folder just left is highlighted by [`Tab::go_to`], off the trail.
    pub fn go_up(&mut self) {
        if let Some(parent) = crate::fs::parent_of(&self.path) {
            self.navigate(parent);
        }
    }

    /// Point the tab at a path without touching the history.
    pub(crate) fn go_to(&mut self, path: PathBuf) {
        // The one place [`Tab::trail`] is decided, because this is the one place the path
        // changes — `navigate`, `go_back`, `go_forward` and `go_up` all come through here.
        //
        // `Path::starts_with` compares whole components, so `C:\Users` is a prefix of
        // `C:\Users\tony` and `C:\Use` is not. The empty path is a prefix of everything,
        // which is the answer this wants: This PC is where the breadcrumb starts, so going
        // there is going up rather than going somewhere else.
        if !self.trail.starts_with(&path) {
            self.trail = path.clone();
        }
        // **The child of this folder that the breadcrumb still shows, selected on arrival.**
        // Standing in `a/b` with `a/b/c` on the bar, `c` is the one thing you are most likely to
        // want next — it is where you just came from, or where you were heading before you
        // stopped off here — and the bar is already pointing at it. So the listing selects it and
        // scrolls to it, which for a folder of five thousand names is the difference between
        // going up a level and losing your place.
        //
        // Asked of [`crate::fs::breadcrumb_segments`] rather than worked out from components, so
        // that "what the breadcrumb shows" means literally that: the same walk, with the same
        // answer for a drive root and for This PC, where a raw component walk gives `C:` without
        // its root and no place at all above it.
        //
        // This subsumes what `go_up` and `go_back` each used to do for themselves, and covers
        // what neither did: clicking a segment three levels up now selects the segment below it
        // too, rather than only a single step back.
        let trail = crate::fs::breadcrumb_segments(&self.trail);
        self.reveal = trail
            .iter()
            .position(|(_, at)| *at == path)
            .and_then(|here| trail.get(here + 1))
            .map(|(_, child)| display_name(child));
        // A diff tab keeps the name it was given: it is about two folders, and this is one of them.
        if self.diff.is_none() {
            self.title = display_name(&path);
        }
        self.path = path;
        self.dir = None;
        self.awaiting = None;
        self.asked_at = None;
        // A new view of a new folder: any icon answer still in flight for the old one is now
        // addressed to a view that no longer exists, and the column it would have gone in is
        // released here.
        self.view = next_view();
        self.file_icons = Vec::new();
        self.file_icons.shrink_to_fit();
        self.links = std::collections::HashMap::new();
        // The measurement goes with the folder it was about — a fresh generation inside `forget`
        // rather than waiting for the next listing to land, so the walks under way are abandoned on
        // this frame instead of at whatever point the new folder arrives. See
        // [`crate::sizes::Sizes::only`].
        //
        // **The button itself stays as it was**, which is the one place this function keeps a view
        // setting rather than dropping it. See [`crate::sizes::Measurement::on`] for why: the whole
        // gesture is to open the folder that turned out to be big and ask the same question of it.
        self.sizes.forget();
        self.order.clear();
        self.order_gen = next_order_gen();
        self.tree.clear();
        // The tiles' geometry, which described an order this tab no longer has. Dropped rather than
        // left to be noticed as stale: a tree of a hundred thousand files leaves the best part of a
        // megabyte of block tops and cell positions behind, and `view_mode` going back to `Details`
        // just below means nothing would ask `Layout::ensure` to replace it.
        self.grid = crate::ui::grid::Layout::default();
        self.selected.clear();
        self.selected_count = 0;
        self.selected_size = 0;
        self.cursor = None;
        self.anchor = None;
        self.filter.clear();
        self.filter_at = None;
        // And with it the other half of the filter, for the same reason: what git has touched *here*
        // is not a question about the folder being opened, any more than a name fragment is. See
        // [`Lens`]. `Tab::refresh` keeps it, because that is the same folder read again.
        self.lens = None;
        // And neither do the tiles, for the same reason as the two above: a folder of photographs is
        // worth looking at as pictures and the folder you open out of it is a different question. So
        // every folder opens in the details view — see [`ViewMode`], which is also why there is no
        // setting for this *value*.
        self.view_mode = ViewMode::Details;
        // What there is a setting for is the rule, and this is where a folder becomes eligible for
        // it: the listing that lands next is a folder nobody has looked at yet, so it may still be
        // read as a folder of pictures and opened as tiles. See [`Tab::choose_view`] and
        // [`AutoTiles`] — and note that the line above is not undone by it, only overruled: a folder
        // that is not mostly pictures opens in the details view, as it always has.
        self.opening = true;
        // **A flatten does not come along to the next folder**, for the same reason the
        // filter does not: both are a question asked of the folder you were looking at,
        // and the answer to a question about somewhere else is not the same answer. It
        // also means opening a row in a flattened listing lands in an ordinary folder,
        // which is the only way out of the view that does not need the button again — and
        // it is what stops a click on a deep folder from silently starting a second tree
        // walk. `Tab::refresh` keeps it, because that is the same question again.
        //
        // **Except in a folder diff**, which is a comparison of two trees and nothing else — see
        // [`Tab::diff`]. Opening a folder there moves that side's root down to it, and the walk is
        // the point.
        self.flat = self.diff.is_some();
        // And with it what was shut in the tree, which named folders under the folder being
        // left. `Tab::refresh` keeps these too: the same tree, read again, is the same tree.
        self.collapsed.clear();
        self.widths_measured = false;
        self.editing_path = false;
        // **A different folder opens at the top.** Row 200 of the folder you just left is not
        // row 200 of anything, and the scroll offset does not belong to this tab as far as egui
        // is concerned — it belongs to the pane's one scroll area, which goes on showing
        // whatever it was showing unless something asks it not to. Setting `scroll_y` alone
        // records where the listing *is*; `scroll_to` is what moves it.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        self.band = None;
        self.renaming = None;
        self.keep_selected.clear();
        // A snapshot of somewhere else says nothing about what is new here.
        self.name_the_new = None;
    }

    /// Flatten this folder, or stop flattening it.
    ///
    /// The listing goes, because the two are different listings of the same folder —
    /// a flattened one is [`crate::fs::scan::scan_deep`]'s and holds relative paths.
    /// Turning it *off* comes straight back out of the cache, which still has the
    /// folder's own children; turning it *on* is a fresh walk every time, which is the
    /// point of the gesture.
    ///
    /// The selection is not carried across. It is kept by name, and a name means two
    /// different things on the two sides of this: `file.txt` on one and
    /// `sub\file.txt` on the other, so nothing would match anyway — and a selection
    /// that half-survived would be worse than one that plainly did not.
    pub fn toggle_flat(&mut self, mode: FlatMode, regroup: bool) {
        // "This PC" is not a folder and has no tree: its rows are volumes, each of which is a
        // place to flatten of its own. There is nothing for the button to do here, so it is
        // drawn disabled and this refuses — rather than latching over a listing that would not
        // have changed.
        if self.path.as_os_str().is_empty() {
            return;
        }
        // A folder diff is always a tree. See [`Tab::diff`].
        if self.diff.is_some() {
            return;
        }
        self.flat = !self.flat;
        // Both halves of the window's preference for how a tree looks, taken at the moment the view
        // is turned on. A tree opens fully expanded, which is what an empty `collapsed` means — see
        // the field.
        self.flat_mode = mode;
        self.regroup = regroup;
        self.collapsed.clear();
        self.keep_selected.clear();
        self.selected.clear();
        self.selected_count = 0;
        self.selected_size = 0;
        self.cursor = None;
        self.anchor = None;
        self.renaming = None;
        // For the same reason as the selection just above: a name is `file.txt` on one side of
        // this and `sub\file.txt` on the other, so nothing in the snapshot would match and every
        // row of the new listing would look new.
        self.name_the_new = None;
        self.dir = None;
        self.awaiting = None;
        self.asked_at = None;
        self.order.clear();
        self.order_gen = next_order_gen();
        self.tree.clear();
        self.widths_measured = false;
        // And the measurement, because the two sides of this are counted differently and neither
        // answer is the other's: a folder's own children are walked one by one, and a flattened tree
        // already holds every file it would have walked. See [`Tab::settle_sizes`], which is where
        // the two totals part company, and [`crate::sizes::Measurement::wanted`].
        self.sizes.forget();
        // The top, because row 200 of a folder's own children is not row 200 of its
        // whole tree — the same reason a new folder opens at the top in [`Tab::go_to`].
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
    }

    /// Show the flattened tree the other way round.
    ///
    /// **Nothing is re-read**, which is the point of the two modes being one listing: the walk's
    /// answer is already here and the mode only decides the order built over it. So this is a
    /// re-sort, and on a tree that took seconds to walk it is instant.
    ///
    /// The selection *is* kept, unlike [`Tab::toggle_flat`]'s — it is held by entry index, and
    /// both modes are orders over the same entries, so every selected row is still the same file.
    /// Its position moves, which is what [`Tab::rebuild_order`] already puts the cursor back for.
    ///
    /// Nothing happens when the tab is not flattened: the mode is the window's preference and
    /// takes effect the next time the button is pressed. Answering `false` for that case rather
    /// than doing the work anyway is what lets `App` skip the rebuild.
    /// Merge chains of only-children into one row, or stop. Answers whether it did anything.
    ///
    /// [`Tab::set_flat_mode`]'s twin, and everything said there applies: it is a re-sort and never a
    /// re-read, the preference is the window's, and a tab that is not showing a tree takes the
    /// setting for the next time its button is pressed. A tab showing the flat **list** is in the
    /// second case rather than the first — the merge is the tree's shape and the list has no shape to
    /// change.
    pub fn set_regroup(&mut self, on: bool) -> bool {
        // Never in a folder diff — see [`Tab::diff_side`].
        if self.diff.is_some() {
            return false;
        }
        if !self.is_tree() || self.regroup == on {
            self.regroup = on;
            return false;
        }
        self.regroup = on;
        self.rebuild_order();
        // The rows are the same rows arranged differently, which is [`Tab::set_flat_mode`]'s reason
        // for going back to the top as well.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        true
    }

    /// Show the files Windows marks hidden, or stop. Answers whether it did anything.
    ///
    /// [`Tab::set_regroup`]'s twin for the third of the window's listing preferences, with one
    /// difference: this decides which entries are rows at all rather than how they are arranged, so
    /// the columns are measured again — the widest name in a folder is often a hidden one, and a
    /// `.git` under a column fitted without it is a truncated row.
    ///
    /// Still never a re-read: the walk always reports hidden entries and it is the display that
    /// leaves them out. See [`crate::fs::sort::build_order`].
    pub fn set_show_hidden(&mut self, on: bool) -> bool {
        if self.show_hidden == on {
            return false;
        }
        self.show_hidden = on;
        self.rebuild_order();
        self.widths_measured = false;
        true
    }

    pub fn set_flat_mode(&mut self, mode: FlatMode) -> bool {
        // A folder diff is a tree whatever the window prefers. See [`Tab::diff`].
        if self.diff.is_some() {
            return false;
        }
        if !self.flat || self.flat_mode == mode {
            self.flat_mode = mode;
            return false;
        }
        self.flat_mode = mode;
        self.rebuild_order();
        // Straight to the top for the reason a settled filter goes there: the rows are the same
        // rows, and row 200 of a list is not row 200 of the tree the same files make.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        true
    }

    /// Open or shut the folder at a position in the display order, in a tree.
    ///
    /// Answers whether it did anything: `false` for a row that is not a folder, and for a listing
    /// that is not a tree — a caller does not have to test either first.
    ///
    /// Keyed by the row's stored name, which is its path relative to the folder being flattened.
    /// See [`Tab::collapsed`] for why that and not the entry index.
    pub fn toggle_collapsed(&mut self, position: usize) -> bool {
        self.set_collapsed(position, !self.is_collapsed(position))
    }

    /// Shut a folder, or open it. Answers whether the tree changed.
    pub fn set_collapsed(&mut self, position: usize, shut: bool) -> bool {
        match self.entry_at(position) {
            Some(entry) => self.set_collapsed_entry(entry, shut),
            None => false,
        }
    }

    /// [`Tab::set_collapsed`] by entry index rather than position — the half both it and
    /// [`Tab::set_collapsed_named`] come down to.
    fn set_collapsed_entry(&mut self, entry: usize, shut: bool) -> bool {
        if !self.is_tree() {
            return false;
        }
        let Some(dir) = self.dir.clone() else {
            return false;
        };
        if !dir.entries.get(entry).is_some_and(|e| e.is_dir()) {
            return false;
        }
        let name = dir.name(entry);
        let changed = if shut {
            self.collapsed.insert(name.to_owned())
        } else {
            self.collapsed.remove(name)
        };
        if !changed {
            return false;
        }
        // The rows under it are leaving the order or joining it, so the cursor's *position* moves
        // even though it is still on the same file — which `rebuild_order` puts back.
        self.rebuild_order();
        true
    }

    /// Shut or open the folder called `name`, if this tree has one — the other half of a folder
    /// diff following a twisty clicked on this one. Answers whether the tree changed.
    ///
    /// By name rather than position, because the two halves are two different orders; and matched
    /// the way the diff matches, ignoring case, so the folder it finds is the one the diff paired up.
    pub fn set_collapsed_named(&mut self, name: &str, shut: bool) -> bool {
        let entry = self
            .dir
            .as_ref()
            .zip(self.diff.as_ref())
            .and_then(|(dir, diff)| diff.entry_named(dir, name));
        match entry {
            Some(entry) => self.set_collapsed_entry(entry, shut),
            None => false,
        }
    }

    /// The name of the row at `position` — its path relative to the folder, in a flattened listing.
    pub fn name_at(&self, position: usize) -> Option<&str> {
        Some(self.dir.as_ref()?.name(self.entry_at(position)?))
    }

    /// Whether the folder at a position in the display order is shut.
    ///
    /// `false` for anything that is not a shut folder, including every row of a listing that is
    /// not a tree — so this is also the question "does this row draw an opened twisty".
    pub fn is_collapsed(&self, position: usize) -> bool {
        if !self.flat || self.flat_mode != FlatMode::Tree {
            return false;
        }
        let Some(dir) = self.dir.as_ref() else {
            return false;
        };
        let Some(entry) = self.entry_at(position) else {
            return false;
        };
        self.collapsed.contains(dir.name(entry))
    }

    /// Whether the listing on show is a tree — a flatten in [`FlatMode::Tree`].
    ///
    /// The one question the drawing asks, and the one place the two halves of it are spelled out
    /// together: the mode alone says nothing while the tab is showing a folder's own children.
    #[inline]
    pub fn is_tree(&self) -> bool {
        self.flat && self.flat_mode == FlatMode::Tree
    }

    /// How far in the row at `position` is drawn — its depth in the tree **as shown**.
    ///
    /// Not `Dir::depth`, and the difference is a merged chain: `src > main > java` is one row
    /// standing where `src` stood, so it is at `src`'s depth and its children are one in from
    /// *that*. `0` for every row of every listing that is not a tree. See [`sort::TreeRow`].
    #[inline]
    pub fn row_depth(&self, position: usize) -> usize {
        self.tree.get(position).map_or(0, |row| row.depth as usize)
    }

    /// How many folders are merged into the name of the row at `position`: `0` for an ordinary row,
    /// `2` for `a > b > c`.
    #[inline]
    pub fn row_merged(&self, position: usize) -> usize {
        self.tree.get(position).map_or(0, |row| row.merged as usize)
    }

    /// Whether the row at `position` has anything **inside** it on show.
    ///
    /// **Asked of the display order rather than of the file system.** In pre-order a folder's
    /// children are the rows immediately after it, so a next row drawn further in *is* the answer —
    /// one comparison, no index, nothing to keep in step.
    ///
    /// Which is why it compares the depths the rows are *drawn* at: a merged chain's path is deeper
    /// than its indent, so an empty folder followed by the chain `b > c` would otherwise look as
    /// though the chain were inside it, and wear a twisty that does nothing.
    ///
    /// `false` therefore covers a folder with nothing in it — empty, or the walk stopped at its
    /// budget, or a junction it would not follow — and every row of a listing that is not a tree.
    /// A folder the user has **shut** also answers `false`, because its children are not in the
    /// order at all; that is the other half of the twisty's test, where it is asked.
    pub fn has_children_below(&self, position: usize) -> bool {
        let Some(here) = self.tree.get(position) else {
            return false;
        };
        self.tree
            .get(position + 1)
            .is_some_and(|next| next.depth > here.depth)
    }

    /// Drop the listing so the folder is read again, keeping the selection by name.
    ///
    /// This is a *refresh*, not a navigation: the same folder, read afresh because something
    /// changed it. Every file operation ends here, so losing the selection here means copying a
    /// file and then having nothing selected to copy again.
    pub fn refresh(&mut self) {
        self.keep_selected = match &self.dir {
            Some(dir) => self
                .selected
                .iter()
                .enumerate()
                .filter(|(_, &on)| on)
                .filter(|&(entry, _)| entry < dir.len())
                .map(|(entry, _)| dir.name(entry).to_owned())
                .collect(),
            None => std::mem::take(&mut self.keep_selected),
        };
        self.dir = None;
        self.awaiting = None;
        self.asked_at = None;
    }

    /// Whether this tab has been waiting for its listing long enough to say so.
    ///
    /// See [`SLOW_SCAN`]. `false` while nothing has been asked for, which is also the
    /// answer for the frame between a folder being wanted and being requested.
    pub fn waiting_visibly(&self, now: f64) -> bool {
        self.asked_at.is_some_and(|asked| now - asked >= SLOW_SCAN)
    }
}
