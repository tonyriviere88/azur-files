//! Moving files about: the clipboard, and drag and drop.
//!
//! Both go through the shell's own interfaces, so a copy taken here pastes into Explorer and a
//! drag out of this window is the drag Explorer would have started. See [`crate::shell`].

use super::*;

impl App {
    /// Put the selection on the clipboard, as a cut or as a copy.
    ///
    /// The context is only needed for a selection inside an archive, which has to be extracted on a
    /// thread before there is anything a clipboard can name — see [`App::put_these_on_clipboard`].
    pub(super) fn put_on_clipboard(&mut self, ctx: &egui::Context, pane: PaneId, cutting: bool) {
        let paths = self
            .pane_mut(pane)
            .map(|p| p.tab().selection_paths())
            .unwrap_or_default();
        self.put_these_on_clipboard(ctx, paths, cutting);
    }

    /// The same, on paths named outright.
    ///
    /// Split out from [`App::put_on_clipboard`] because the context menu's `cut` and `copy` are
    /// redirected into it — see [`App::ours_rather_than_the_shell_s`] — and the menu carries the
    /// items it was raised over rather than reading them back off the pane. One implementation, so
    /// that Ctrl+X and the menu's Couper cannot come to mean two different things.
    pub(super) fn put_these_on_clipboard(
        &mut self,
        ctx: &egui::Context,
        paths: Vec<PathBuf>,
        cutting: bool,
    ) {
        use crate::shell::clipboard::{put, Effect};

        if paths.is_empty() {
            self.notice = Some("Nothing selected".to_owned());
            return;
        }
        // **Nor out of the Recycle Bin**, where what a row names is the `$R…` file the bin holds.
        // A copy of it would be a copy under that name; a cut, pasted, would move it and leave its
        // entry in the bin describing nothing. Restoring is the way out — see
        // [`crate::fs::recycle`].
        if paths.iter().any(|path| fs::recycle::is_held(path)) {
            self.notice = Some(fs::recycle::RESTORE_FIRST.to_owned());
            return;
        }
        // **Inside an archive there is nothing yet to put on a clipboard.** A `CF_HDROP` is a list of
        // file names, and `D:\dl\pkg.zip\src\main.rs` is not one — pasting it anywhere would fail in
        // whichever program tried. So a copy becomes an extraction first, and the clipboard is
        // written when the real files exist: [`App::copy_out_of_archive`], whose answer comes back
        // through this same function with paths that are real.
        //
        // A **cut** is refused outright. It is not a copy with a flag: it is a promise that pasting
        // will *remove* the originals, and nothing in this program writes to an archive — see
        // [`crate::archive`], where that is scope and format both. Explorer refuses it on a zip for
        // the same reason.
        if paths.iter().any(|path| crate::archive::is_virtual_item(path)) {
            if cutting {
                self.notice =
                    Some("Files cannot be moved out of an archive. Copy them instead.".to_owned());
            } else {
                // Re-entered from `Extracted::Copied` with real paths, so this cannot recurse.
                self.copy_out_of_archive(ctx, paths);
            }
            return;
        }
        let effect = if cutting { Effect::Move } else { Effect::Copy };
        match put(&paths, effect) {
            // A cut marks its sources rather than moving them — nothing moves until
            // something pastes — so until then they have to *look* pending.
            Ok(()) => {
                self.cut = if cutting { paths } else { Vec::new() };
                self.notice = None;
            }
            Err(why) => self.notice = Some(why),
        }
    }

    /// Act on whatever is on the clipboard, into this pane's folder.
    pub(super) fn paste_into(&mut self, pane: PaneId, ctx: &egui::Context) {
        let Some(into) = self.pane_mut(pane).map(|p| p.tab().path.clone()) else {
            return;
        };
        self.paste_into_folder(into, ctx);
    }

