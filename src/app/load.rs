//! Asking for listings, and taking delivery of them.
//!
//! Every scan, icon, link target and thumbnail arrives here — on a channel, from a worker,
//! between frames. Nothing in this file blocks: a folder that takes twenty seconds to read is
//! twenty seconds of a window that still scrolls.

use super::*;

impl App {
    /// Hand every finished scan to the tab that asked for it.
    pub(super) fn collect_scans(&mut self) {
        // Collected first so the loader is not borrowed while the panes are.
        let arrived: Vec<_> = self.loader.drain().collect();
        let auto = self.auto_tiles;
        let Self { panes, providers, .. } = self;
        for loaded in arrived {
            for pane in panes.iter_mut() {
                for tab in &mut pane.tabs {
                    if tab.awaiting == Some(loaded.token) {
                        tab.apply(loaded.dir.clone());
                        // And, if this is a folder being *opened*, whether it is one to open as
                        // tiles — which only the listing that just arrived can say. See
                        // [`crate::pane::Tab::choose_view`]; every other reason a listing lands here
                        // is the same folder again, and leaves the view alone.
                        tab.choose_view(auto, providers);
                    }
                }
            }
        }
    }

    /// Count what is inside the folders on show, take the totals, and keep the shares in step.
    ///
    /// Everything the measure button costs after the click is here, and it is three things in the
    /// order they have to happen:
    ///
    /// 1. **Abandon what nobody is waiting for.** One call, with the generation of every tab still
    ///    measuring, which is the whole of the cancellation — see [`crate::sizes::Sizes::only`] for the
    ///    five ways a walk stops being wanted and why they are one rule.
    /// 2. **Ask about whatever is on show and has not been asked about**, which is
    ///    [`crate::sizes::Measurement::wanted`]'s answer — including the flattened case, which is
    ///    counted off its own listing with no disk at all. This layer only decides *where* the answer
    ///    goes; which kind of listing it is came out of the state that knows.
    /// 3. **Take the answers**, and settle the total the bars are a share of. See
    ///    [`crate::pane::Tab::settle_sizes`], which also re-sorts a Size-sorted listing — on a
    ///    deadline, because a rebuild per answer is a rebuild per frame.
    ///
    /// **Every tab, not just the active one**, which is where this parts company with
    /// [`App::collect_git`]: a background tab is not on screen, but it holds the numbers for its own
    /// folder and switching to it should show them rather than start over. The work is bounded by
    /// what each tab has actually asked for, and a tab that has never had the button pressed asks for
    /// nothing.
    ///
    /// # A re-read starts over, and that is bounded by step 1
    ///
    /// Every listing that lands throws the totals away, because a total names a row — see
    /// [`crate::sizes::Measurement::gen`] — so a build writing into a folder that is being measured
    /// has [`crate::watch`] re-reading it and this counting it again. What stops that being a
    /// treadmill is that the *previous* round is abandoned rather than left running: only one
    /// generation is ever live per tab, so at most one round of walks exists at a time and each of the
    /// old ones stops at its next directory read. The cost of a folder changing under a measurement is
    /// therefore one re-count, not one re-count per notification piled on top of the last.
    pub(super) fn collect_sizes(&mut self, ctx: &egui::Context, now: f64) {
        // Which measurements are still wanted. A tab whose button is off contributes nothing, which
        // is what makes turning it off a cancellation.
        //
        // **Handed over only when it has changed**, and that is not a micro-optimisation: `only` holds
        // the one lock all eight workers need in order to pop a directory or push its children, while
        // it retains over a queue that a listing of twenty thousand folder rows fills in one request.
        // Doing that sixty times a second to hand over a set that is almost always identical would
        // stall the walk to say nothing. Cancellation stays immediate, because the *difference* is the
        // cancellation.
        let live: Vec<u64> = self
            .panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .filter_map(|tab| tab.sizes.live())
            .collect();
        if live != self.measuring {
            self.sizes.only(&live);
            self.measuring = live;
        }

        for tab in self.panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
            // This PC has nothing to count — see `filelist::status_line`, where the button that says
            // so is drawn disabled. The same answer from the other end.
            if tab.path.as_os_str().is_empty() {
                continue;
            }
            match tab.wanted_sizes() {
                crate::sizes::Work::Walk(folders) => self.sizes.request(tab.sizes.gen(), folders),
                // A flattened listing, already summed off itself by the call above.
                crate::sizes::Work::Done | crate::sizes::Work::None => {}
            }
        }

