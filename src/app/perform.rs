//! Where actions happen.
//!
//! [`App::apply`] drains the queue the frame filled and hands each one to [`App::perform`],
//! which is the only function in this program that restructures anything. Nothing is borrowed
//! by the time it runs, which is the whole reason the queue exists.

use super::*;

impl App {
    pub(super) fn apply(&mut self, ctx: &egui::Context) {
        // Files that finished coming out of an archive since the last frame. Queued as ordinary
        // actions so that opening one is the same code path as opening anything else — see
        // [`App::open_from_archive`].
        while let Ok(answer) = self.extracted.try_recv() {
            match answer {
                Extracted::Ready(path) => self.actions.push(Action::Open(path)),
                // The real paths, not the ones inside the archive: what goes on the clipboard has to
                // be something another program can open. A cut is never one of these — see
                // [`App::put_these_on_clipboard`], which refuses it.
                Extracted::Copied(paths) => self.put_these_on_clipboard(ctx, paths, false),
                // The drop that was waiting on it, taken from the top with real paths — see
                // [`App::land`], which is also what sent this. The one thing that has to be said
                // about re-entering it: the items are no longer virtual, so it cannot come back here
                // a third time.
                Extracted::Landed(dropped) => self.land(ctx, *dropped),
                Extracted::Failed(why) => self.report(why),
            }
        }

        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            self.perform(ctx, action);
        }
        // Written on the way out rather than on every drag frame; the cost of losing the last few
        // points of a sidebar width is nothing, and the cost of a file write per frame is a
        // stutter.
        //
        // **Rate-limited here rather than trusted to the call sites, because there are twenty-two
        // of them.** Two set the flag on every frame of a splitter drag — the sidebar's used
        // `dragged()`, which is true for frames with no movement at all — and each one is a read
        // plus up to two writes on the UI thread, on a path that may be a redirected `%APPDATA%`
        // on a share. One choke point cannot be forgotten by the next gesture that marks the
        // settings dirty; a rule at the sites can, and was.
        //
        // Nothing is lost by waiting: the flag stays set, so the next frame past the interval
        // writes it, and `on_exit` saves unconditionally whether or not one ever comes.
        const AT_MOST_EVERY: f64 = 0.5;
        let now = ctx.input(|i| i.time);
        if self.config_dirty && now - self.config_saved_at >= AT_MOST_EVERY {
            self.config_dirty = false;
            self.config_saved_at = now;
            // Through [`App::save_settings`], which is also what the exit save goes through: it
            // writes nothing when nothing this window holds has actually changed.
            self.save_settings();
        }
    }

    pub(super) fn perform(&mut self, ctx: &egui::Context, action: Action) {
        if let Some(journal) = &mut self.journal {
            journal.push(action.name());
        }
        // A folder diff's right half is a pane, but not a place in the layout: what it asks of the
        // tree goes to the pane it is drawn in, and what it has no switch for is not done. See
        // [`App::in_the_layout`].
        let action = self.in_the_layout(action);
        if self.refused_in_a_diff(&action) {
            return;
        }
        match action {
            Action::Focus(id) => {
                if self.panes.iter().any(|p| p.id == id) {
                    self.focused = id;
                }
            }

            Action::ActivateTab { pane, tab } => {
                if let Some(p) = self.pane_mut(pane) {
                    if tab < p.tabs.len() {
                        p.show_tab(tab);
                    }
                }
                self.focused = pane;
            }
            Action::NextTab { pane, delta } => {
                if let Some(p) = self.pane_mut(pane) {
                    let count = p.tabs.len() as isize;
                    p.show_tab((((p.active as isize + delta) % count + count) % count) as usize);
                }
            }
            Action::NewTab { pane } => {
                if let Some(p) = self.pane_mut(pane) {
                    let tab = p.tab().duplicate();
                    p.tabs.push(tab);
                    p.show_tab(p.tabs.len() - 1);
                }
                self.focused = pane;
                self.config_dirty = true;
            }
            Action::NewTabFocused => {
                let pane = self.focused;
                self.perform(ctx, Action::NewTab { pane });
            }
            Action::FolderDiff => self.open_folder_diff(),
            Action::DiffFolders { pane, left, right } => {
                let host = self.outer(pane);
                self.diff_folders(host, left, Some(right));
            }
            Action::SetDiffShow { pane, show } => self.set_diff_show(pane, show),
            Action::SplitFocused { side } => {
                let pane = self.focused;
                if let Some(path) = self.pane_mut(pane).map(|p| p.tab().path.clone()) {
                    self.perform(ctx, Action::OpenInSplit { pane, path, side });
                }
            }
            Action::CloseTab { pane, tab } => self.close_tab(ctx, pane, tab),
            Action::ReopenTab => {
                // Somewhere to put it, before taking it off the stack: the focused pane, or any
                // pane if focus is stale. Popping first and then finding nowhere to open it
                // would spend the entry and give nothing back.
                let Some(pane) = self
                    .panes
                    .iter()
                    .find(|p| p.id == self.focused)
                    .or_else(|| self.panes.first())
                    .map(|p| p.id)
                else {
                    return;
                };
                let Some(path) = self.closed.pop() else {
                    return;
                };
                self.perform(ctx, Action::NavigateNewTab { pane, path });
            }

            Action::BeginTabDrag { pane, tab, grab_dx } => {
                let title = self
                    .pane_mut(pane)
                    .and_then(|p| p.tabs.get(tab))
                    .map(|t| t.title.clone());
                if let Some(title) = title {
                    self.drag = Some(TabDrag {
                        pane,
                        tab,
                        grab_dx,
                        title,
                        live: false,
                    });
                }
            }

            Action::MoveTab {
                from,
                tab,
                to,
                index,
            } => self.move_tab(from, tab, to, index),

            Action::SplitTab {
                from,
                tab,
                target,
                side,
            } => {
                let Some(moved) = self.take_tab(from, tab) else {
                    return;
                };
                let id = self.spawn_pane(moved);
                if !self.layout.split(target, side, id) {
                    // The target vanished between the drop and now. Put the pane
                    // beside the focused one rather than losing the tab.
                    let anchor = self.focused;
                    self.layout.split(anchor, Side::Right, id);
                }
                self.focused = id;
                self.config_dirty = true;
            }

            Action::OpenInSplit { pane, path, side } => {
                let id = self.spawn_pane(Tab::showing(path, self.show_hidden));
                if !self.layout.split(pane, side, id) {
                    self.panes.retain(|p| p.id != id);
                    return;
                }
                self.focused = id;
                self.config_dirty = true;
            }

            Action::Navigate { pane, path } => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().navigate(path);
                }
                self.focused = pane;
                self.config_dirty = true;
            }
            Action::NavigateNewTab { pane, path } => {
                let hidden = self.show_hidden;
                if let Some(p) = self.pane_mut(pane) {
                    p.tabs.push(Tab::showing(path, hidden));
                    p.show_tab(p.tabs.len() - 1);
                }
                self.focused = pane;
                self.config_dirty = true;
            }
            Action::Back(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().go_back();
                }
            }
            Action::Forward(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().go_forward();
                }
            }
            Action::Up(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().go_up();
                }
            }
            Action::Refresh(pane) => {
                // Re-probe the volumes too: a full disk or an ejected card is exactly
                // the kind of thing someone presses F5 about. The probes are threads,
                // so this costs nothing here.
                self.volumes.refresh(ctx);
                let path = self.pane_mut(pane).map(|p| p.tab().path.clone());
                if let Some(path) = path {
                    // **F5 on a refused share is a request to be asked again**, possibly as
                    // somebody else. Without this the credential dialog is raised once per path per
                    // session and a cancel is permanent — see [`super::connect`], where the
                    // once-only rule is what stops a cancel from looping.
                    self.connecting.forget(&path);
                    self.loader.invalidate(&path);
                    // **And the archive behind it, if this is a folder inside one.** Dropping the
                    // listing alone would re-read it straight back out of the cached index, so F5
                    // inside a `.zip` would show exactly what it showed before — which is wrong in
                    // the one case somebody presses F5 for: the archive has been rebuilt since.
                    // Takes any path and does nothing for a path with no archive in it.
                    crate::archive::forget(&path);
                    if let Some(p) = self.pane_mut(pane) {
                        let tab = p.tab_mut();
                        // Keep the cursor where it was: a refresh should not move
                        // what you were looking at.
                        let keep = tab
                            .cursor
                            .and_then(|at| tab.entry_at(at))
                            .and_then(|i| tab.dir.as_ref().map(|d| d.name(i).to_owned()));
                        tab.refresh();
                        tab.reveal = keep;
                    }
                }
            }
            Action::EditPath(pane) => {
                let slashes = self.forward_slashes;
                if let Some(p) = self.pane_mut(pane) {
                    breadcrumb::start_editing(p.tab_mut(), slashes);
                }
            }

            Action::Sort { pane, column } => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().sort_by_column(column);
                }
            }
            Action::SelectAll(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().select_all();
                }
            }
            // Whether hidden files are rows, for every tab of every pane at once — the window's
            // preference like `SetFlatMode` and `SetRegroup` below, and written down like them.
            //
            // Nothing is re-read, for the reason those two are not: the walk always reports hidden
            // entries and it is the display that leaves them out. What is different is that this
            // changes which rows *exist* rather than how they are arranged, so the columns are
            // measured again — see [`crate::pane::Tab::set_show_hidden`].
            Action::ToggleHidden => {
                let on = !self.show_hidden;
                self.show_hidden = on;
                for p in &mut self.panes {
                    for tab in p.tabs.iter_mut() {
                        tab.set_show_hidden(on);
                    }
                }
                self.config_dirty = true;
            }
            // Unlike the other view toggles, this one changes what was *read* rather than
            // what is shown of it, so the listing goes and `start_scans` asks again.
            Action::ToggleFlat(pane) => {
                let (mode, regroup) = (self.flat_mode, self.regroup);
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().toggle_flat(mode, regroup);
                }
            }
            // And this one changes neither what was read nor how it is ordered: the rows, the sort,
            // the selection and the scroll are all untouched, and what changes is that a folder's
            // Size cell has a figure in it. The work it starts is `App::start_sizes`', on the next
            // frame, which is also what turning it off cancels. See [`crate::sizes`].
            Action::ToggleSizes(pane) => {
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                // Refused on This PC as well as drawn disabled there — see `filelist::status_line`
                // for why there is nothing to count — for the reason `Tab::toggle_flat` refuses: a
                // latched button over a listing that did not change. The Recycle Bin's items each
                // carry the size they were deleted at, which is the figure the bin has.
                if fs::is_synthetic(&tab.path) {
                    return;
                }
                let on = !tab.sizes.on;
                tab.set_sizes(on);
            }
            // And this one changes neither: both flatten modes are orders over the one listing the
            // walk already produced, so every tab showing a tree re-sorts and nothing is re-read.
            // See [`crate::pane::FlatMode`].
            //
            // **Every tab, not the pane the menu was opened on.** It is the window's preference,
            // it is written to the settings file, and a preference that applied to one pane would
            // leave the other one disagreeing with the tick in a menu that claims to be about
            // both. Tabs that are not flattened take the mode for the next time their button is
            // pressed, which is what `Tab::set_flat_mode` does for nothing.
            // Rows or tiles, for **this tab and no other** — and nothing is remembered: no window
            // preference, no settings key, and the next folder this tab opens is back in the details
            // view. See [`crate::pane::ViewMode`], which is where that argument lives, and
            // `SetFlatMode` just below for the preference this deliberately is not.
            //
            // The listing is not touched: both views are drawn over the same order, the same
            // selection and the same cursor. What does have to move is the *scroll*, because the
            // offset means a different place in each — row 40 of a listing and line 40 of a grid are
            // hundreds of files apart — so the view opens on the cursor if there is one and at the top
            // if there is not, which is the same rule a changed filter follows.
            Action::SetView { pane, mode } => {
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                if tab.view_mode == mode {
                    return;
                }
                tab.view_mode = mode;
                if tab.cursor.is_some() {
                    tab.scroll_to_cursor = true;
                } else {
                    tab.scroll_y = 0.0;
                    tab.scroll_to = Some(0.0);
                }
            }
            Action::SetFlatMode(mode) => {
                self.flat_mode = mode;
                for p in &mut self.panes {
                    for tab in p.tabs.iter_mut() {
                        tab.set_flat_mode(mode);
                    }
                }
                self.config_dirty = true;
            }
            // The same again for the other half of a tree's shape, and for the same reasons: every
            // tab, at once, nothing re-read — a merged chain is an order over the listing the walk
            // already produced. See [`crate::fs::sort::build_tree_order`].
            Action::SetRegroup(on) => {
                self.regroup = on;
                for p in &mut self.panes {
                    for tab in p.tabs.iter_mut() {
                        tab.set_regroup(on);
                    }
                }
                self.config_dirty = true;
            }
            // Whether a folder that is *opened* becomes tiles on its own, and at what share of
            // pictures. The window's preference like the two above and written down the same way —
            // and **the two panes are deliberately not touched**, which is the whole of what makes
            // this a rule about opening rather than a second view switch. See
            // [`crate::pane::AutoTiles`], and [`Action::SetAutoTiles`] for why that asymmetry with
            // the two above is the right way round.
            //
            // No listing is re-read either: `Tab::choose_view` reads the listing the tab already has
            // when the *next* folder lands, so there is nothing here to invalidate.
            Action::SetAutoTiles(on) => {
                self.auto_tiles.on = on;
                self.config_dirty = true;
            }
            Action::SetTilesThreshold(threshold) => {
                self.auto_tiles.threshold = crate::pane::AutoTiles::clamped(threshold);
                self.config_dirty = true;
            }
            // Which slash the path field writes. The window's preference like the two above, and
            // written down for the same reason — but this one has to rewrite what is already in a
            // field that is open, because that field is where the menu was just ticked: a setting
            // whose effect you have to close and reopen the field to see reads as a setting that
            // did nothing.
            //
            // Every tab, and the ones without a field open have nothing to rewrite. `\` and `/` are
            // both one byte and neither can appear in a Windows file name, so the swap is exact and
            // leaves the caret in front of the same character — see
            // [`crate::ui::breadcrumb::with_separator`], and [`PathComplete::rewritten`] for why the
            // completion has to be told the text moved without anybody typing.
            Action::SetForwardSlashes(on) => {
                self.forward_slashes = on;
                for p in &mut self.panes {
                    let id = p.id;
                    for tab in p.tabs.iter_mut() {
                        if !tab.editing_path {
                            continue;
                        }
                        tab.edit_text = breadcrumb::with_separator(&tab.edit_text, on);
                        self.complete.rewritten(id, &tab.edit_text);
                    }
                }
                self.config_dirty = true;
            }
            // A lens picked from the filter box's funnel, or the status line's `N changed` pressed.
            //
            // **More than one setting, because each alone answers half the question**: the lens over
            // a folder's own children finds only what is in *that* folder, and a flatten without the
            // lens is the whole tree with the answer buried in it. Together they are the listing that
            // was asked for — and for pictures the tiles are the third part of it, since a gallery
            // whose only column of interest is the name is what that view is for.
            //
            // Everything is *set* rather than toggled: pressing a button whose label is a fact about
            // the repository should not undo itself over a pane already showing the tree, and it is
            // the lens that is being changed on the second press. `toggle_flat` is still what turns
            // the flatten on, because doing so is a re-read and everything that comes with it lives
            // there.
            //
            // **Turning a lens off leaves the view where it is**, which is the one asymmetry here and
            // is deliberate: the flatten has its own button four points along the same bar and the
            // tiles their own switch on the status line, both latched to say so, and a menu entry that
            // quietly put back a view somebody may have changed by hand since would be undoing more
            // than it did.
            Action::SetLens { pane, lens } => {
                let (mode, regroup) = (self.flat_mode, self.regroup);
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                tab.lens = lens;
                if let Some(lens) = lens {
                    if !tab.flat {
                        tab.toggle_flat(mode, regroup);
                    }
                    if lens.wants_tiles() {
                        tab.view_mode = crate::pane::ViewMode::Icons;
                    }
                }
                // Straight away rather than through `filter_changed`: the delay there is for
                // keystrokes, and there are none — a menu entry arrives whole. A flatten that is
                // starting has no listing to rebuild yet, and the walk landing rebuilds from the
                // filter as it stands.
                tab.rebuild_order();
                tab.widths_measured = false;
                // Where a changed filter goes, and for the same reason — row 200 of the folder is not
                // row 200 of what the lens left, and in the tiles it is not even the same arithmetic.
                // See [`Action::SetView`], which is the other half of this pair.
                tab.scroll_y = 0.0;
                tab.scroll_to = Some(0.0);
            }
            Action::ToggleCollapsed { pane, position } => {
                let mut followed = None;
                if let Some(p) = self.pane_mut(pane) {
                    let tab = p.tab_mut();
                    // Read before the toggle, which rebuilds the order the position is into.
                    let shut = !tab.is_collapsed(position);
                    let name = tab.name_at(position).map(str::to_owned);
                    if tab.toggle_collapsed(position) && tab.diff.is_some() {
                        followed = name.map(|name| (name, shut));
                    }
                }
                // And the same folder on the other side of a diff, so the two trees move together.
                if let Some((name, shut)) = followed {
                    self.follow_collapse(pane, &name, shut);
                }
            }
            Action::TogglePreview(pane) => {
                let diffing = self.preview.diff;
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                if tab.preview.open {
                    tab.preview.close();
                } else {
                    // Opened with nothing previewable selected, the panel still opens and says
                    // what it would show. A shortcut that silently does nothing is a shortcut
                    // people conclude is broken.
                    // The cursor's file, not the tiling: the shortcut is about one file, and the
                    // next frame's `follow_all` spreads it out again if the selection is several.
                    // See [`crate::ui::preview::Preview::ask_for`].
                    match Self::selected_previews(tab, diffing, false).into_iter().next() {
                        Some(ask) => tab.preview.ask_for(ask),
                        None => tab.preview.open = true,
                    }
                }
                self.config_dirty = true;
            }
            Action::ClosePreview(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().preview.close();
                }
                // A video filling the screen belongs to the panel that was showing it, and the panel
                // has just gone. Nothing else has to be undone — the player went with the content —
                // but the *window* is still fullscreen, and a fullscreen window with a file listing
                // in it is not a state anybody asked for.
                if self.fullscreen_video == Some(pane) {
                    self.perform(ctx, Action::ToggleVideoFullscreen(pane));
                }
                self.config_dirty = true;
            }
            // **The window follows the state and not the other way round.** The flag is what the
            // drawing reads — see [`App::theatre`] — and the viewport command is sent from here so
            // that the four ways in and out cannot each have their own idea of what the window should
            // be doing. Not written to the settings file: nobody wants to reopen the program with a
            // video filling the screen.
            //
            // # The window is moved directly, not asked
            //
            // `ViewportCommand::Fullscreen` is the obvious way and it is wrong twice over for *this*
            // window — which carries `WS_MAXIMIZE` and sits on the monitor's work area, so the
            // platform keeps it clamped to the screen minus the taskbar. See `win::fill_screen`, which
            // has both failures and does the whole transition in one move that does not animate.
            Action::ToggleVideoFullscreen(pane) => {
                let filling = self.fullscreen_video != Some(pane);
                self.fullscreen_video = filling.then_some(pane);
                self.fill_screen(filling);
                // Nothing else asks for one: the window is about to change shape underneath a
                // program that is idle between events, and the frame that notices is this one's
                // successor.
                ctx.request_repaint();
            }
            // The shell is not started here and not stopped here. Opening the panel is the cheap
            // half — `console_panel` starts one on the frame it first has a rect to draw in, and
            // closing leaves the shell running, because a build you hid the panel to get out of the
            // way is a build you still want when you bring it back.
            Action::ToggleConsole(pane) => {
                let id = crate::ui::console::id(pane);
                let open = match self.pane_mut(pane) {
                    Some(p) => {
                        p.console_open = !p.console_open;
                        p.console_open
                    }
                    None => return,
                };
                if open {
                    // Opening one puts the keyboard in it — and takes it off any other pane's,
                    // because two panels both certain they own the keyboard would both read the same
                    // keystroke.
                    for other in &mut self.panes {
                        other.console_state.drop_keys();
                    }
                    let shell = self.console_shell;
                    if let Some(p) = self.pane_mut(pane) {
                        p.console_state.take_keys();
                        // The shell this window was last working in — but only when there is no
                        // session yet, since changing the kind is what replaces one.
                        if p.console.is_none() {
                            p.console_state.set_kind(shell);
                        }
                    }
                    ctx.memory_mut(|m| m.request_focus(id));
                } else {
                    // The panel is gone, so what was holding the keyboard inside it cannot still be,
                    // or the listing stays deaf with nothing on screen to explain why.
                    if let Some(p) = self.pane_mut(pane) {
                        p.console_state.drop_keys();
                    }
                    ctx.memory_mut(|m| m.surrender_focus(id));
                }
                self.config_dirty = true;
            }
            // The panel down the left, for the whole window. Nothing to undo and nothing to tell:
            // where the panes go is worked out from this every frame — see [`App::split_body`] — so
            // the next frame is the whole of the change.
            Action::ToggleSidebar => {
                self.sidebar_shown = !self.sidebar_shown;
                self.config_dirty = true;
            }
            Action::RememberLayout => self.config_dirty = true,

            Action::Cut(pane) => self.put_on_clipboard(ctx, pane, true),
            Action::Copy(pane) => self.put_on_clipboard(ctx, pane, false),
            Action::Paste(pane) => self.paste_into(pane, ctx),
            // The context menu's Couper, Copier and Coller, which name what they act on rather
            // than reading it off a pane. See `ours_rather_than_the_shell_s`.
            Action::CutItems(items) => self.put_these_on_clipboard(ctx, items, true),
            Action::CopyItems(items) => self.put_these_on_clipboard(ctx, items, false),
            Action::PasteIntoFolder(into) => self.paste_into_folder(into, ctx),
            Action::Delete { pane, permanent } => {
                let items = self
                    .pane_mut(pane)
                    .map(|p| p.tab().selection_paths())
                    .unwrap_or_default();
                if items.is_empty() {
                    self.notice = Some("Nothing selected".to_owned());
                    return;
                }
                // **In the Recycle Bin, Delete is the bin's own Delete**, which is permanent — with
                // or without Shift, as in Explorer, since there is nowhere further for it to go. Not
                // a job: the rows are `$R…` files, and `IFileOperation` on one would remove it and
                // leave the `$I…` that describes it. The bin's verb removes both, and asks first.
                // It goes the way a menu entry would, on the modal thread, and the watcher on the
                // bin's folders is what brings the listing up to date. See [`crate::fs::recycle`].
                if items.iter().all(|item| fs::recycle::is_held(item)) {
                    self.bin_verb(items, "delete");
                    return;
                }
                // No confirmation of our own: the shell asks, and being asked twice
                // about the same thing is how a prompt becomes something people click
                // through without reading.
                self.ops.start(
                    crate::shell::ops::Job::Delete {
                        items,
                        to_bin: !permanent,
                    },
                    self.owner,
                    ctx,
                );
            }
            // Ctrl+Z and Ctrl+Y. The history decides what the job is; this only starts it, and
            // tells the user when there was nothing to start.
            //
            // `After::Settle` is what closes the loop: the job runs on its own thread like every
            // other, and the entry it came from does not move to the other stack until the shell
            // reports back — see [`crate::shell::ops::history::History::record`].
            Action::Undo { redo } => {
                let job = if redo {
                    self.history.redo()
                } else {
                    self.history.undo()
                };
                match job {
                    Some(job) => {
                        self.notice = None;
                        self.ops.start_then(
                            job,
                            crate::shell::ops::After::Settle,
                            self.owner,
                            ctx,
                        );
                    }
                    None => self.notice = Some(self.history.why_not(redo).to_owned()),
                }
            }
            Action::BeginRename(pane) => {
                // Refused before the editor opens rather than after it is filled in. The rename
                // would be refused either way — [`crate::shell::ops::Jobs::start_then`] is the guard
                // that matters — but letting somebody type a new name and press Enter to be told
                // "no" is a worse way to say it than not offering the box.
                let inside = self
                    .pane_mut(pane)
                    .map(|p| crate::archive::is_virtual_location(&p.tab().path))
                    .unwrap_or(false);
                if inside {
                    self.report("Files inside an archive cannot be renamed".to_owned());
                    return;
                }
                // Nor in the bin: the name on the row is the one it will get back, and renaming the
                // `$R…` file under it would break the pair the bin restores from. See
                // [`crate::fs::recycle`].
                let binned = self
                    .pane_mut(pane)
                    .is_some_and(|p| fs::recycle::is_bin(&p.tab().path));
                if binned {
                    self.report(fs::recycle::RESTORE_FIRST.to_owned());
                    return;
                }
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().begin_rename();
                }
            }
            Action::CommitRename { pane, name } => {
                let owner = self.owner;
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                let Some((entry, _)) = tab.renaming.take() else {
                    return;
                };
                let Some(dir) = tab.dir.clone() else { return };
                let name = name.trim().to_owned();
                // Against the leaf, which is what the field was seeded with — in a
                // flattened listing the entry's *name* is a relative path, and comparing
                // against that would make every rename look like a change.
                if name.is_empty() || name == dir.leaf(entry) {
                    return;
                }
                // Checked here rather than left to the shell. See [`crate::fs::why_not_a_name`] —
                // the separator is the one that matters, because a name containing one is a path
                // and asks for something other than a rename.
                if let Some(why) = crate::fs::why_not_a_name(&name) {
                    self.notice = Some(why);
                    return;
                }
                let item = dir.target(entry);
                // Selected again once the folder is re-read, so the renamed file is
                // still the thing you were looking at.
                tab.reveal = Some(name.clone());
                self.ops
                    .start(crate::shell::ops::Job::Rename { item, name }, owner, ctx);
            }
            Action::CancelRename(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().renaming = None;
                }
            }
            Action::BeginKeywords { pane, entry } => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().begin_keywords(entry);
                }
            }
            Action::CommitKeywords { pane, text } => {
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                let Some((entry, key, _)) = tab.keywords.take() else {
                    return;
                };
                // Where the file is now, for the store's own record of it — see
                // [`crate::fs::keywords`]. The key is what is kept; the path is for a person.
                let path = tab
                    .dir
                    .as_ref()
                    .filter(|dir| dir.key(entry) == Some(key))
                    .map(|dir| dir.target(entry))
                    .unwrap_or_default();
                if !fs::keywords::set(key, &text, &path) {
                    return;
                }
                // Every listing on show whose order is *made of* keywords is now out of date, in any
                // pane — the same file can be on show in two. The others only draw them, and draw
                // them from the store next frame. The column is re-fitted where it is on show, so a
                // longer set is not elided by a width chosen before it existed.
                for pane in &mut self.panes {
                    for tab in &mut pane.tabs {
                        if tab.dir.as_ref().is_some_and(|dir| dir.volume == Some(key.volume)) {
                            tab.widths_measured = false;
                            if tab.sort_by == crate::fs::Column::Keywords {
                                tab.rebuild_order();
                            }
                        }
                    }
                }
            }
            Action::CancelKeywords(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().keywords = None;
                }
            }
            Action::NewFolder(pane) => {
                let owner = self.owner;
                let Some(parent) = self.pane_mut(pane).map(|p| p.tab().path.clone()) else {
                    return;
                };
                if fs::is_synthetic(&parent) {
                    self.notice = Some(format!(
                        "{} is not a folder to create in",
                        fs::display_name(&parent)
                    ));
                    return;
                }
                // The shell picks a free name from this one, so "New folder (2)" and the
                // rest come out right without this program having to count — and reports back
                // which it chose, so the row can be named the moment it appears.
                let name = "New folder".to_owned();
                self.ops.start_then(
                    crate::shell::ops::Job::NewFolder { parent, name },
                    crate::shell::ops::After::NameIt(pane),
                    owner,
                    ctx,
                );
            }
            Action::DropHere {
                pane,
                items,
                into,
                effect,
            } => {
                use crate::shell::clipboard::Effect;
                self.focused = pane;
                let job = match effect {
                    Effect::Move => crate::shell::ops::Job::Move { items, into },
                    Effect::Copy => crate::shell::ops::Job::Copy { items, into },
                    Effect::Link => crate::shell::ops::Job::Link { items, into },
                };
                self.ops.start(job, self.owner, ctx);
            }
            Action::DragOut { pane, items } => {
                // Out of the bin, the only thing a drag could carry is the `$R…` file, and wherever
                // it landed would take the file and leave its entry behind. See
                // [`crate::shell::ops::Job::bin_refusal`], which is the same rule for a paste.
                if items.iter().any(|item| fs::recycle::is_held(item)) {
                    self.report(fs::recycle::RESTORE_FIRST.to_owned());
                    return;
                }
                // **A selection inside an archive drags out like any other**, and nothing here has to
                // know that. `CF_HDROP` cannot carry a path that names no file, so the drag source
                // offers Windows' virtual-file formats instead and decompresses at the drop rather
                // than at the gesture — see [`crate::windows::dnd`]'s `virtual_files`, where the
                // whole mechanism lives. The one thing this arm must *not* do is extract anything
                // first: that would put the cost back on the UI thread, which is what the data
                // object exists to avoid.
                //
                // Started here and now rather than parked for later: the drag has its own
                // thread, so nothing about it re-enters this pass. One at a time, since the
                // second would be following a button the first is already holding — and only
                // with a window, because the drag joins *this* thread's input queue to find
                // the button it is following and a thread with no window has no gesture to
                // follow. That last one is also what keeps the tests off the real pointer.
                if self.file_drag.is_none() && self.owner.0 != 0 {
                    // Cloned because the window keeps its own list: the ghost under the pointer is
                    // drawn from it for as long as the drag runs, and the copy handed to OLE belongs
                    // to the drag's own thread.
                    //
                    // The drop target goes with it, because a drag inside this window has both ends
                    // in it: the cursor is the source's to set and what it should be is the target's
                    // to know. See [`crate::shell::dnd::Shared::silent`].
                    self.file_drag = crate::shell::dnd::drag_out(items.clone(), &self.drops)
                        .map(|drag| super::Dragging::new(pane, items, drag));
                }
            }
            Action::ShellMenu {
                pane,
                items,
                at,
            } => self.shell_menu(pane, items, at, ctx),
            // **An archive opens here too, as a folder.** `pkg.zip` is a file to the operating
            // system and a place to this program: the same double click that steps into a directory
            // steps into one of these, which is what Explorer has always done for a `.zip` and what
            // [`crate::archive`] does for eleven more formats. The test is on the extension alone
            // and touches no disk — a folder that happens to be *named* `stuff.zip` never arrives
            // here, because the listing already knows it is a directory and sent a `Navigate`.
            //
            // A file **inside** an archive is the other half, and it cannot be opened where it is:
            // there are no bytes behind its path until something decompresses them. That is a
            // thread's work, so it goes to one, and the answer comes back around to this same arm
            // with a real path. See [`App::open_from_archive`].
            // **A file in the Recycle Bin is not opened**, which is Explorer's rule too: a double click
            // there shows the item's Properties — where it came from, when it went — through the
            // bin's own verb. Opening the `$R…` file would hand a deleted program to `ShellExecute`,
            // and *run* it. A deleted folder never gets here: the listing knows it is a directory
            // and sends a `Navigate`, which is how you look inside one before putting it back.
            Action::Open(path) if fs::recycle::is_held(&path) => self.bin_verb(vec![path], "properties"),
            Action::Open(path) => match crate::archive::split(&path) {
                Some(inside) if inside.is_root() => {
                    let pane = self.focused;
                    self.perform(ctx, Action::Navigate { pane, path });
                }
                Some(_) => self.open_from_archive(ctx, path),
                // **A shortcut to a folder opens here, not in Explorer.** Handing it to the shell
                // is what `.lnk` files get by default, and for a folder that means a second file
                // manager opening over the top of this one — which is not what clicking a row in
                // this window can be allowed to do. Every route into this arm gets it: a double
                // click, `Enter`, and a path typed into the bar.
                //
                // A directory *reparse point* — a junction or a directory symlink — never comes
                // through here at all: the enumeration reports it as a directory, so it is a
                // `Navigate` before this is reached.
                //
                // The pane is the focused one because that is where the gesture was: a click on a
                // row focuses its pane first, and `Enter` acts on the focused pane by definition.
                None => match crate::shell::links::folder_target(&path) {
                    Some(folder) => {
                        let pane = self.focused;
                        self.perform(ctx, Action::Navigate { pane, path: folder });
                    }
                    None => fs::shell::open(&path),
                },
            },
            // The same, in a tab of its own — a middle click on a folder shortcut. A shortcut to
            // a *file* does nothing here rather than opening it somewhere it cannot be shown: a
            // new tab is a place, and a file is not one.
            Action::OpenNewTab(path) => {
                let pane = self.focused;
                // An archive is a place, so it gets a tab of its own from a middle click exactly as
                // a folder does. A *file inside* one is not a place and is left alone here, which is
                // the same silence a middle click on any other file gets — see the arm above, where
                // that distinction is `is_root`.
                if crate::archive::browsable(&path) {
                    self.perform(ctx, Action::NavigateNewTab { pane, path });
                } else if let Some(folder) = crate::shell::links::folder_target(&path) {
                    self.perform(ctx, Action::NavigateNewTab { pane, path: folder });
                }
            }
            // **Both of these are asked of the operating system, so both take the archive instead of
            // what is inside it.** Explorer cannot select an entry that is not a file and a shell has
            // nowhere to start in a folder that is not a directory; handing either a virtual path
            // fails with a shrug. The archive *is* on a real disk, and revealing it or opening a
            // terminal beside it is the nearest true answer to what was asked — and better than a
            // greyed-out entry, because it is the thing the user would have picked next anyway.
            Action::Reveal(path) => fs::shell::reveal(&nearest_real(&path)),
            Action::OpenTerminal(path) if fs::is_synthetic(&path) => {
                self.report(format!("{} is not a folder a terminal can start in", fs::display_name(&path)));
            }
            Action::OpenTerminal(path) => fs::shell::open_terminal(&nearest_real(&path)),
            // The paths as text, one per line — `Ctrl+Shift+C`, and the context menu's
            // `Copy path(s)`, which is this same action so that the two cannot drift.
            //
            // **With whichever slash the path field is set to write**, because a path being copied
            // out is what that setting is for — see [`crate::config::Config::forward_slashes`]. One
            // `replace` over the whole line is exact either way: neither slash can appear in a
            // Windows file name, so every one of them is a separator. See
            // [`crate::ui::breadcrumb::with_separator`].
            //
            // `\r\n` between them, which is what the clipboard means by a line on this platform: a
            // list pasted into Notepad, `cmd` or an editor comes out as lines rather than as one.
            Action::CopyPaths(paths) => {
                let slashes = self.forward_slashes;
                let text = paths
                    .iter()
                    .map(|p| breadcrumb::with_separator(&p.to_string_lossy(), slashes))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                ctx.copy_text(text);
            }
            // Every one of these says whether it changed anything, and that is what marks the
            // settings dirty: a drag that ended where it started, a folder already pinned, a
            // group index from a stale menu — all of them are no-ops, and none of them is
            // worth writing the file for.
            Action::AddBookmark(path) => self.config_dirty |= self.bookmarks.add(path),
            Action::AddBookmarkIn { group, path } => {
                self.config_dirty |= self.bookmarks.add_in(group, path);
            }
            Action::RemoveBookmark(path) => self.config_dirty |= self.bookmarks.remove(&path),
            Action::MoveBookmark { from, to } => {
                self.config_dirty |= self.bookmarks.move_to(from, to);
            }
            Action::ToggleBookmark(path) => {
                self.config_dirty |= if self.is_bookmarked(&path) {
                    self.bookmarks.remove(&path)
                } else {
                    self.bookmarks.add(path)
                };
            }
            // Nothing is written to the settings: what a browse found is about the network at the
            // moment it was asked, not a preference, and a machine that has gone quiet should not
            // come back next launch as a row that leads nowhere.
            Action::DiscoverNetwork => self.volumes.discover(ctx),
            Action::AddBookmarkGroup => {
                // Made and named in one gesture: the `+` is pressed, the group appears at the
                // end of the list with its name in a field, and typing over `New group` is the
                // rest of it. A dialog for one word would be a dialog too many, and a group
                // called `New group` because nobody was asked is a group nobody can find.
                self.sections.bookmarks = true;
                let group = self
                    .bookmarks
                    .add_group(crate::ui::sidebar::bookmarks::NEW_GROUP, true);
                self.bookmark_edit.rename = Some(crate::ui::sidebar::Rename::new(
                    group,
                    crate::ui::sidebar::bookmarks::NEW_GROUP.to_owned(),
                ));
                self.config_dirty = true;
            }
            Action::ToggleBookmarkGroup(group) => {
                self.config_dirty |= self.bookmarks.toggle_group(group);
            }
            Action::BeginRenameBookmarkGroup(group) => {
                if let Some(existing) = self.bookmarks.group(group) {
                    self.bookmark_edit.rename =
                        Some(crate::ui::sidebar::Rename::new(group, existing.name.clone()));
                }
            }
            Action::CommitRenameBookmarkGroup { group, name } => {
                self.bookmark_edit.rename = None;
                self.config_dirty |= self.bookmarks.rename_group(group, &name);
            }
            Action::CancelRenameBookmarkGroup => self.bookmark_edit.rename = None,
            Action::UngroupBookmarks(group) => {
                self.bookmark_edit.rename = None;
                self.config_dirty |= self.bookmarks.ungroup(group);
            }
            Action::RemoveBookmarkGroup(group) => {
                // The field goes with the row it was over. Without this, renaming a group and
                // removing it in the same breath leaves a field editing a group that is not
                // there — and, worse, one whose index now names its neighbour.
                self.bookmark_edit.rename = None;
                self.config_dirty |= self.bookmarks.remove_group(group);
            }
            Action::SetTheme(palette) => {
                if palette == self.theme.palette {
                    return;
                }
                self.theme = Theme::of(palette);
                // The style has to be reinstalled, and only then — installing it every
                // frame would throw away egui's galley and shape caches.
                self.installed = false;
                self.config_dirty = true;
            }

            Action::SetWinKey(on) => {
                // Nothing is marked dirty and nothing reaches `config.ini`: the registry is the
                // setting. And the tick is set from what the registry says *afterwards* rather than
                // from what was asked for — a write refused by policy has to leave it where it was,
                // which is the whole reason `set` answers with a state rather than with `()`.
                match crate::shell::winkey::set(on) {
                    Ok(state) => self.win_key = state,
                    Err(why) => self.report(why),
                }
            }
            // From the next copy on: one already running stays with the engine it started on.
            Action::SetFastCopy(on) => {
                self.ops.set_fast(on);
                self.config_dirty = true;
            }
            Action::Steer { transfer, steer } => self.ops.steer(transfer, steer),
            Action::Leave(leave) => {
                use crate::ui::transfers::Leave;
                self.closing = match leave {
                    Leave::Stay => Closing::No,
                    Leave::WhenDone => Closing::WhenDone,
                    Leave::StopAndClose => {
                        self.ops.cancel_copies();
                        Closing::WhenDone
                    }
                };
            }

            Action::Window(what) => {
                use egui::ViewportCommand as Cmd;
                match what {
                    WindowAction::Minimize => ctx.send_viewport_cmd(Cmd::Minimized(true)),
                    WindowAction::ToggleMaximize => {
                        ctx.send_viewport_cmd(Cmd::Maximized(!self.maximized));
                    }
                    // Every monitor at once, which the platform has no command for: its maximise is
                    // one monitor by definition. So the window is moved directly, the same way and
                    // for the same reasons a video filling the screen is — see `win::span_screens`.
                    //
                    // The flag is set from here rather than left to the frame that observes the new
                    // shape, for the reason `ResetSize` below gives: a window closed in between would
                    // write the wrong one.
                    WindowAction::SpanScreens => {
                        if self.span_screens() {
                            self.maximized = false;
                            self.config_dirty = true;
                            // The window is about to change shape underneath a program that is idle
                            // between events, and the frame that notices is this one's successor.
                            ctx.request_repaint();
                        }
                    }
                    WindowAction::ResetSize => {
                        let [w, h] = crate::config::WINDOW_SIZE;
                        // Un-maximised first, and said out loud rather than relied on. On
                        // Windows an `InnerSize` alone is enough — measured: from a maximised
                        // 2560×1392 the window comes back to 1024×600 with this line taken out,
                        // because `SetWindowPos` on a maximised window restores it on the way.
                        // That is winit's platform behaviour and not a promise, and asking for
                        // the state this wants costs one command.
                        if self.maximized {
                            ctx.send_viewport_cmd(Cmd::Maximized(false));
                        }
                        ctx.send_viewport_cmd(Cmd::InnerSize(egui::vec2(w, h)));
                        // Both remembered now rather than left to the frame that observes the
                        // new shape. That frame does set them, so this is belt and braces for
                        // the window being closed in between — which would otherwise save the
                        // size and the maximised flag this has just replaced.
                        self.maximized = false;
                        self.window_size = Some([w, h]);
                        self.config_dirty = true;
                    }
                    WindowAction::Close => ctx.send_viewport_cmd(Cmd::Close),
                    WindowAction::Drag => ctx.send_viewport_cmd(Cmd::StartDrag),
                }
            }
        }
    }
}

