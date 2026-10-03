//! What is selected, where the cursor is, and everything that moves either.

use super::*;

impl Tab {

    #[inline]
    pub fn is_selected(&self, position: usize) -> bool {
        self.entry_at(position)
            .and_then(|i| self.selected.get(i).copied())
            .unwrap_or(false)
    }

    /// What entry `entry` adds to [`Tab::selected_size`]: its bytes, or nothing for a directory.
    ///
    /// Nothing, because a directory's `size` from the find data is noise — the details view leaves
    /// that cell blank for the same reason, and a folder counted as its own few hundred bytes would
    /// make the total of a selection wrong rather than incomplete.
    #[inline]
    pub(crate) fn size_at(dir: Option<&Arc<Dir>>, entry: usize) -> u64 {
        dir.and_then(|dir| dir.entries.get(entry))
            .filter(|e| !e.is_dir())
            .map_or(0, |e| e.size)
    }

    pub fn clear_selection(&mut self) {
        if self.selected_count > 0 {
            self.selected.iter_mut().for_each(|s| *s = false);
            self.selected_count = 0;
            self.selected_size = 0;
        }
    }

    /// One row, alone. A plain click.
    pub fn select_only(&mut self, position: usize) {
        self.clear_selection();
        if let Some(entry) = self.entry_at(position) {
            self.selected[entry] = true;
            self.selected_count = 1;
            self.selected_size = Self::size_at(self.dir.as_ref(), entry);
        }
        self.cursor = Some(position);
        self.anchor = Some(position);
    }

    /// Flip one row. Ctrl-click.
    pub fn toggle(&mut self, position: usize) {
        if let Some(entry) = self.entry_at(position) {
            let now = !self.selected[entry];
            let size = Self::size_at(self.dir.as_ref(), entry);
            self.selected[entry] = now;
            let (count, bytes) = if now {
                (self.selected_count + 1, self.selected_size + size)
            } else {
                (
                    self.selected_count.saturating_sub(1),
                    self.selected_size.saturating_sub(size),
                )
            };
            self.selected_count = count;
            self.selected_size = bytes;
        }
        self.cursor = Some(position);
        self.anchor = Some(position);
    }

    /// Everything between the anchor and here. Shift-click.
    pub fn select_range_to(&mut self, position: usize) {
        let from = self.anchor.unwrap_or(position);
        self.clear_selection();
        let (lo, hi) = (from.min(position), from.max(position));
        for at in lo..=hi.min(self.order.len().saturating_sub(1)) {
            if let Some(entry) = self.entry_at(at) {
                if !self.selected[entry] {
                    self.selected[entry] = true;
                    self.selected_count += 1;
                    self.selected_size += Self::size_at(self.dir.as_ref(), entry);
                }
            }
        }
        self.cursor = Some(position);
    }

    /// Apply a rubber-band's coverage to the selection.
    ///
    /// Recomputed from the band's base on every frame of the drag rather than
    /// accumulated, so shrinking the band deselects again — which is what makes a
    /// rubber band feel like a rubber band instead of a paintbrush.
    pub fn apply_band(&mut self) {
        let Some(band) = &self.band else { return };
        // Worked out before the reset, because both halves of this need `&mut self`. A `Range` rather
        // than a collection, so nothing is allocated: a band over a folder of 300,000 is two numbers.
        let covered = band.rows(self.order.len());
        self.apply_band_over(covered);
    }

    /// Apply a coverage the caller has worked out — which is what the **grid** hands over.
    ///
    /// A tile is a rectangle in two axes, so "which rows does the band's `y` cross" is not the question
    /// there: it is which cells the band's rectangle overlaps, and that depends on the column count and,
    /// in a tree, on which block each cell is in. Only [`crate::ui::grid`] knows where a tile went, so
    /// it answers and this applies — rather than the coverage rule being written twice in two languages.
    ///
    /// Generic over what comes in so both views reach it unchanged: the details view's `Range<usize>`
    /// and the grid's slice of positions.
    pub fn apply_band_over<I: IntoIterator<Item = usize>>(&mut self, covered: I) {
        if self.band.is_none() {
            return;
        }
        let dir = self.dir.clone();
        self.reset_to_band_base();
        for position in covered {
            self.band_select(dir.as_ref(), position);
        }
    }

    /// Put the selection back to what the band started from.
    ///
    /// The first half of a band pass, and the reason a band feels like a band rather than a
    /// paintbrush: the coverage is applied to *this* on every frame of the drag rather than
    /// accumulated, so shrinking the band deselects again.
    pub(crate) fn reset_to_band_base(&mut self) {
        let Some(band) = &self.band else { return };
        let base = &band.base;
        // The listing, taken out once: the loop below holds a borrow of `selected`, and an `Arc`
        // clone is a counter rather than a copy of the folder.
        let dir = self.dir.clone();
        self.selected_count = 0;
        self.selected_size = 0;
        for (entry, selected) in self.selected.iter_mut().enumerate() {
            *selected = base.get(entry).copied().unwrap_or(false);
            if *selected {
                self.selected_count += 1;
                self.selected_size += Self::size_at(dir.as_ref(), entry);
            }
        }
    }

    /// And the second half: one position the band covers.
    pub(crate) fn band_select(&mut self, dir: Option<&Arc<Dir>>, position: usize) {
        let Some(&entry) = self.order.get(position) else {
            return;
        };
        let entry = entry as usize;
        if !self.selected[entry] {
            self.selected[entry] = true;
            self.selected_count += 1;
            self.selected_size += Self::size_at(dir, entry);
        }
    }

