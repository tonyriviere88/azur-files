//! Taking delivery of a scan, and the order the rows end up in: the filter, the sort, the
//! grouping, and the tree's collapsed branches.

use super::*;

impl Tab {
    /// Take a finished listing and build the view over it.
    ///
    /// A *re-read* of the folder already being shown keeps the selection, by name. This is what
    /// a refresh has to do -- every file operation ends in one, so a selection that did not
    /// survive it meant copying a file and then finding nothing selected to copy again. Which is
    /// exactly what Ctrl+C, Ctrl+V, Ctrl+C did: the second copy had nothing to work with.
    /// [`Tab::go_to`] still clears everything, because a different folder is a different set of
    /// files.
    pub fn apply(&mut self, dir: Arc<Dir>) {
        // Whatever [`Tab::refresh`] put aside, plus the live selection when the listing is being
        // replaced under a tab that still has one.
        let mut held = std::mem::take(&mut self.keep_selected);
        if held.is_empty() {
            if let Some(old) = self.dir.as_ref().filter(|_| self.selected_count > 0) {
                held = self
                    .selected
                    .iter()
                    .enumerate()
                    .filter(|(_, &on)| on)
                    .filter(|&(entry, _)| entry < old.len())
                    .map(|(entry, _)| old.name(entry).to_owned())
                    .collect();
            }
        }

        // The row being renamed, remembered by *name* before the listing it is indexed into goes
        // away. `renaming` holds an entry index, and an index into the new listing may well mean
        // a different file — so left alone it is not a cosmetic problem: the field stays on
        // screen, stays committing, and commits the typed name onto whatever now sits at that
        // index.
        //
        // Which is not hypothetical and not rare, because the two halves collide by construction:
        // naming a file the shell has just made is a rename that *starts* on a re-read, and
        // making a file is exactly what has [`crate::watch`] asking for another one.
        let renaming = self.renaming.take().and_then(|(entry, text)| {
            let old = self.dir.as_ref()?;
            (entry < old.len()).then(|| (entry, old.leaf(entry).to_owned(), text))
        });

        self.selected = vec![false; dir.len()];
        self.file_icons = vec![crate::shell::icons::UNASKED; dir.len()];
        self.links.clear();
        // **A new listing is a new question for git**, and the old answer goes with the old listing
        // rather than being kept while it is checked. A tick that survives the read that would have
        // disproved it is the whole failure this design is avoiding.
        self.git = None;
        self.git_asked = false;
        self.git_answered = false;
        self.git_settled_at = None;
        self.selected_count = 0;
        self.selected_size = 0;
        self.cursor = None;
        self.anchor = None;
        self.widths_measured = false;
        self.dir = Some(dir);
        self.awaiting = None;
        self.asked_at = None;
        self.rebuild_order();

        // The rename field, put back on whichever row its file is now. Gone means gone: a file
        // renamed or deleted from under an open rename field has nothing left to commit onto, and
        // dropping it is what takes the field off the screen.
        if let Some((was, leaf, text)) = renaming {
            if let Some(dir) = self.dir.clone() {
                if let Some(&i) = self.order.iter().find(|&&i| dir.leaf(i as usize) == leaf) {
                    let entry = i as usize;
                    // The field's egui id is built from the entry index, so an index that moved is
                    // a different widget as far as egui is concerned, with none of the focus the
                    // old one had. Asking for it fresh is what puts the caret back in it. The text
                    // comes across either way -- it lives here, not in the widget -- so whatever
                    // was half-typed survives.
                    self.rename_fresh |= entry != was;
                    self.renaming = Some((entry, text));
                }
            }
        }

        // Whatever was selected and is still there. Anything that has gone -- moved, renamed,
        // deleted -- simply is not selected any more, which is the only sensible answer.
        if !held.is_empty() {
            if let Some(dir) = self.dir.clone() {
                for (position, &entry) in self.order.iter().enumerate() {
                    if held.iter().any(|name| name == dir.name(entry as usize)) {
                        self.selected[entry as usize] = true;
                        self.selected_count += 1;
                        self.selected_size += Self::size_at(Some(&dir), entry as usize);
                        self.cursor.get_or_insert(position);
                        self.anchor.get_or_insert(position);
                    }
                }
            }
        }

        // Highlight and scroll to whatever we came out of.
        let rename = std::mem::take(&mut self.rename_revealed);
        if let Some(name) = self.reveal.take() {
            if let Some(dir) = self.dir.clone() {
                // By leaf, so that a row revealed after a rename or a new folder is still
                // found in a flattened listing, where the name it is stored under carries
                // the folders in front of it. Identical to the name in every other
                // listing. Two files of the same name in different folders are two matches
                // and the first one wins, which is the same rule a duplicate name in one
                // folder would meet if the filesystem allowed one.
                if let Some(at) = self
                    .order
                    .iter()
                    .position(|&i| dir.leaf(i as usize) == name)
                {
                    self.select_only(at);
                    self.scroll_to_cursor = true;
                    // And it beats the top of the listing, which is where [`Tab::go_to`] asks a
                    // new folder to open. This is Back or Up, or a folder just created: the row
                    // being revealed is the whole reason for the move, and it can be row 500.
                    self.scroll_to = None;
                    if rename {
                        self.begin_rename();
                    }
                }
            }
        }

        // A file the shell has just made, whose name nothing told us: it is the row that was not
        // in the folder when the menu entry was chosen. Same ending as `rename_revealed` above —
        // selected, scrolled to, name open for editing — with the row arrived at by elimination
        // rather than handed over. See [`Tab::name_the_new`].
        //
        // Taken whether or not it finds anything, exactly as `reveal` is. A snapshot left lying
        // about would sit there until the folder next changed for any reason at all, and then open
        // a rename field on a file some other program had written, which is worse than missing the
        // one this was for.
        if let Some(before) = self.name_the_new.take() {
            if let Some(dir) = self.dir.clone() {
                // Searched over `order`, so a row a filter is hiding is not offered for renaming.
                // The snapshot itself is the whole listing rather than `order` — a hidden row is
                // still a file that was already there, and comparing against the visible ones only
                // would call it new.
                if let Some(at) = self
                    .order
                    .iter()
                    .position(|&i| !before.iter().any(|had| had == dir.name(i as usize)))
                {
                    self.select_only(at);
                    self.scroll_to_cursor = true;
                    self.scroll_to = None;
                    self.begin_rename();
                }
            }
        }
    }