/// The nearest path the operating system will accept: the path itself, or the **archive** holding it.
///
/// For the two actions that are questions for the shell rather than for this program — Reveal and
/// Open terminal. Neither can be answered about a path inside an archive, because there is no such
/// file and no such directory, and both have a sensible true answer one level out: the archive is a
/// real file in a real folder. See the arms that use it.
impl App {
    /// Run one of the Recycle Bin's own verbs on items it holds, the way a menu entry would be run:
    /// on the modal thread, against the menu the bin gives for them. See
    /// [`crate::shell::ops::bin::held_menu`], and [`crate::fs::recycle`] for why its items are
    /// never handed to anything else.
    fn bin_verb(&mut self, items: Vec<std::path::PathBuf>, verb: &str) {
        self.modal.send(crate::shell::Request::Invoke {
            parent: fs::recycle::location(),
            items,
            command: crate::shell::menu::Command::verb_only(verb),
            depth: crate::shell::menu::Depth::Full,
            owner: self.owner,
        });
    }
}

fn nearest_real(path: &std::path::Path) -> std::path::PathBuf {
    // An item in the bin is revealed in the bin: `explorer /select,` on the `$R…` file would open
    // the raw `$Recycle.Bin` folder, mangled names and all.
    if fs::recycle::is_held(path) {
        return fs::recycle::location();
    }
    // Confirmed rather than guessed from the extension, or a real folder named `stuff.zip` would
    // have Reveal point Explorer at the folder instead of at the file inside it that was asked for.
    match crate::archive::inside_archive(path) {
        Some(inside) => inside.file,
        None => path.to_path_buf(),
    }
}