    /// The same, into a folder named outright.
    ///
    /// Split out from [`App::paste_into`] because the context menu's `paste` is redirected into it
    /// — see [`App::ours_rather_than_the_shell_s`] — and that entry means "into the folder the
    /// menu was raised over", which is a *selected* folder and not the one the pane is showing.
    pub(super) fn paste_into_folder(&mut self, into: PathBuf, ctx: &egui::Context) {
        use crate::shell::clipboard::{self, Effect};
        use crate::shell::ops::Job;

        // **Nor is the Recycle Bin**, though a drop onto it is a delete: Explorer offers no Paste
        // there either, and a paste that quietly deleted what was on the clipboard would be the one
        // gesture in this program that loses a copy by making one.
        if fs::is_synthetic(&into) {
            self.notice = Some(format!(
                "{} is not a folder to paste into",
                fs::display_name(&into)
            ));
            return;
        }
        let Some(pasteable) = clipboard::get() else {
            self.notice = Some("There are no files on the clipboard".to_owned());
            return;
        };

        let moving = pasteable.effect == Effect::Move;
        let items = pasteable.items;
        // A cut is finished when the move is: `After::FinishCut` tells the clipboard's owner
        // it worked and then empties it, once the shell says it did. Emptying it here instead
        // -- which is what this used to do -- loses the cut for anyone who answers the
        // conflict dialog with Cancel, and leaves the sources still faded with nothing on the
        // clipboard to paste them from.
        let (job, after) = if moving {
            (
                Job::Move { items, into },
                crate::shell::ops::After::FinishCut(clipboard::sequence()),
            )
        } else {
            (Job::Copy { items, into }, crate::shell::ops::After::Nothing)
        };
        self.notice = None;
        self.ops.start_then(job, after, self.owner, ctx);
    }

    /// Keep a drag in flight painting, and clear up after it once it has landed.
    ///
    /// The drag runs on its own thread — see [`crate::shell::dnd::Drag`] — and while it does,
    /// nothing in egui's own event flow is happening: the pointer belongs to OLE, so no mouse
    /// event reaches winit and nothing would ask for a frame. Without a frame the drop
    /// highlight never appears and the selection the drag just made is never drawn. So the
    /// repaint is asked for unconditionally for the length of the drag, which is the one case
    /// in this program where painting is driven by a state rather than by an event.
    pub(super) fn pump_drag(&mut self, ctx: &egui::Context) {
        let Some(dragging) = &self.file_drag else {
            return;
        };
        let Some(effect) = dragging.drag.finished() else {
            ctx.request_repaint();
            return;
        };
        let pane = dragging.pane;
        self.file_drag = None;
        self.release_buttons(ctx);
        // A move took files out of this folder, and OLE does not say which — so the folder
        // is re-read. A copy changed nothing here.
        if effect == Some(crate::shell::clipboard::Effect::Move) {
            self.perform(ctx, Action::Refresh(pane));
        }
        ctx.request_repaint();
    }