    /// Every name in the listing, spelled the way [`Tab::apply`] compares them.
    ///
    /// The whole listing and not just the rows on show: a filter hides rows, and a hidden row is
    /// still a file that is already there. See [`Tab::name_the_new`], which is what wants this.
    pub fn names(&self) -> Vec<String> {
        match &self.dir {
            Some(dir) => (0..dir.len()).map(|e| dir.name(e).to_owned()).collect(),
            None => Vec::new(),
        }
    }

    /// Note that the filter text has changed, without applying it yet.
    ///
    /// Called on every keystroke; the last one before the pause is the only one that ends up
    /// doing any work. See [`FILTER_DELAY`] for what a pass costs and why that matters.
    pub fn filter_changed(&mut self, now: f64) {
        self.filter_at = Some(now);
    }

    /// Apply a filter whose keystrokes have stopped, or say how long is left until they have.
    ///
    /// Called once a frame. `Some(seconds)` means a rebuild is owed but not due, and the caller
    /// has to make sure there *is* a frame then — this program is idle between events, so
    /// without a repaint asked for, the filter would be applied whenever something else next
    /// happened to want a frame. `None` means there is nothing owed, either because nothing
    /// changed or because this call has just done it.
    pub fn settle_filter(&mut self, now: f64) -> Option<f64> {
        let at = self.filter_at?;
        let left = FILTER_DELAY - (now - at);
        if left > 0.0 {
            return Some(left);
        }
        self.filter_at = None;
        self.rebuild_order();
        // **A changed filter opens at the top**, for the same reason a different folder does in
        // [`Tab::go_to`]: row 200 of what one filter left is not row 200 of what the next one
        // leaves. Here it is sharper than that. Narrow a listing from five thousand rows to twelve
        // while scrolled halfway down it and there is no offset to keep — `ScrollArea` clamps to
        // the content it has, so what arrives is the *end* of the twelve, or the empty tail under
        // them. The rows a filter finds are worth looking at from the first one.
        //
        // This is `settle_filter` and not [`Tab::rebuild_order`] on purpose: every path into that
        // one rebuilds from the filter as it stands, and a column click or an `F5` has not changed
        // what the filter says. Losing your place in a listing you are still reading is what those
        // two must not do.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        None
    }

    /// Whether the listing is showing a question only git can answer. See [`Lens::Git`].
    pub fn filters_on_git(&self) -> bool {
        self.lens == Some(Lens::Git)
    }

