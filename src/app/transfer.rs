//! Moving files about: the clipboard, and drag and drop.
//!
//! Both go through the shell's own interfaces, so a copy taken here pastes into Explorer and a
//! drag out of this window is the drag Explorer would have started. See [`crate::shell`].

use super::*;

impl App {
    /// Put the selection on the clipboard, as a cut or as a copy.
    pub(super) fn put_on_clipboard(&mut self, pane: PaneId, cutting: bool) {
        let paths = self
            .pane_mut(pane)
            .map(|p| p.tab().selection_paths())
            .unwrap_or_default();
        self.put_these_on_clipboard(paths, cutting);
    }

    /// The same, on paths named outright.
    ///
    /// Split out from [`App::put_on_clipboard`] because the context menu's `cut` and `copy` are
    /// redirected into it — see [`App::ours_rather_than_the_shell_s`] — and the menu carries the
    /// items it was raised over rather than reading them back off the pane. One implementation, so
    /// that Ctrl+X and the menu's Couper cannot come to mean two different things.
    pub(super) fn put_these_on_clipboard(&mut self, paths: Vec<PathBuf>, cutting: bool) {
        use crate::shell::clipboard::{put, Effect};

        if paths.is_empty() {
            self.notice = Some("Nothing selected".to_owned());
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

        if into.as_os_str().is_empty() {
            self.notice = Some("This PC is not a folder to paste into".to_owned());
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
        let Some((pane, drag)) = &self.file_drag else {
            return;
        };
        let Some(effect) = drag.finished() else {
            ctx.request_repaint();
            return;
        };
        let pane = *pane;
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
    pub(super) fn publish_drop_targets(&self, ctx: &egui::Context) {
        use crate::shell::dnd::{Onto, Targets};

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

        let mut zones = Vec::with_capacity(self.panes.len() + 1);
        // The bookmarks group first, so it is *behind* the panes: they cannot overlap, and
        // if a future layout let them, dropping onto a listing should mean the listing.
        if let Some(rect) = self.bookmarks_rect {
            zones.push((physical(rect), Onto::Bookmarks));
            // Then each group — its own row and the rows under it, since a group is one thing —
            // so a group wins over the section it is in. The same arrangement, and the same
            // reason, as a folder row winning over its listing below: dropping a folder on a
            // group means *into that group*, and anywhere else in the section still means the
            // end of the list.
            for (row, group) in &self.bookmark_rows {
                let row = row.intersect(rect);
                if usable(row) {
                    zones.push((physical(row), Onto::BookmarkGroup(*group)));
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
            zones.push((physical(pane.drop_area), Onto::Folder(folder)));
        }
        // The folder rows last, so they win: `Targets::at` takes the last match, and dropping
        // onto a folder has to mean *into that folder*. Dropping anywhere else in the listing
        // still means the folder being shown, which is what the zone above is for.
        for pane in &self.panes {
            for (row, folder) in &pane.drop_rows {
                let row = row.intersect(pane.drop_area);
                if usable(row) {
                    zones.push((physical(row), Onto::Folder(folder.clone())));
                }
            }
        }
        self.drops.publish(Targets { zones });
    }

    /// Which of these items a drop into `into` can actually act on.
    ///
    /// Dropping a folder into itself is meaningless whatever button carried it, and the shell would
    /// refuse it noisily. A file dropped back into the folder it is already in is meaningless too
    /// *for a left drag* — it is a move to where it already is — but not for a right one:
    /// right-dragging a file onto its own folder is how Explorer is asked for a copy of it, and the
    /// answer is `one - Copy.txt`. Filtering those out before the question was asked meant a right
    /// drag inside a folder did nothing at all, which is the most obvious way to try the gesture.
    pub(super) fn droppable(items: Vec<PathBuf>, into: &Path, asked: bool) -> Vec<PathBuf> {
        items
            .into_iter()
            .filter(|item| item != into)
            .filter(|item| asked || item.parent() != Some(into))
            .collect()
    }

    /// Act on files dropped onto a pane, and highlight the one being hovered.
    pub(super) fn collect_drops(&mut self, ctx: &egui::Context) {
        use crate::shell::clipboard::Effect;
        use crate::shell::ops::Job;

        // The highlight, while a drag is over the window. A repaint is asked for
        // because the OLE callbacks run outside egui's own event flow and nothing else
        // would wake it.
        let hovering = self.drops.hovering();
        if hovering != self.drop_hover {
            self.drop_hover = hovering;
            ctx.request_repaint();
        }

        for dropped in self.drops.take_drops() {
            // Where the drop actually landed, decided when the pointer was there rather than
            // worked out again now. It was worked out again, from the pane under the pointer,
            // and so a drop onto a *folder row* went into the folder being shown instead of into
            // the folder it was dropped on — the one thing dragging onto a folder means.
            let into = match dropped.onto {
                // Onto the sidebar: pin the folders and move nothing. Files are ignored rather
                // than refused, so dragging a mixed selection over pins what can be pinned.
                crate::shell::dnd::Onto::Bookmarks => {
                    for item in dropped.items {
                        if item.is_dir() {
                            self.perform(ctx, Action::AddBookmark(item));
                        }
                    }
                    continue;
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
                    continue;
                }
                crate::shell::dnd::Onto::Folder(into) => into,
            };
            if into.as_os_str().is_empty() {
                continue;
            }

            let scale = ctx.pixels_per_point();
            let at = egui::pos2(
                dropped.at.0 as f32 / scale,
                dropped.at.1 as f32 / scale,
            );
            // Only to decide which pane the keyboard should follow the drop into; the
            // destination is `into`.
            let Some(pane) = self
                .panes
                .iter()
                .find(|p| p.rect.contains(at))
                .map(|p| p.id)
            else {
                continue;
            };
            // Dropping a folder into itself is meaningless whatever button carried it, and the
            // shell would refuse it noisily. A file dropped back into the folder it is already in
            // is meaningless too *for a left drag* -- it is a move to where it already is -- but
            // not for a right one: right-dragging a file onto its own folder is how Explorer is
            // asked for a copy of it, and the answer is `one - Copy.txt`. Filtering those out
            // before the question was asked meant a right drag inside a folder did nothing at
            // all, which is the most obvious way to try the gesture.
            let items = Self::droppable(dropped.items, &into, dropped.asked);
            if items.is_empty() {
                continue;
            }
            self.focused = pane;
            // A right-button drag asks rather than assumes, which is what Windows does and the
            // whole reason anybody drags with the right button.
            if dropped.asked {
                use crate::shell::menu::{Entry, Own};
                let own = [Own::CopyHere, Own::MoveHere, Own::Cancel]
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
                continue;
            }
            let job = match dropped.effect {
                Effect::Move => Job::Move { items, into },
                Effect::Copy => Job::Copy { items, into },
            };
            self.ops.start(job, self.owner, ctx);
        }
    }
}
