//! The context menu: asking the shell for it, drawing it, and deciding which entries this
//! program answers itself rather than handing back.
//!
//! What is redirected and what is not is the interesting half — see
//! [`App::ours_rather_than_the_shell_s`].

use super::*;

impl App {
    /// Raise the context menu: ask the shell for it, and open it when the answer comes.
    ///
    /// The shell's entries take 130 ms for a folder and up to most of a second for a file --
    /// every time, not just the first -- so they are fetched by
    /// [`crate::shell::menu::Builder`] on a thread of its own. Nothing is drawn in the
    /// meantime: the menu appears once, whole, at its final size. The window keeps running
    /// frames throughout, which is the part that used to be missing.
    pub(super) fn shell_menu(
        &mut self,
        pane: PaneId,
        items: Vec<PathBuf>,
        at: (i32, i32),
        ctx: &egui::Context,
    ) {
        let Some(p) = self.panes.iter().find(|p| p.id == pane) else {
            return;
        };
        let folder = p.tab().path.clone();
        if folder.as_os_str().is_empty() {
            // This PC is a list of volumes, not a directory; the shell has no menu for it
            // that would mean anything here.
            return;
        }
        // **And no menu inside an archive**, for the same reason one level further on: the shell is
        // being asked about `D:\dl\pkg.zip\src\main.rs`, and there is no such file.
        // `SHParseDisplayName` refuses it, so there is no `IContextMenu` to query and the menu that
        // came up would be empty — or worse, the menu for the *archive*, whose Delete would delete
        // the whole thing.
        //
        // Nothing is lost that this program had to give. The note on [`crate::shell::menu::Own`]
        // sets out the position: a context menu here *is* Windows' menu, every command it carries
        // is on a keyboard shortcut too, and the shortcuts that make sense inside an archive still
        // work — `Ctrl+C` copies the files out (see [`crate::app::App::collect_operations`]) and
        // `Ctrl+Shift+C` copies their paths. The ones that do not are refused with a sentence
        // rather than left to fail.
        if crate::archive::is_virtual_location(&folder) {
            return;
        }

        // Two folders selected, which is what `Folder diff` is offered on. Asked of the listing and not
        // of the disk: the selection is rows of it, and each row already says whether it is a folder.
        // Not in the bin, whose folders are held as `$R…` and would be compared under those names.
        let diffable = items.len() == 2 && !fs::is_synthetic(&folder) && {
            let tab = p.tab();
            tab.dir.as_ref().is_some_and(|dir| {
                let folders: Vec<PathBuf> = tab
                    .selected
                    .iter()
                    .enumerate()
                    .filter(|&(entry, &on)| on && dir.entries.get(entry).is_some_and(|e| e.is_dir()))
                    .map(|(entry, _)| dir.target(entry))
                    .collect();
                folders.len() == 2 && items.iter().all(|item| folders.contains(item))
            })
        };

        // Whatever was open, or on its way, is not what was asked for.
        self.close_menu();
        let scale = ctx.pixels_per_point();
        let at = egui::pos2(at.0 as f32 / scale, at.1 as f32 / scale);
        let depth = Self::menu_depth(&folder, &items);
        self.asking = Some(Asking {
            token: self.menu_builder.build(&folder, &items, depth),
            pane,
            at,
            items,
            folder,
            depth,
            diffable,
            since: ctx.cumulative_pass_nr(),
            asked: std::time::Instant::now(),
        });
    }

    /// How much of a menu to ask the shell for.
    ///
    /// The whole cost of a context menu is inside one `QueryContextMenu`, which lets every
    /// installed extension contribute — so there is no such thing as skipping the slow entries
    /// once they exist. They cost what they cost before this program sees any of them. The only
    /// choice is how much to ask for, and [`crate::shell::menu::Depth`] has the measurements.
    ///
    /// So: an executable image on a network drive gets the reduced menu, and everything else
    /// gets everything. That is a narrow rule and it is the one the evidence supports. What is
    /// slow is not "the network" — a 41 MB `.lib` on the same share builds a full menu in 1.8 s,
    /// and the folder itself in 0.2 s — it is an executable on a share, where the time is linear
    /// in the file's size at about 570 kB/s because something reads all of it. A blanket rule for
    /// network paths would throw away 7-Zip and Send To on every file on the share to fix a
    /// problem that only executables have.
    ///
    /// It is not the last word either: [`App::pump_asking`] downgrades anything on a network
    /// drive that turns out to be slow regardless of what it is called, which is what covers the
    /// file types this list has not heard of.
    pub(super) fn menu_depth(folder: &Path, items: &[PathBuf]) -> crate::shell::menu::Depth {
        use crate::shell::menu::Depth;
        // What Windows loads as an executable image, which is the set something is entitled to
        // inspect byte by byte before deciding what to offer.
        const IMAGES: [&str; 8] = ["exe", "com", "scr", "dll", "ocx", "sys", "cpl", "drv"];

        let any_image = items.iter().any(|item| {
            item.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .is_some_and(|ext| IMAGES.contains(&ext.as_str()))
        });
        if any_image && crate::shell::over_network(folder) {
            Depth::Fast
        } else {
            Depth::Full
        }
    }