    /// Re-apply the sort and the filter from scratch.
    pub fn rebuild_order(&mut self) {
        // Whatever was owed is paid by this: every path into here builds the order from the
        // filter as it stands, so a deadline left behind would only buy a second identical pass.
        self.filter_at = None;
        // A new order, whatever comes of it — including the empty one below. See [`Tab::order_gen`].
        self.order_gen = next_order_gen();
        let Some(dir) = self.dir.clone() else {
            self.order.clear();
            self.tree.clear();
            return;
        };
        // The half of the filter that is not about the name, as a test per row — see [`Lens`]. The
        // closures hold their own `Arc`s and their own answer, because the loop they are called from
        // borrows `self.order`.
        let names = dir.clone();
        let repo = self.git.clone();
        let by_lens: Option<Box<dyn Fn(usize) -> bool>> = match self.lens {
            None => None,
            // Three states and three different answers — see [`Tab::git_answered`].
            Some(Lens::Git) => match (self.git_answered, repo) {
                // Asked and not answered: the question cannot be evaluated, so it excludes nothing.
                // The answer arriving rebuilds this — `App::collect_git`.
                (false, _) => None,
                // Every row git has anything to say about, which is the same set the status line
                // counts as `N changed` — a folder included, because it wears the strongest state
                // beneath it and a folder filtered out is a folder you cannot open to reach what is
                // inside it.
                (true, Some(repo)) => Some(Box::new(move |entry| {
                    repo.state(names.name(entry))
                        .is_some_and(|state| state != crate::git::State::Clean)
                })),
                // Answered, and there is no repository here: nothing has changed, because there is
                // nothing that could have.
                (true, None) => Some(Box::new(|_| false)),
            },
            // Worked out once for the whole listing rather than per row, because the folders are not
            // a question about themselves: one is kept when a picture is somewhere under it, which is
            // a walk *up* from every picture. See [`sort::image_rows`].
            Some(Lens::Images) => {
                let kept = sort::image_rows(&dir, self.is_tree());
                Some(Box::new(move |entry| kept[entry]))
            }
        };
        // Remember what the cursor was pointing at, since its position moves.
        let cursor_entry = self.cursor.and_then(|at| self.order.get(at).copied());
        // The two flatten modes are two orders over one listing, and this is the only place that
        // knows which — see [`FlatMode`]. Everything either side of it is the same for both: the
        // same filter, the same lens, the same cursor put back on the same file.
        if self.is_tree() {
            let collapsed = std::mem::take(&mut self.collapsed);
            sort::build_tree_order(
                &dir,
                &mut self.order,
                &mut self.tree,
                self.sort_by,
                self.ascending,
                self.show_hidden,
                &self.filter,
                by_lens.as_deref(),
                // Taken out and put back rather than borrowed: the set and the order are both
                // fields of this tab, and the builder needs one while it fills the other.
                &|name| collapsed.contains(name),
                self.regroup,
            );
            self.collapsed = collapsed;
        } else {
            sort::build_order(
                &dir,
                &mut self.order,
                self.sort_by,
                self.ascending,
                self.show_hidden,
                &self.filter,
                by_lens.as_deref(),
            );
            // Nothing but a tree has a shape, and a stale one would outlive the listing it described.
            self.tree.clear();
        }
        self.cursor = cursor_entry.and_then(|entry| self.order.iter().position(|&i| i == entry));
        self.anchor = self.cursor;
    }

    /// Click a column header: toggle the direction if it is already the sort,
    /// otherwise switch to it in its natural direction.
    pub fn sort_by_column(&mut self, column: Column) {
        if self.sort_by == column {
            self.ascending = !self.ascending;
        } else {
            self.sort_by = column;
            self.ascending = column.starts_ascending();
        }
        self.rebuild_order();
    }

    /// The entry index at a position in the display order.
    #[inline]
    pub fn entry_at(&self, position: usize) -> Option<usize> {
        self.order.get(position).map(|&i| i as usize)
    }

    /// Where the row at `position` leads.
    pub fn target_at(&self, position: usize) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        Some(dir.target(self.entry_at(position)?))
    }

    /// Whether the row at `position` is a shortcut of either kind — a `.lnk` or a reparse
    /// point.
    ///
    /// From the enumeration alone, so it costs nothing: it says the row is *worth* reading,
    /// not what it leads to. [`crate::shell::links::folder_target`] is what answers that, and
    /// it is only ever asked of a row this returns `true` for.
    pub fn is_shortcut_at(&self, position: usize) -> bool {
        let Some(entry) = self.entry_at(position) else {
            return false;
        };
        let Some(dir) = &self.dir else { return false };
        crate::shell::links::kind_of(dir.ext(entry), dir.entries[entry].is_link()).is_some()
    }

    /// Whether the row at `position` is a folder — which decides whether opening
    /// it navigates or hands it to the shell.
    pub fn is_dir_at(&self, position: usize) -> bool {
        self.entry_at(position)
            .and_then(|i| self.dir.as_ref().map(|d| d.entries[i].is_dir()))
            .unwrap_or(false)
    }
}