    /// Begin renaming the row under the cursor.
    pub fn begin_rename(&mut self) {
        let Some(position) = self.cursor else { return };
        let Some(entry) = self.entry_at(position) else {
            return;
        };
        let Some(dir) = &self.dir else { return };
        // The file's own name, not the whole of what the row shows: in a flattened
        // listing a name is a relative path, and a rename field holding `sub\file.txt`
        // would be inviting the user to type a path into a rename.
        self.renaming = Some((entry, dir.leaf(entry).to_owned()));
        self.rename_fresh = true;
    }

    /// The name being edited, if this row is the one being renamed.
    pub fn rename_text(&mut self, entry: usize) -> Option<&mut String> {
        match &mut self.renaming {
            Some((at, text)) if *at == entry => Some(text),
            _ => None,
        }
    }

    pub fn select_all(&mut self) {
        self.clear_selection();
        let dir = self.dir.clone();
        for &entry in &self.order {
            self.selected[entry as usize] = true;
            self.selected_size += Self::size_at(dir.as_ref(), entry as usize);
        }
        self.selected_count = self.order.len();
    }

    /// The paths of everything selected, in display order.
    pub fn selection_paths(&self) -> Vec<PathBuf> {
        let Some(dir) = &self.dir else { return Vec::new() };
        self.order
            .iter()
            .filter(|&&i| self.selected.get(i as usize).copied().unwrap_or(false))
            .map(|&i| dir.target(i as usize))
            .collect()
    }

    /// Move the cursor, taking the selection with it unless `extend` is set.
    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        if self.order.is_empty() {
            return;
        }
        let last = self.order.len() as isize - 1;
        let from = self.cursor.map(|c| c as isize).unwrap_or(-1);
        let to = (from + delta).clamp(0, last) as usize;
        if extend {
            self.select_range_to(to);
        } else {
            self.select_only(to);
        }
        self.scroll_to_cursor = true;
    }

    pub fn move_cursor_to(&mut self, position: usize, extend: bool) {
        if self.order.is_empty() {
            return;
        }
        let position = position.min(self.order.len() - 1);
        if extend {
            self.select_range_to(position);
        } else {
            self.select_only(position);
        }
        self.scroll_to_cursor = true;
    }

    /// Open or shut the folder the cursor is on, for the `Left` and `Right` keys.
    ///
    /// Answers whether the tree changed, which is what tells `Left` apart from "shut already":
    /// there is nothing to close, so it means go out to the parent instead. Every other listing
    /// answers `false` for every row, so both keys are inert in one without being tested for.
    pub fn set_collapsed_at_cursor(&mut self, shut: bool) -> bool {
        let Some(at) = self.cursor else {
            return false;
        };
        // Whether it *is* already what is being asked for, taken first: `set_collapsed` answers
        // `false` for both "not a folder" and "already shut", and `Left` on a shut folder should
        // step out rather than sit there.
        self.set_collapsed(at, shut)
    }

    /// Put the cursor on the row of the folder the cursor's row is *in*, in a tree.
    ///
    /// `Left` on a file, or on a folder that is already shut — the way out of a branch. Answers
    /// whether it moved: `false` at the top level of the tree, where the folder the row is in is
    /// the folder being flattened and has no row of its own.
    ///
    /// Found by walking *up* the rows on screen for the first one drawn a level shallower, which is
    /// where the parent is in pre-order and costs the depth of the branch rather than a lookup
    /// table. A tree is at most a few dozen levels and the rows are `u32`s in a `Vec`.
    ///
    /// The depth is the one the row is **drawn** at — [`Tab::row_depth`] — and not the one its path
    /// has. Two merged chains side by side at the top of the tree are siblings however deep their
    /// paths run, and a search by path depth would have called the one above the parent of the one
    /// below.
    pub fn move_cursor_to_parent(&mut self) -> bool {
        if !self.is_tree() {
            return false;
        }
        let Some(at) = self.cursor else {
            return false;
        };
        let depth = self.row_depth(at);
        if depth == 0 || at >= self.order.len() {
            return false;
        }
        let Some(parent) = (0..at).rev().find(|&above| self.row_depth(above) < depth) else {
            return false;
        };
        self.move_cursor_to(parent, false);
        true
    }

    /// Jump to the next row whose name starts with what has been typed.
    ///
    /// `now` is the frame time; a gap longer than a second starts a fresh word,
    /// which is what makes typing `re` find `readme` but typing `r` a minute later
    /// start again from `r`.
    pub fn type_ahead(&mut self, ch: char, now: f64) {
        if now - self.typeahead_at > 1.0 {
            self.typeahead.clear();
        }
        self.typeahead_at = now;
        self.typeahead.extend(ch.to_lowercase());

        let Some(dir) = self.dir.clone() else { return };
        let needle = self.typeahead.as_str();
        // Start from the row after the cursor, so repeating a letter walks through
        // the matches instead of sticking on the first.
        let start = self.cursor.map(|c| c + 1).unwrap_or(0);
        let found = (0..self.order.len())
            .map(|offset| (start + offset) % self.order.len())
            .find(|&at| {
                let name = dir.name(self.order[at] as usize);
                name.len() >= needle.len()
                    && name
                        .chars()
                        .zip(needle.chars())
                        .all(|(a, b)| a.to_ascii_lowercase() == b)
            });
        if let Some(at) = found {
            self.select_only(at);
            self.scroll_to_cursor = true;
        }
    }
}
