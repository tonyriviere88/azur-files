//! Where actions happen.
//!
//! [`App::apply`] drains the queue the frame filled and hands each one to [`App::perform`],
//! which is the only function in this program that restructures anything. Nothing is borrowed
//! by the time it runs, which is the whole reason the queue exists.

use super::*;

impl App {
    pub(super) fn apply(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            self.perform(ctx, action);
        }
        if self.config_dirty {
            // Written on the way out rather than on every drag frame; the cost of
            // losing the last few points of a sidebar width is nothing, and the cost
            // of a file write per frame is a stutter.
            self.config_dirty = false;
            self.config = self.settings();
            self.config.save();
        }
    }

    pub(super) fn perform(&mut self, ctx: &egui::Context, action: Action) {
        if let Some(journal) = &mut self.journal {
            journal.push(action.name());
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
                let id = self.spawn_pane(Tab::new(path));
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
                if let Some(p) = self.pane_mut(pane) {
                    p.tabs.push(Tab::new(path));
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
                    self.loader.invalidate(&path);
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
            Action::ToggleHidden(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    let tab = p.tab_mut();
                    tab.show_hidden = !tab.show_hidden;
                    tab.rebuild_order();
                    tab.widths_measured = false;
                }
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
                // latched button over a listing that did not change.
                if tab.path.as_os_str().is_empty() {
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
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().toggle_collapsed(position);
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
                    match Self::selected_preview(tab, diffing) {
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
                self.config_dirty = true;
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
            Action::RememberLayout => self.config_dirty = true,

            Action::Cut(pane) => self.put_on_clipboard(pane, true),
            Action::Copy(pane) => self.put_on_clipboard(pane, false),
            Action::Paste(pane) => self.paste_into(pane, ctx),
            // The context menu's Couper, Copier and Coller, which name what they act on rather
            // than reading it off a pane. See `ours_rather_than_the_shell_s`.
            Action::CutItems(items) => self.put_these_on_clipboard(items, true),
            Action::CopyItems(items) => self.put_these_on_clipboard(items, false),
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
            Action::NewFolder(pane) => {
                let owner = self.owner;
                let Some(parent) = self.pane_mut(pane).map(|p| p.tab().path.clone()) else {
                    return;
                };
                if parent.as_os_str().is_empty() {
                    self.notice = Some("This PC is not a folder to create in".to_owned());
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
                moving,
            } => {
                self.focused = pane;
                let job = if moving {
                    crate::shell::ops::Job::Move { items, into }
                } else {
                    crate::shell::ops::Job::Copy { items, into }
                };
                self.ops.start(job, self.owner, ctx);
            }
            Action::DragOut { pane, items } => {
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
            // **A shortcut to a folder opens here, not in Explorer.** Handing it to the shell is
            // what `.lnk` files get by default, and for a folder that means a second file
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
            Action::Open(path) => match crate::shell::links::folder_target(&path) {
                Some(folder) => {
                    let pane = self.focused;
                    self.perform(ctx, Action::Navigate { pane, path: folder });
                }
                None => fs::shell::open(&path),
            },
            // The same, in a tab of its own — a middle click on a folder shortcut. A shortcut to
            // a *file* does nothing here rather than opening it somewhere it cannot be shown: a
            // new tab is a place, and a file is not one.
            Action::OpenNewTab(path) => {
                if let Some(folder) = crate::shell::links::folder_target(&path) {
                    let pane = self.focused;
                    self.perform(ctx, Action::NavigateNewTab { pane, path: folder });
                }
            }
            Action::Reveal(path) => fs::shell::reveal(&path),
            Action::OpenTerminal(path) => fs::shell::open_terminal(&path),
            Action::CopyPaths(paths) => {
                let text = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
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
            Action::SetTheme { dark } => {
                if dark == self.theme.dark {
                    return;
                }
                self.theme = if dark { Theme::dark() } else { Theme::light() };
                // The style has to be reinstalled, and only then — installing it every
                // frame would throw away egui's galley and shape caches.
                self.installed = false;
                self.config_dirty = true;
            }

            Action::Window(what) => {
                use egui::ViewportCommand as Cmd;
                match what {
                    WindowAction::Minimize => ctx.send_viewport_cmd(Cmd::Minimized(true)),
                    WindowAction::ToggleMaximize => {
                        ctx.send_viewport_cmd(Cmd::Maximized(!self.maximized));
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