    /// While the shell is still being asked, say so — and take Escape or a click as "never mind".
    ///
    /// Usually there is nothing to say: a folder's menu comes back in a tenth of a second and
    /// this is over before a frame has been drawn. The case it is here for is the one measured
    /// in [`crate::shell::menu::Builder`] — most of half a minute for an executable on a share
    /// — where a window that shows nothing at all is indistinguishable from a window that has
    /// stopped working, and where a menu that finally appears long after the click has been
    /// forgotten is worse than no menu.
    ///
    /// Cancelling does not stop `QueryContextMenu`, because nothing stops `QueryContextMenu`.
    /// It stops *waiting* for it: the worker is abandoned and its answer will be thrown away.
    pub(super) fn pump_asking(&mut self, ctx: &egui::Context) {
        let Some(asking) = &self.asking else { return };
        // Not on the pass that asked. The right click that opens a menu is a press, and this
        // would take it as the cancellation of the menu it just asked for.
        let settled = ctx.cumulative_pass_nr() > asking.since;
        let quit = settled
            && ctx.input(|i| {
                i.key_pressed(egui::Key::Escape) || i.pointer.any_pressed() || i.pointer.any_click()
            });
        if quit {
            self.close_menu();
            return;
        }

        // A full menu on a network drive that has not arrived by now is not going to arrive
        // soon: the fast measurement on that share was 1.8 s for a 41 MB file, so anything past
        // this is an extension inspecting the file rather than the share being busy. Ask again
        // for less. Network only — a local menu is 0.13-0.69 s and should never be quietly
        // reduced because the machine happened to be busy for a moment.
        //
        // A selection only. The empty-selection menu is the folder's *background* menu, and
        // `CMF_DEFAULTONLY` there asks for the default verb of a thing that has none: what comes
        // back is a couple of entries with New — the only reason to open that menu — not among
        // them. There is nothing to reduce anyway, since the slow case this covers is an
        // extension reading a selected file.
        const PATIENCE: std::time::Duration = std::time::Duration::from_millis(2_500);
        if asking.depth == crate::shell::menu::Depth::Full
            && !asking.items.is_empty()
            && asking.asked.elapsed() > PATIENCE
            && crate::shell::over_network(&asking.folder)
        {
            let (pane, at, items, folder, diffable) = (
                asking.pane,
                asking.at,
                asking.items.clone(),
                asking.folder.clone(),
                asking.diffable,
            );
            self.close_menu();
            self.asking = Some(Asking {
                token: self
                    .menu_builder
                    .build(&folder, &items, crate::shell::menu::Depth::Fast),
                pane,
                at,
                items,
                folder,
                depth: crate::shell::menu::Depth::Fast,
                diffable,
                since: ctx.cumulative_pass_nr(),
                asked: std::time::Instant::now(),
            });
        }

        ctx.set_cursor_icon(egui::CursorIcon::Progress);
        // The answer arrives on a channel and the worker asks for a repaint when it has one, so
        // this is not how the menu gets drawn. It is here so that the cursor is re-asserted and
        // the window demonstrably keeps drawing while the shell takes its time — at ten frames
        // a second rather than as fast as possible, since there is nothing to animate.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// Take delivery of whatever the menu builder has finished, and pass on what the menu
    /// on screen has since asked for.
    pub(super) fn pump_menu(&mut self) {
        use crate::shell::menu::Said;

        while let Some(said) = self.menu_builder.poll() {
            // A menu the user has dismissed, or replaced with a second right click, still
            // has an answer coming. The token is how it is told apart from the live one, and
            // an answer that does not match is dropped.
            match said {
                Said::Built {
                    token,
                    entries,
                    depth,
                    handlers,
                } => {
                    let Some(asking) = self.asking.take_if(|a| a.token == token) else {
                        continue;
                    };
                    // The shell's entries go in raw. Banding them into groups, and putting this
                    // program's own Paste and `Copy path(s)` in, is `Open::arrange`'s job — because
                    // it has to happen again, unchanged, every time a right click moves an entry.
                    //
                    // The clipboard is read here and not there: it is the desktop's one clipboard
                    // and `has_files` takes it, which is once per menu and not once per
                    // rearrangement. Only for a background menu, which is the only one that gets a
                    // Paste of ours.
                    // Not in the Recycle Bin, which is not a folder to paste into — see
                    // [`App::paste_into_folder`], which would only say so.
                    let can_paste = asking.items.is_empty()
                        && !fs::is_synthetic(&asking.folder)
                        && crate::shell::clipboard::has_files();
                    self.menu = Some(
                        crate::ui::menu::Open::new(
                            asking.pane,
                            asking.at,
                            asking.items,
                            asking.folder,
                            entries,
                            depth,
                            token,
                        )
                        .diffable(asking.diffable)
                        .banded(handlers, can_paste, &self.menu_moves),
                    );
                }
                Said::Filled { token, id, children } => {
                    if let Some(menu) = self.menu.as_mut().filter(|m| m.token == token) {
                        menu.filled(id, children);
                    }
                }
            }
        }

        if let Some(menu) = self.menu.as_mut() {
            let token = menu.token;
            for id in std::mem::take(&mut menu.fills) {
                self.menu_builder.fill(token, id);
            }
        }
    }

    /// Whether a menu has been asked for and has not appeared yet.
    ///
    /// For `--shot --menu`, which would otherwise photograph the window without one.
    pub fn menu_pending(&self) -> bool {
        self.asking.is_some()
    }

    /// Let go of the menu that is closing, and of the one on its way if there is one.
    pub fn close_menu(&mut self) {
        if let Some(menu) = self.menu.take() {
            self.menu_builder.close(menu.token);
        }
        if self.asking.take().is_some() {
            // Nothing to close — the shell has not finished making it. The worker is let go of
            // instead, so that the next menu is built on a thread that is not inside a call
            // that may have twenty seconds left to run.
            self.menu_builder.abandon();
        }
    }

    /// Put every popup away when the window stops being the one you are using.
    ///
    /// A menu belongs to a moment. Alt-tab to something else and come back ten minutes later and
    /// a context menu still standing over the listing is not where you left off — it is a menu
    /// about a file you have stopped thinking about, over a window you have to click twice to get
    /// back into. Windows itself dismisses a menu when its owner loses activation, and this window
    /// has three kinds of its own to dismiss: the context menu, the popups egui tracks in its
    /// memory — the application menu under the mark at the top left — and the path bar's
    /// dropdowns with the tracking mode they turn on.
    ///
    /// Every unfocused frame rather than only the one where focus was lost: the transition needs a
    /// frame to be noticed in, an unfocused window is not always given one at the moment it goes,
    /// and asking "is anything open while we are not in front" has the same answer either way. It
    /// is also cheap — the three are already empty every other time this runs.
    ///
    /// `focused` is the *window's*, not egui's: a focused text field is a different thing entirely
    /// and closing a menu because the filter box has the caret would be a bug. `RawInput::focused`
    /// defaults to `true`, so a frame from an integration that does not track focus never trips
    /// this.
    pub(super) fn close_on_blur(&mut self, ctx: &egui::Context) {
        // The design system puts egui's own popups away and reports whether it fired, so this
        // window's two hand-tracked ones — the application menu and the breadcrumb's dropdown,
        // neither of which is an `egui::Popup` — go with them in the same breath.
        if azur_egui_theme::desktop::close_popups_on_blur(ctx) {
            self.close_menu();
            self.crumbs.close();
        }
    }

    /// Raise the focused pane's menu, over its listing.
    ///
    /// For `--menu`, which is how a capture run gets a menu on screen: it has no pointer
    /// to right-click with, and the menu is the one part of the window a screenshot cannot
    /// otherwise reach. Goes through the same action as a real right click, so what it
    /// captures is the real menu and not a mock-up of one.
    ///
    /// **The selection's menu if there is a selection**, and the folder's background menu otherwise.
    /// Which is what a right click does, and it is the difference between the two that makes the flag
    /// worth having: a folder has *two* menus from two different shell objects — see
    /// `win::context_of` — and only the selection's has shell32's Cut/Copy/Rename/Share/Delete block
    /// in it, which is the row `crate::shell::menu::regroup` turns into tiles. So
    /// `--reveal=<name> --menu` photographs the banded menu with its tile row, and `--menu` alone
    /// photographs the background one. Before this it was always the second, and the tile row could
    /// not be captured at all.
    pub fn open_folder_menu(&mut self, ctx: &egui::Context) {
        let Some(pane) = self.panes.iter().find(|p| p.id == self.focused) else {
            return;
        };
        let rect = pane.rect;
        let scale = ctx.pixels_per_point();
        let at = rect.min + egui::vec2(rect.width() * 0.22, rect.height() * 0.30);
        let items = pane.tab().selection_paths();
        self.actions.push(Action::ShellMenu {
            pane: pane.id,
            items,
            at: ((at.x * scale) as i32, (at.y * scale) as i32),
        });
    }

    /// Draw the context menu, if one is open, and act on what it says.
    pub(super) fn draw_menu(&mut self, ui: &mut Ui, theme: &Theme) {
        use crate::shell::menu::Command;
        use crate::ui::menu::Outcome;

        // Unconditionally, and before the early return: this is what *opens* the menu, so a
        // version that skipped it while there was nothing on screen would wait for ever for
        // a menu it never took delivery of. Cheap when there is nothing to collect — one
        // `try_recv` that fails.
        self.pump_menu();

        let outcome = match &mut self.menu {
            Some(menu) => crate::ui::menu::show(ui, theme, menu),
            None => return,
        };
        // A hover during this pass may have asked for a submenu; send it now rather than
        // waiting a frame for the next pump.
        self.pump_menu();

        match outcome {
            Outcome::Open => {}
            Outcome::Closed => self.close_menu(),
            // A right click moved an entry between a collapsed group and the main menu. The menu
            // stays open and is rearranged under the pointer, which is the whole point — the
            // alternative is a menu that closes so you can reopen it to see what you did.
            //
            // Nothing is asked of the shell: `Open::arrange` re-runs `regroup` over the entries the
            // menu already holds. See `crate::ui::menu::Open::raw`.
            Outcome::Move { keys, into_group } => {
                for key in keys {
                    self.menu_moves.record(key, into_group);
                }
                // Cloned because `arrange` borrows the menu mutably and the moves are a field
                // beside it. A `Moves` is two sets of short strings and this happens on a click.
                let moves = self.menu_moves.clone();
                if let Some(menu) = self.menu.as_mut() {
                    menu.arrange(&moves);
                }
                // Marked rather than written. `App::apply` is the one choke point that rate-limits
                // settings writes, and the note there is explicit about why a call site must not
                // save for itself — see `crate::app::perform`.
                self.config_dirty = true;
            }
            Outcome::Chose(command) => {
                let Some(menu) = self.menu.take() else { return };
                self.menu_builder.close(menu.token);
                match command {
                    Command::Own(which) => {
                        if let Some(action) = self.own_menu_action(&menu, which) {
                            self.actions.push(action);
                        }
                    }
                    // Off to the modal thread: invoking can open anything from a
                    // Properties sheet to an installer, and neither belongs in a frame.
                    // Nothing is remembered about which pane asked: whatever the command does
                    // to the folder, `crate::watch` is what notices. See `collect_modal`.
                    //
                    // Unless it is one of the handful this program answers itself — see
                    // `ours_rather_than_the_shell_s`.
                    Command::Shell { .. } => {
                        let ours = Self::ours_rather_than_the_shell_s(&menu, &command);
                        if !ours.is_empty() {
                            self.actions.extend(ours);
                            return;
                        }
                        // With one exception, and it is written down *here* because here is the
                        // last moment it can be.
                        self.watch_for_a_new_item(menu.pane, &menu.folder, &command);
                        self.modal.send(crate::shell::Request::Invoke {
                            parent: menu.folder.clone(),
                            items: menu.items.clone(),
                            command,
                            depth: menu.depth,
                            owner: self.owner,
                        });
                    }
                }
            }
        }
    }

    /// The entries in Windows' own menu that this program answers itself.
    ///
    /// The context menu is the shell's — every entry in it is there because the shell or an
    /// installed extension put it there, under whatever name this Windows is in. For nearly all of
    /// them, handing the verb straight back is exactly right: that is the whole point of showing
    /// the real menu rather than an imitation of it. A handful are different, because what they are
    /// *for* is the file manager in front of the user, and that one is this one:
    ///
    /// | verb | what Windows would do | what happens instead |
    /// | --- | --- | --- |
    /// | `open` on a folder | opens it in a new **Explorer window** | navigates this pane |
    /// | `cut`, `copy` | fills the clipboard, and nothing here knows | [`App::put_these_on_clipboard`] |
    /// | `rename` | **nothing at all** — it needs a view to put a caret in, and there is none | this program's rename field, `F2`'s |
    /// | `paste` on a folder | the shell's own copy, with no notice and no undo of ours | [`App::paste_into_folder`] |
    /// | `pintohome` | Explorer's Quick access | this program's bookmarks — see [`App::pin_is_a_bookmark`] |
    ///
    /// Recognised by verb in every case, never by label. `GetCommandString` gives the canonical
    /// name, which is the same on every Windows; the labels on this machine are `Ouvrir`, `Couper`,
    /// `Copier` and `Coller`, and matching those would be a program that works in French.
    /// `the_verbs_this_program_takes_over_are_still_the_shell_s` is what holds the four names to
    /// what the shell actually offers, because a hook keyed on a verb that has been renamed does
    /// not fail — it silently stops intercepting.
    ///
    /// Empty means "not ours, give it to the shell", which is the answer for all but four verbs
    /// and also for the cases below where a verb *is* one of the four and there is nothing here to
    /// do with it.
    ///
    /// # What is deliberately not redirected
    ///
    /// **`open` on files.** Only folders are taken over. The shell's `open` on a file is the
    /// registered default verb with everything that comes with it, and a file is not a place this
    /// program can show, so there is nothing to gain and a working Open to lose.
    ///
    /// **Paste on empty space.** There is no `paste` verb on a folder's *background* menu to
    /// redirect: that menu is the view object's, and Explorer synthesises its own Paste around it
    /// rather than reading one out of the shell — see `shell::menu::win::context_of`. So pasting
    /// into the folder you are looking at is Ctrl+V, as it already was. The `paste` this hooks is
    /// the one on a **selected folder**, which means "into that folder", and the shell offers it
    /// only while there is something on the clipboard.
    pub(super) fn ours_rather_than_the_shell_s(
        menu: &crate::ui::menu::Open,
        command: &crate::shell::menu::Command,
    ) -> Vec<Action> {
        let crate::shell::menu::Command::Shell { verb: Some(verb), .. } = command else {
            return Vec::new();
        };
        match verb.as_str() {
            "pintohome" | "unpinfromhome" => Self::pin_is_a_bookmark(menu, command),
            "open" => Self::open_in_this_explorer(menu),
            // The selection the menu was raised over, not the pane's — see
            // [`App::put_these_on_clipboard`]. Empty is the background menu, which has neither
            // entry on it; the guard is here so that a shell that grew one would fall through to
            // it rather than quietly clearing the clipboard.
            // **Except in the Recycle Bin**, where the bin's own Cut is the right one: it puts the
            // bin's items on the clipboard in the bin's own terms, and a paste of that in Explorer
            // takes them out of the bin properly. This program's clipboard would carry the `$R…`
            // files, which is exactly what [`App::put_these_on_clipboard`] refuses.
            "cut" | "copy" if menu.items.iter().any(|item| fs::recycle::is_held(item)) => {
                Vec::new()
            }
            "cut" if !menu.items.is_empty() => vec![Action::CutItems(menu.items.clone())],
            "copy" if !menu.items.is_empty() => vec![Action::CopyItems(menu.items.clone())],
            // The one verb here that the shell would not merely do *differently* — it would do
            // nothing. `rename` opens an inline editor in the view hosting the menu, and a menu built
            // from a bare shell folder has no view, so `InvokeCommand` returns and no caret appears
            // anywhere. See `win::flags`, which is also what asks the shell for the entry at all.
            //
            // `BeginRename` renames the row under the pane's cursor, and the cursor is on the row
            // this menu was raised over: a right click selects the row before the menu is asked for
            // — see `crate::ui::filelist`, where the selection is settled first. One item only,
            // because a rename field is one name: the shell offers the entry on a multiple selection
            // and Explorer answers it by renaming them all in sequence, which is a different feature
            // and not one to imply by accident.
            "rename" if menu.items.len() == 1 => vec![Action::BeginRename(menu.pane)],
            // Into the selected folder. The shell offers this on any selection with a folder
            // somewhere in it, including several at once — where Explorer's own answer is not
            // something to reproduce by accident. The first folder is the one, and the rest of
            // the selection is left alone: pasting into one folder is undone by hand, and
            // pasting into four is not.
            "paste" => menu
                .items
                .iter()
                .find(|item| item.is_dir())
                .map(|into| vec![Action::PasteIntoFolder(into.clone())])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// `Open` on a selection with a folder in it, as this program's own navigation.
    ///
    /// The gesture is one `InvokeCommand` for the whole selection — there is no asking the shell
    /// to open half of it — so this either takes the lot or none of it. It takes the lot as soon as
    /// there is one folder present, because the alternative is a folder opening in a second file
    /// manager over the top of this one, which is the thing being fixed. Any files alongside go
    /// through [`Action::Open`], which is what `Enter` and a double click on a row already do.
    ///
    /// A selection of nothing but files is *not* ours — see the note on
    /// [`App::ours_rather_than_the_shell_s`] — and comes back empty.
    ///
    /// One folder navigates the pane the menu was raised in, which is what double-clicking it
    /// does and what Explorer's own Open does to the window it was invoked from. Several open in
    /// tabs of their own instead, leaving the pane where it is: Explorer answers the same gesture
    /// with one new window each, and a tab is what this program has that a window was for.
    pub(super) fn open_in_this_explorer(menu: &crate::ui::menu::Open) -> Vec<Action> {
        let places: Vec<(PathBuf, Option<PathBuf>)> = menu
            .items
            .iter()
            .map(|item| (item.clone(), Self::place_of(item)))
            .collect();
        let folders: Vec<PathBuf> = places.iter().filter_map(|(_, place)| place.clone()).collect();
        if folders.is_empty() {
            return Vec::new();
        }

        let mut actions: Vec<Action> = if let [only] = &folders[..] {
            vec![Action::Navigate {
                pane: menu.pane,
                path: only.clone(),
            }]
        } else {
            folders
                .into_iter()
                .map(|path| Action::NavigateNewTab {
                    pane: menu.pane,
                    path,
                })
                .collect()
        };
        actions.extend(
            places
                .into_iter()
                .filter(|(_, place)| place.is_none())
                .map(|(item, _)| Action::Open(item)),
        );
        actions
    }

    /// Where this program could go for an item, if the item is a place at all.
    ///
    /// A directory, or a shortcut to one — the same two things `Enter` on a row treats as somewhere
    /// to go, and for the same reason: a `.lnk` to a folder handed to the shell opens Explorer. A
    /// junction or a directory symlink is a directory here, which is what the listing calls it too.
    pub(super) fn place_of(item: &Path) -> Option<PathBuf> {
        if item.is_dir() {
            return Some(item.to_path_buf());
        }
        crate::shell::links::folder_target(item)
    }

    /// `Pin to Quick access` means *this* program's bookmarks, not Explorer's Quick access.
    ///
    /// The entry is Windows' own — it is in the menu because the shell put it there, under
    /// whatever name this Windows is in: `Épingler à l'accès rapide` here, `Pin to Quick access`
    /// on an English one. What it is *for* is the sidebar of a file manager, and the sidebar in
    /// front of the user is this one. Handing it to the shell put the folder in Explorer's
    /// Quick access, where nothing in this program can see it, and left this program's own
    /// bookmarks — the same gesture, on Ctrl+D — untouched.
    ///
    /// Recognised by verb, because the label is a translation: `pintohome` is what the shell
    /// calls it on every Windows, and `unpinfromhome` is the other half. Both are folder-only,
    /// which is also what a bookmark is. `pintohomefile` — Windows 11's `Add to Favorites`, for
    /// files — is deliberately *not* here: it is a different list of a different kind of thing,
    /// and a bookmark bar of files is not what this sidebar is.
    ///
    /// Empty means "not ours, give it to the shell".
    pub(super) fn pin_is_a_bookmark(menu: &crate::ui::menu::Open, command: &crate::shell::menu::Command) -> Vec<Action> {
        let crate::shell::menu::Command::Shell { verb: Some(verb), .. } = command else {
            return Vec::new();
        };
        let add = match verb.as_str() {
            "pintohome" => true,
            "unpinfromhome" => false,
            _ => return Vec::new(),
        };
        // A selection is what is selected; an empty one is the folder the menu was raised in,
        // which is the background menu's answer to "pin what?".
        let mut targets: Vec<PathBuf> = if menu.items.is_empty() {
            vec![menu.folder.clone()]
        } else {
            menu.items.clone()
        };
        // Only folders. The shell offers this on nothing else, but the selection is this
        // program's and a rule that depends on the shell having filtered it is not a rule.
        targets.retain(|path| path.is_dir());
        targets
            .into_iter()
            .map(|path| {
                if add {
                    Action::AddBookmark(path)
                } else {
                    Action::RemoveBookmark(path)
                }
            })
            .collect()
    }

    /// Note the listing before handing over a verb that is about to add to it.
    ///
    /// A `New >` entry creates a file and will not say which, so the row that is in the next
    /// listing and not in this one is the file it made — and that next listing is the watcher's,
    /// arriving on its own a moment later. See [`crate::pane::Tab::name_the_new`] for why the name
    /// cannot simply be asked for, and
    /// [`crate::shell::menu::Command::creates_an_item`] for how the entry is told apart from the
    /// rest of the menu.
    ///
    /// Its own method rather than four lines inside [`App::draw_menu`] so that the test which
    /// drives the whole chain — real verb, real watcher, real re-read — makes the same decision the
    /// menu makes instead of a copy of it that can drift.
    pub(super) fn watch_for_a_new_item(
        &mut self,
        pane: PaneId,
        folder: &Path,
        command: &crate::shell::menu::Command,
    ) {
        if !command.creates_an_item() {
            return;
        }
        let Some(p) = self.pane_mut(pane) else { return };
        let tab = p.tab_mut();
        // Only while this pane is still showing the folder the menu was raised over. A snapshot of
        // somewhere else would call every row in this folder new.
        if tab.path == *folder {
            tab.name_the_new = Some(tab.names());
        }
    }

    /// What one of this program's own menu entries means.
    ///
    /// Three things ask anything of their own now: a right-button drop, Paste on empty space, and
    /// `Copy path(s)`. Everything else that used to be here -- Open, Open in new tab, Open in a pane
    /// to the right or below, Add to bookmarks, Refresh, Select all, Show hidden files, New folder,
    /// Open terminal here -- has been taken out of the context menu, which otherwise shows Windows'
    /// menu and nothing else.
    pub(super) fn own_menu_action(
        &self,
        menu: &crate::ui::menu::Open,
        which: crate::shell::menu::Own,
    ) -> Option<Action> {
        use crate::shell::menu::Own;

        Some(match which {
            // A right-button drag, answered. The menu already carries what was dropped and
            // where, so there is nothing to look up.
            Own::CopyHere | Own::MoveHere | Own::LinkHere => Action::DropHere {
                pane: menu.pane,
                items: menu.items.clone(),
                into: menu.folder.clone(),
                effect: match which {
                    Own::MoveHere => crate::shell::clipboard::Effect::Move,
                    Own::LinkHere => crate::shell::clipboard::Effect::Link,
                    _ => crate::shell::clipboard::Effect::Copy,
                },
            },
            Own::Cancel => return None,
            // The two in the order the listing shows them, which is the order they were selected in
            // on screen: the upper one on the left.
            Own::FolderDiff => match &menu.items[..] {
                [left, right] => Action::DiffFolders {
                    pane: menu.pane,
                    left: left.clone(),
                    right: right.clone(),
                },
                _ => return None,
            },
            // Into the folder the menu was raised in, which for a background menu is the folder
            // being shown. Named outright rather than as [`Action::Paste`], which would read the
            // pane again: the same reasoning as the redirected `paste` verb, and the same action.
            Own::Paste => Action::PasteIntoFolder(menu.folder.clone()),
            // The selection the menu was raised over, and an empty one is the folder being shown —
            // [`App::pin_is_a_bookmark`]'s answer to the same question, and `Ctrl+Shift+C`'s: a
            // background menu asking "the path of what?" is asking about the folder you are in.
            // Which slash they are written with is [`Action::CopyPaths`]', so the entry and the
            // shortcut cannot disagree about it.
            Own::CopyPaths => Action::CopyPaths(if menu.items.is_empty() {
                vec![menu.folder.clone()]
            } else {
                menu.items.clone()
            }),
        })
    }
}