        for answer in self.sizes.drain().collect::<Vec<_>>() {
            // By generation and not by path: two tabs on the same folder are two measurements, and an
            // answer belongs to the one that asked. `take` checks it, and stopping at the first match
            // is what `deliver_icons` and `deliver_links` beside this do — a generation is unique, so
            // there is never a second tab to offer it to.
            self.panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .any(|tab| tab.sizes.take(&answer, now));
        }

        // And the frame that would notice a re-sort has come due. Nothing else would ask for it once
        // the last answer has landed: this program is idle between events. The same booking
        // `start_scans` makes for `SLOW_SCAN`, and `breadcrumb::show` for the filter.
        let mut soonest: Option<f64> = None;
        for tab in self.panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
            if let Some(left) = tab.settle_sizes(now) {
                soonest = Some(soonest.map_or(left, |best: f64| best.min(left)));
            }
        }
        if let Some(left) = soonest {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }

    /// Hand each per-file icon answer to the view that asked for it, and drop the rest.
    ///
    /// This is where the folder-scoped rule is enforced. An answer names the view it belongs
    /// to; if no tab still holds that view — because it moved on, or was closed, or the
    /// folder was refreshed — the answer is discarded here and nothing anywhere remembers the
    /// file it was about. Nothing is keyed by path, so nothing outlives the folder.
    pub(super) fn deliver_icons(&mut self) {
        let answers = self.icons.answers();
        if answers.is_empty() {
            return;
        }
        for (view, row, index) in answers {
            let Some(tab) = self
                .panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .find(|tab| tab.view == view)
            else {
                continue;
            };
            if let Some(slot) = tab.file_icons.get_mut(row as usize) {
                *slot = index;
            }
        }
    }

    /// The same, for what each shortcut row points at. See [`crate::shell::links`].
    ///
    /// `None` is stored rather than skipped: it means the shortcut was read and had nothing to
    /// show, and storing it is what stops the row asking about it again on every frame.
    pub(super) fn deliver_links(&mut self) {
        let answers = self.links.answers();
        if answers.is_empty() {
            return;
        }
        for (view, row, target) in answers {
            let Some(tab) = self
                .panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .find(|tab| tab.view == view)
            else {
                continue;
            };
            // Only if the row is still asking. A refresh clears the map, and an answer that
            // arrives after that would otherwise put back a row's context for a listing the
            // entry indices no longer belong to.
            if let Some(slot) = tab.links.get_mut(&row) {
                *slot = target;
            }
        }
    }

    /// Apply whatever the modal thread came back with.
    ///
    /// **Nothing, and that is the point.** A shell command can do anything — rename, delete,
    /// extract, commit — and there is no way to be told which, so this used to re-read the folder
    /// on the way out of *every* one of them. Which meant a scan and a rebuilt listing after
    /// `Copy`, after `Properties`, after `Open with`, after `Scan with Defender`: the whole view
    /// thrown away and made again to discover that nothing had changed.
    ///
    /// [`crate::watch`] is what answers this properly, and it is already running. Every folder on
    /// screen has a `ReadDirectoryChangesW` handle on it, so a verb that *did* change something is
    /// noticed within a sixth of a second whoever changed it — this program, Explorer, a terminal,
    /// or the extension the verb belonged to — and a verb that changed nothing costs nothing.
    /// Still drained, because the reply is what tells [`crate::shell::Modal`] the gesture is
    /// over — and while one is in flight this window keeps painting for it.
    pub(super) fn collect_modal(&mut self) {
        match self.modal.poll() {
            Some(crate::shell::Reply::Invoked) | None => {}
        }
    }

    /// Watch the folders on screen, and re-read any that changed underneath us.
    ///
    /// A listing used to be only as fresh as the last thing *this* program did to it. Anything
    /// anybody else did went unseen: a file dragged out to Explorer stayed on screen, because
    /// Explorer performs the move after our drag has finished and there was nothing to wait on;
    /// a build writing into the folder showed it as it had been; a file deleted from a terminal
    /// left a row behind. And a stale row is worse than wrong — dragging one cannot start a
    /// drag, so the window looked like it had stopped responding.
    pub(super) fn collect_changes(&mut self, ctx: &egui::Context) {
        let mut folders: Vec<PathBuf> = self
            .panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .map(|tab| tab.path.clone())
            .collect();
        // **And every `.git` behind a folder on screen.** A commit, a pull, a branch switch or a
        // rebase changes what git says about a folder without changing the folder, so watching the
        // folder alone would leave the marks describing the last read and nothing to disprove them.
        // See [`crate::git::Repo::dot_git`].
        folders.extend(
            self.panes
                .iter()
                .flat_map(|pane| pane.tabs.iter())
                .filter_map(|tab| tab.git.as_ref())
                .map(|repo| repo.dot_git.clone()),
        );
        folders.dedup();
        self.watch.keep(&folders);

        // **Our own git write, echoing back as a change to notice.** `git::read` can write the
        // repository's index as a side effect of the very question we just asked it — see its own
        // doc — and that write is a rename of `index.lock` to `index`, directly inside `.git`. Two
        // watches can see it: the one on `.git` itself, which is meant to notice a commit made
        // elsewhere, and — because `ReadDirectoryChangesW` reports a direct child's own metadata
        // changing even without watching subtrees — the one on the *folder* too, since `.git` is a
        // direct child of it. Both read this exactly as they would read a real external change,
        // because the watch does not look at which file changed, by design.
        //
        // The second one is the dangerous one: a folder's own "changed" re-reads its listing, and
        // that re-read is what resets `git_asked` — see [`crate::pane::Tab::apply`]. So the folder's
        // watch answers our own write by asking git again, which writes again, which the watch reads
        // again — a folder that spawns `git status` under itself forever, on its own, whether or not
        // the window is even being looked at. This is the loop [`GIT_WRITE_SETTLE`] exists to break.
        let now = ctx.input(|i| i.time);
        for path in self.watch.changed(now) {
            // A change under `.git` asks git again and leaves the listing alone: the working tree
            // did not move, so re-reading the folder would be a scan for nothing.
            if path.file_name().is_some_and(|name| name == ".git") {
                for tab in self.panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
                    if tab.git.as_ref().is_some_and(|repo| repo.dot_git == path)
                        && now - tab.git_settled_at.unwrap_or(f64::NEG_INFINITY) > GIT_WRITE_SETTLE
                    {
                        tab.git_asked = false;
                    }
                }
                continue;
            }
            // **Except when a change to this folder is the thing being waited for.** A file the
            // user has just asked the shell to make is not our own git write coming back, and the
            // window the filter below suppresses is two whole seconds — long enough that, in any
            // folder git has something to say about, `New >` never appeared at all until something
            // else happened to touch the folder. Measured: git answered at 1.23 s and the file
            // landed before 2.57 s, so every single one was swallowed.
            //
            // It cannot restart the loop the filter is here to break, because the snapshot is
            // consumed by the very re-read this lets through — one extra read, once, and then the
            // filter applies again as before. See [`crate::pane::Tab::name_the_new`].
            let expected = self
                .panes
                .iter()
                .flat_map(|pane| pane.tabs.iter())
                .any(|tab| tab.path == path && tab.name_the_new.is_some());
            // The folder itself, echoing the same write back through its own watch. A real change to
            // this folder's own contents landing in the same short window is missed rather than
            // acted on immediately — recoverable by `F5`, or by the next thing that touches it — which
            // is the cheaper mistake next to a loop that never stops on its own.
            let echo = !expected
                && self.panes.iter().flat_map(|pane| pane.tabs.iter()).any(|tab| {
                    tab.path == path
                        && now - tab.git_settled_at.unwrap_or(f64::NEG_INFINITY) <= GIT_WRITE_SETTLE
                });
            if echo {
                continue;
            }
            self.folder_changed(&path);
        }
        // A change inside its settle window is a frame that has to come back for it, and there
        // is no input on the way to bring one.
        if self.watch.waiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
    }

    /// Re-read a folder that changed on disk, without blanking what is on screen.
    ///
    /// `Tab::refresh` is deliberately *not* used: it drops the listing, which puts "Reading..."
    /// in the pane until the scan lands. That is right for F5, where somebody asked; here it
    /// would flash on every file written into the folder being watched. So the old listing stays
    /// up and only the request is made — `Tab::apply` then carries the selection across by
    /// name, exactly as it does for a refresh.
    pub(super) fn folder_changed(&mut self, path: &Path) {
        self.loader.invalidate(path);
        let Self { panes, loader, .. } = self;
        for tab in panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
            if tab.path == path {
                // Replacing a token that is already out means the older answer is dropped when
                // it arrives, which is what should happen: it read the folder as it was.
                tab.awaiting = Some(ask_for(tab, loader));
            }
        }
    }

    /// Take delivery of finished file operations and re-read what they changed.
    pub(super) fn collect_operations(&mut self) {
        for done in self.ops.drain() {
            let worked = done.error.is_none();
            // Borrowed rather than taken, because the history is handed the whole `Done` at the
            // end of this body — deciding what Ctrl+Z does next needs both what the operation did
            // and whether it worked.
            if let Some(why) = done.error.as_deref().filter(|why| !why.is_empty()) {
                self.notice = Some(why.to_owned());
            }
            // The documented end of a cut, and only when the move actually happened.
            if let crate::shell::ops::After::FinishCut(was) = done.after {
                if worked {
                    crate::shell::clipboard::cut_pasted(was);
                    self.cut.clear();
                }
            }
            // A folder this program has just made: select it and open the name for editing as
            // soon as the re-read brings it in. `New folder` on its own is only half the
            // gesture; nobody wants a folder called `New folder`.
            if let crate::shell::ops::After::NameIt(pane) = done.after {
                if let (true, Some(name)) = (worked, done.outcome.created_name()) {
                    if let Some(p) = self.pane_mut(pane) {
                        let tab = p.tab_mut();
                        tab.reveal = Some(name);
                        tab.rename_revealed = true;
                    }
                }
            }
            for path in &done.touched {
                self.loader.invalidate(path);
            }
            // Only a tab showing an affected folder re-reads. A tab elsewhere is left
            // alone, which is the point of tracking this by path.
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    if done.touched.contains(&tab.path) {
                        tab.refresh();
                    }
                }
            }
            // What Ctrl+Z will take back, or — when this *was* a Ctrl+Z — which stack its entry
            // belongs on now. Both are one call, because both are answered by the same two things
            // the operation reported: what it did, and whether it worked.
            //
            // Last, and it takes the `Done` with it: an entry is the job and the outcome, and
            // handing them over beats copying them. See [`History::record`].
            self.history.record(done);
        }
        // A cut whose sources have gone is a cut that has been honoured.
        self.cut.retain(|path| path.exists());
    }

    /// Ask for anything nobody has asked for yet.
    ///
    /// The cache is probed synchronously first, which is what makes Back, Forward
    /// and revisiting a folder appear in the same frame as the click.
    pub(super) fn start_scans(&mut self, ctx: &egui::Context, now: f64) {
        let auto = self.auto_tiles;
        let Self { panes, loader, providers, .. } = self;
        let mut asked = false;
        for pane in panes.iter_mut() {
            for tab in pane.tabs.iter_mut() {
                if tab.dir.is_some() || tab.awaiting.is_some() {
                    continue;
                }
                // A flattened tab never looks in the cache, in either direction: the
                // cache holds the folder's own children under this very path, and handing
                // those over would put a shallow listing on screen with the button lit.
                // It is not put *in* the cache either — see [`Loader::request_deep`].
                if tab.flat {
                    tab.awaiting = Some(ask_for(tab, loader));
                    tab.asked_at = Some(now);
                    asked = true;
                    continue;
                }
                match loader.cached(&tab.path) {
                    Some(dir) => {
                        tab.apply(dir);
                        // The cache's answer is a listing arriving, exactly as a worker's is — and
                        // this is the path Back, Forward and a revisited folder take, which is
                        // rather more than half of the openings there are. Missing it here would
                        // make the rule work only on folders that happened to be cold.
                        tab.choose_view(auto, providers);
                    }
                    None => {
                        tab.awaiting = Some(ask_for(tab, loader));
                        // When it was asked for, which is what decides whether the listing
                        // says anything about waiting. See [`crate::pane::SLOW_SCAN`].
                        tab.asked_at = Some(now);
                        asked = true;
                    }
                }
            }
        }
        // And the frame that would notice the half-second has passed. Nothing else would ask
        // for it: this program is idle between events, and the answer arriving is the only
        // other thing that wakes it — so without this, `Reading…` would appear on a slow scan
        // only if something else happened to want a frame in the meantime.
        if asked {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(crate::pane::SLOW_SCAN));
        }
    }

    /// Back and forward on the mouse's thumb buttons.
    ///
    /// The two extra buttons every mouse past a certain price has. Windows sends them as
    /// `WM_XBUTTONDOWN` with `XBUTTON1` or `XBUTTON2`; winit turns those into `MouseButton::Back`
    /// and `Forward`, and egui into `PointerButton::Extra1` and `Extra2`. That is the whole chain,
    /// and it is worth writing down because "button 4 and 5" appear under four different names on
    /// the way through and `winit::MouseButton::Other` — which is what anything past the fifth
    /// button becomes — is dropped before egui ever sees it.
    pub(super) fn thumb_buttons(&mut self, ctx: &egui::Context) {
        use egui::PointerButton as B;

        let (back, forward) = ctx.input(|i| {
            (
                i.pointer.button_pressed(B::Extra1),
                i.pointer.button_pressed(B::Extra2),
            )
        });
        let pane = self.focused;
        if back {
            self.actions.push(Action::Back(pane));
        }
        if forward {
            self.actions.push(Action::Forward(pane));
        }
    }
}