    /// Tell egui the button came up, because nothing else is going to.
    ///
    /// **This is why a second drag did nothing.** `DoDragDrop` takes the mouse capture for the
    /// length of the drag and its own loop consumes the button-up that ends it, so the window
    /// never sees the release: egui goes on believing the button is held, and a press that
    /// arrives while a button is already down starts no new drag. One drag per window, and then
    /// nothing — until some unrelated click happened to put the state right, which is why it
    /// looked intermittent and why every test that ran a single drag in a fresh window passed.
    ///
    /// It cannot be fixed by watching for the release: it is never delivered here. So the
    /// release is stated rather than awaited, for whichever buttons egui still thinks are down,
    /// at the position egui already has — moving it would be inventing a gesture rather than
    /// finishing one.
    pub(super) fn release_buttons(&mut self, ctx: &egui::Context) {
        let (pos, modifiers, down) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                i.modifiers,
                [
                    egui::PointerButton::Primary,
                    egui::PointerButton::Secondary,
                ]
                .into_iter()
                .filter(|button| i.pointer.button_down(*button))
                .collect::<Vec<_>>(),
            )
        });
        let Some(pos) = pos else { return };
        for button in down {
            self.injected.push(egui::Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers,
            });
        }
    }

    /// Events egui has to be told about because the platform could not deliver them.
    ///
    /// Drained by the input hook, which is the only place raw input can be added to.
    pub fn take_injected(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.injected)
    }

    /// Tell the drop target which pane covers which folder.
    ///
    /// Published every frame because a pane can be split, resized or navigated between
    /// one drag and the next, and the OLE callbacks answer `DragOver` synchronously with
    /// no way to ask.
    ///
    /// Each region carries the destination's **name** as well as its rectangle, because the
    /// callbacks also have to say what a drop there will do — *Copy one.txt into src*, *Pin src to
    /// Work* — and neither the bookmark list nor a folder's display name is reachable from where
    /// they run. See [`crate::shell::dnd::Region`].
    pub(super) fn publish_drop_targets(&self, ctx: &egui::Context) {
        use crate::shell::dnd::{Onto, Region, Targets};

        let scale = ctx.pixels_per_point();
        let physical = |rect: Rect| {
            (
                (rect.left() * scale) as i32,
                (rect.top() * scale) as i32,
                (rect.right() * scale) as i32,
                (rect.bottom() * scale) as i32,
            )
        };

        /// Big enough to drop on and to draw a highlight around.
        fn usable(rect: Rect) -> bool {
            rect.width() >= 1.0 && rect.height() >= 1.0
        }

        /// What the sidebar's own heading calls the section, which is what a drop onto it means
        /// and so what the pointer has to be told it means.
        const SECTION: &str = "Bookmarks";

        /// And what a tab strip makes, which is the nearest thing it has to a name: there is no
        /// heading over a row of tabs, and the place a drop there lands does not exist yet.
        ///
        /// The **singular**, because a zone is named here, before there is a drag to count. The
        /// plural is put on where the count is — see `Target::describe`.
        const A_NEW_TAB: &str = "a new tab";

        // A pane's listing and a pane's strip, plus the Bookmarks section. The rows either of them
        // publishes are what the vector grows for.
        let mut zones: Vec<Region> = Vec::with_capacity(self.panes.len() * 2 + 1);
        // The bookmarks group first, so it is *behind* the panes: they cannot overlap, and
        // if a future layout let them, dropping onto a listing should mean the listing.
        if let Some(rect) = self.bookmarks_rect {
            zones.push(Region {
                rect: physical(rect),
                onto: Onto::Bookmarks,
                name: SECTION.to_owned(),
            });
            // Then each group — its own row and the rows under it, since a group is one thing —
            // so a group wins over the section it is in. The same arrangement, and the same
            // reason, as a folder row winning over its listing below: dropping a folder on a
            // group means *into that group*, and anywhere else in the section still means the
            // end of the list.
            for (row, group) in &self.bookmark_rows {
                let row = row.intersect(rect);
                if usable(row) {
                    zones.push(Region {
                        rect: physical(row),
                        onto: Onto::BookmarkGroup(*group),
                        // Its own name, so the tooltip names the box being joined. A row that
                        // is somehow no longer a group falls back to the section, which is
                        // where such a drop would land anyway.
                        name: self
                            .bookmarks
                            .group(*group)
                            .map_or_else(|| SECTION.to_owned(), |group| group.name.clone()),
                    });
                }
            }
        }
        for pane in &self.panes {
            let folder = pane.tab().path.clone();
            // This PC is a list of volumes rather than a directory, so nothing can be
            // dropped into it.
            //
            // The rows' rectangle rather than the pane's: the column header sorts and the
            // status line counts files, and dropping on either of them is not dropping into
            // the folder — so neither should light up saying that it is.
            if folder.as_os_str().is_empty() || !usable(pane.drop_area) {
                continue;
            }
            zones.push(Region {
                rect: physical(pane.drop_area),
                name: crate::fs::display_name(&folder),
                onto: Onto::Folder(folder),
            });
        }
        // The folder rows last, so they win: `Targets::at` takes the last match, and dropping
        // onto a folder has to mean *into that folder*. Dropping anywhere else in the listing
        // still means the folder being shown, which is what the zone above is for.
        for pane in &self.panes {
            for (row, folder) in &pane.drop_rows {
                let row = row.intersect(pane.drop_area);
                if usable(row) {
                    zones.push(Region {
                        rect: physical(row),
                        // The leaf, which is the name the row itself is showing — so the
                        // tooltip and the highlight are about the same thing.
                        name: crate::fs::display_name(folder),
                        onto: Onto::Folder(folder.clone()),
                    });
                }
            }
        }
        // And the tab strips last, which is the order they are *painted* in: a strip is drawn over
        // the panes so that a tab is never under a pane's card, and the zone that answers for it has
        // to agree. Nothing overlaps today — a band's height comes off the panes under it — but a
        // layout where it did should give the drop to the tab you can see.
        //
        // **The whole strip and not the tabs in it**, which is what makes the far end of the
        // sentence *a new tab* rather than a folder's name: see [`crate::shell::dnd::Onto::Tabs`]
        // for why dropping *on* a tab means the strip, and [`App::reveal_hovered_tab`] for what a
        // drag over a tab does instead.
        for (pane, strip) in &self.tab_strips {
            if !usable(*strip) || !self.panes.iter().any(|it| it.id == *pane) {
                continue;
            }
            zones.push(Region {
                rect: physical(*strip),
                name: A_NEW_TAB.to_owned(),
                onto: Onto::Tabs(*pane),
            });
        }
        // And where this window's own drag began, which is not a place anything can be dropped but
        // is the one thing the pointer cannot work out for itself — see
        // [`crate::shell::dnd::Targets::from`]. The whole pane and not the row: a folder is in one
        // pane's listing, so being back in the pane the drag came from is what makes a refusal
        // there unremarkable rather than wrong.
        let from = self.file_drag.as_ref().and_then(|dragging| {
            self.panes
                .iter()
                .find(|pane| pane.id == dragging.pane)
                .map(|pane| physical(pane.rect))
        });
        self.drops.publish(Targets { zones, from });
    }

    /// Which of these items a drop into `into` can actually act on.
    ///
    /// A folder cannot go inside itself, and the shell would refuse it noisily — see
    /// [`crate::shell::dnd::swallows`], which is the same test the pointer was answered with while
    /// the drag was still moving. **A drag with one such folder in it is refused whole and does not
    /// reach here at all**, so that filter is the guard for the one drag that can: a source that
    /// renders its paths only when the drop is real gave the pointer nothing to refuse, and its
    /// files arrive here for the first time — see [`crate::shell::dnd::refuses`].
    ///
    /// An item dropped back into the folder it is already in is meaningless *as a move* — it is a
    /// move to where it already is, and the pointer says nothing about one at all: see
    /// [`crate::shell::dnd::does_nothing`]. As a **copy** it is an ordinary gesture with an obvious
    /// answer, `one - Copy.txt`, which is what `keep` is: true for a copy, and true for a right
    /// drag, which has not said yet which of the two it is. Filtering those out before the question
    /// was asked meant a right drag inside a folder did nothing at all, which is the most obvious
    /// way anybody tries the gesture.
    pub(super) fn droppable(items: Vec<PathBuf>, into: &Path, keep: bool) -> Vec<PathBuf> {
        items
            .into_iter()
            .filter(|item| !crate::shell::dnd::swallows(item, into))
            .filter(|item| keep || !crate::shell::dnd::already_in(item, into))
            .collect()
    }

    /// Bring the tab a drag is hovering over to the front, so the pane under it shows that folder.
    ///
    /// **What a drop onto a tab strip does is open a tab** — see [`crate::shell::dnd::Onto::Tabs`] —
    /// so a tab is not a way in to the folder it names, and without this there would be no way to
    /// reach a folder that is open in a tab you are not looking at: the drag would have to be put
    /// down, the tab clicked, and the files picked up again. Hovering the tab reveals its pane, and
    /// the listing that appears is a drop away below the pointer.
    ///
    /// **The moment the pointer is over it, with no dwell.** Sweeping along a strip therefore shows
    /// each folder it crosses, which is worth more than it costs: switching a tab here is
    /// [`crate::pane::Pane::show_tab`] and nothing else — every tab in the window is scanned whether
    /// it is on show or not, so no listing is read for being passed over — and a delay would be a
    /// gesture that does nothing for a moment and then does something, which is the shape of a bug.
    ///
    /// Read from [`App::tab_slots`], which is last frame's: the strips are drawn after this runs.
    /// The same frame-late reading as every published drop zone, and for the same reason — a drag
    /// holds the pointer, so the layout under it is not moving.
    pub(super) fn reveal_hovered_tab(&mut self, ctx: &egui::Context) {
        let Some(at) = self.hover_at(ctx.pixels_per_point()) else {
            return;
        };
        let Some((pane, tab)) = self
            .tab_slots
            .iter()
            .find(|slot| slot.rect.contains(at))
            .map(|slot| (slot.pane, slot.tab))
        else {
            return;
        };
        // The pane is still there, the tab is still one of its tabs, and it is not already the one on
        // show — which is what makes this happen once per tab rather than once per frame of the drag.
        // Through the action, so that a tab coming forward is one thing wherever it is asked for,
        // journal entry included.
        if self
            .panes
            .iter()
            .any(|it| it.id == pane && tab < it.tabs.len() && it.active != tab)
        {
            self.perform(ctx, Action::ActivateTab { pane, tab });
        }
    }

    /// Act on files dropped onto a pane, and highlight the one being hovered.
    pub(super) fn collect_drops(&mut self, ctx: &egui::Context) {
        // The highlight, while a drag is over the window. A repaint is asked for
        // because the OLE callbacks run outside egui's own event flow and nothing else
        // would wake it.
        let hovering = self.drops.hovering();
        if hovering != self.drop_hover {
            self.drop_hover = hovering;
            ctx.request_repaint();
        }
        // And what that drop would do, in words. Refreshed beside the hover because the two are
        // halves of the same frame: the highlight says *where* and the sentence says *what*, and
        // one of them arriving a frame after the other would have them briefly disagree.
        let telling = self.drops.telling();
        if telling != self.drop_telling {
            self.drop_telling = telling;
            ctx.request_repaint();
        }
        // And whether that sentence is one to keep to itself, which travels beside it for the
        // reason it is written down beside it — see [`crate::shell::dnd::Shared::silent`].
        let silent = self.drops.silent();
        if silent != self.drop_silent {
            self.drop_silent = silent;
            ctx.request_repaint();
        }

        // And the tab under the pointer comes forward, which is the other half of what a drag over a
        // strip does — the three above are what it *says*, this is what it changes. Beside them
        // rather than after the drop, because it happens while the drag is still moving.
        self.reveal_hovered_tab(ctx);

        for dropped in self.drops.take_drops() {
            self.land(ctx, dropped);
        }
    }

    /// Act on one completed drop.
    ///
    /// # Entered twice for a drop out of an archive
    ///
    /// The drop arrives naming paths *inside* the archive — [`crate::shell::dnd::Shared::carrying`]
    /// explains why it must — so this starts the extraction on a worker and returns, and the answer
    /// comes back as [`Extracted::Landed`]: the same [`crate::shell::dnd::Dropped`] with real paths in
    /// it. The second pass then runs the whole of the rest of this, the right-button menu included,
    /// knowing nothing about archives.
    ///
    /// The same shape as [`App::open_from_archive`], and for the same reason: a second pass composes,
    /// where a special case inside each branch would have to be got right in each of them.
    pub(super) fn land(&mut self, ctx: &egui::Context, dropped: crate::shell::dnd::Dropped) {
        use crate::shell::clipboard::Effect;
        use crate::shell::ops::Job;

        // **On a worker, so the window goes on painting** — and painting is what puts the figures on
        // the status line, which is all somebody who has just dropped 300 files out of a solid `.7z`
        // has to look at. See [`crate::archive::extract::doing`].
        //
        // `any` and not `all`: a drag can mix an archive's entries with real files only through the
        // right-button menu, and [`crate::archive::extract::all`] hands back a real path unchanged,
        // so the mixed case needs nothing of its own.
        if dropped.items.iter().any(|item| crate::archive::is_virtual_item(item)) {
            let paths = dropped.items.clone();
            self.out_of_archive(ctx, paths, Then::Land(Box::new(dropped)));
            return;
        }
        // Where the drop actually landed, decided when the pointer was there rather than
        // worked out again now. It was worked out again, from the pane under the pointer,
        // and so a drop onto a *folder row* went into the folder being shown instead of into
        // the folder it was dropped on — the one thing dragging onto a folder means.
        let into = match dropped.onto {
            // Onto the sidebar: pin the folders and move nothing.
            //
            // **A drag with a file anywhere in it never gets here** — the pointer refuses it
            // while the drag is still moving, and the drop is answered with no effect at all;
            // see [`crate::shell::dnd::refuses`]. So `is_dir` is the guard for a drop whose
            // items have changed under it since, and not the rule: the rule is that a selection
            // is pinnable or it is refused, rather than half of it going in quietly.
            crate::shell::dnd::Onto::Bookmarks => {
                for item in dropped.items {
                    if item.is_dir() {
                        self.perform(ctx, Action::AddBookmark(item));
                    }
                }
                return;
            }
            // Onto a group's row: into that group. Which group was decided when the pointer
            // was over it, like everything else about where a drop landed — and the list
            // cannot have changed since, because a drag holds the pointer.
            crate::shell::dnd::Onto::BookmarkGroup(group) => {
                for item in dropped.items {
                    if item.is_dir() {
                        self.perform(ctx, Action::AddBookmarkIn { group, path: item });
                    }
                }
                return;
            }
            // Onto a pane's tab strip: a tab per folder, in the order they were dragged, and the
            // last of them showing — which is what opening several tabs any other way leaves.
            //
            // **Nothing is copied and nothing is moved**, so there is no [`App::droppable`] here and
            // no job: a tab is a folder being *shown*, and a folder can be shown in as many of them
            // as somebody drops. The `is_dir` is the same guard the two arms above carry, for the
            // same drag — the one that named its items only when it landed.
            crate::shell::dnd::Onto::Tabs(pane) => {
                if !self.panes.iter().any(|it| it.id == pane) {
                    return;
                }
                for item in dropped.items {
                    if item.is_dir() {
                        self.perform(ctx, Action::NavigateNewTab { pane, path: item });
                    }
                }
                return;
            }
            crate::shell::dnd::Onto::Folder(into) => into,
        };
        if into.as_os_str().is_empty() {
            return;
        }
        // **A drop onto the Recycle Bin is a delete**, whichever button carried it and whatever the
        // cursor said — which is what dropping onto Explorer's bin has always been. To the bin, and
        // so undone by Ctrl+Z like any other. Nothing already in the bin is re-recycled: that is a
        // drag from one bin pane to another, and it means nothing.
        if fs::recycle::is_bin(&into) {
            let items: Vec<PathBuf> = dropped
                .items
                .into_iter()
                .filter(|item| !fs::recycle::is_held(item))
                .collect();
            if !items.is_empty() {
                self.ops.start(
                    Job::Delete {
                        items,
                        to_bin: true,
                    },
                    self.owner,
                    ctx,
                );
            }
            return;
        }

        let scale = ctx.pixels_per_point();
        let at = egui::pos2(dropped.at.0 as f32 / scale, dropped.at.1 as f32 / scale);
        // Only to decide which pane the keyboard should follow the drop into; the
        // destination is `into`.
        let Some(pane) = self.panes.iter().find(|p| p.rect.contains(at)).map(|p| p.id) else {
            return;
        };
        // Dropping a folder into itself is meaningless whatever button carried it, and a drag
        // holding one was already refused while it was still moving -- so this is the drag that
        // named its files only now.
        // A *move* back into the folder the items are already in was refused while the drag was
        // still moving; a copy there is `one - Copy.txt`, a shortcut there is `one.txt.lnk`, and
        // a right drag is a question nobody has answered yet — so all three of those keep
        // everything they are carrying.
        let keep = dropped.asked || dropped.effect != Effect::Move;
        let items = Self::droppable(dropped.items, &into, keep);
        if items.is_empty() {
            return;
        }
        self.focused = pane;
        // A right-button drag asks rather than assumes, which is what Windows does and the
        // whole reason anybody drags with the right button.
        if dropped.asked {
            use crate::shell::menu::{Entry, Own};
            // Explorer's own four, in Explorer's own order.
            let own = [Own::CopyHere, Own::MoveHere, Own::LinkHere, Own::Cancel]
                .into_iter()
                .map(Entry::own)
                .collect();
            self.close_menu();
            // Built here rather than through the builder: these are this program's own
            // entries and there is nothing to ask the shell about, so the menu is ready now.
            // Token 0 matches no build, which is exactly right -- no answer is coming, and
            // the depth is the same nothing: every entry here is this program's own, so
            // there is no shell menu for one of them ever to be resolved against.
            self.menu = Some(crate::ui::menu::Open::new(
                pane,
                at,
                items,
                into,
                own,
                crate::shell::menu::Depth::Full,
                0,
            ));
            return;
        }
        let job = match dropped.effect {
            Effect::Move => Job::Move { items, into },
            Effect::Copy => Job::Copy { items, into },
            Effect::Link => Job::Link { items, into },
        };
        self.ops.start(job, self.owner, ctx);
    }
}
