//! Every shortcut in the window, in one place.
//!
//! Read after the frame is drawn, so a field that took a keystroke has already said so and the
//! same key does not do two things at once.

use super::*;

impl App {
    pub(super) fn keyboard(&mut self, ctx: &egui::Context) {
        use egui::Key as K;

        // A focused text field owns the keyboard. Escape is the one key that still
        // has to get through, or a filter box becomes a trap.
        let renaming = self
            .panes
            .iter()
            .any(|p| p.tabs.iter().any(|t| t.renaming.is_some()));
        // A menu on screen owns the keyboard: its own arrows and Enter are handled where
        // it is drawn, and the listing must not move underneath it at the same time.
        let typing =
            renaming || self.menu.is_some() || ctx.memory(|m| m.focused()).is_some();
        if typing {
            // **`Ctrl+E` and `Ctrl+P` still get through.** Both are questions about the folder you
            // are looking at rather than about the field the caret happens to be in, and both
            // compose with a filter: you type two letters to find something, then want the rest of
            // the tree, or want to see what is inside what you found. The filter survives either,
            // so having to click out of the box first is a step with no reason behind it — and
            // `Ctrl+F` already works the other way round.
            //
            // Not while renaming, and not under a menu: a rename is an edit of one name that
            // re-reading the folder would throw away, and a menu owns the keyboard outright.
            // `consume_key` rather than `key_pressed`, so the field it was typed into does not
            // also see it.
            let allowed = !renaming && self.menu.is_none();
            if allowed {
                for (key, action) in [
                    (K::E, Action::ToggleFlat(self.focused)),
                    (K::P, Action::TogglePreview(self.focused)),
                    // And the console's own key, which is the one that has to get through: the
                    // panel holds the keyboard while you are in it, so without this the only way
                    // out of it is the mouse.
                    (K::Backtick, Action::ToggleConsole(self.focused)),
                ] {
                    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key)) {
                        self.actions.push(action);
                    }
                }
            }
            if ctx.input(|i| i.key_pressed(K::Escape)) {
                ctx.memory_mut(|m| m.stop_text_input());
            }
            return;
        }

        let pane = self.focused;
        let mut push = |action| self.actions.push(action);

        ctx.input(|i| {
            let m = i.modifiers;

            // ---- Tabs and panes ------------------------------------------
            // `Shift` is checked here rather than left out, because `m.command` alone would
            // fire both of these on `Ctrl+Shift+T` — a new tab *and* the reopened one.
            if m.command && i.key_pressed(K::T) {
                if m.shift {
                    push(Action::ReopenTab);
                } else {
                    push(Action::NewTab { pane });
                }
            }
            if m.command && i.key_pressed(K::W) {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    push(Action::CloseTab {
                        pane,
                        tab: p.active,
                    });
                }
            }
            if m.command && i.key_pressed(K::Tab) {
                push(Action::NextTab {
                    pane,
                    delta: if m.shift { -1 } else { 1 },
                });
            }
            for (index, key) in [K::Num1, K::Num2, K::Num3, K::Num4, K::Num5, K::Num6, K::Num7, K::Num8, K::Num9]
                .into_iter()
                .enumerate()
            {
                if m.command && i.key_pressed(key) {
                    push(Action::ActivateTab { pane, tab: index });
                }
            }
            // A split of the current folder, so the feature is reachable without
            // knowing that tabs can be dragged.
            if m.command && i.key_pressed(K::Backslash) {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    push(Action::OpenInSplit {
                        pane,
                        path: p.tab().path.clone(),
                        side: Side::Right,
                    });
                }
            }

            // ---- Navigation ----------------------------------------------
            if (m.alt && i.key_pressed(K::ArrowLeft)) || i.key_pressed(K::Backspace) && !m.alt {
                push(if m.alt {
                    Action::Back(pane)
                } else {
                    Action::Up(pane)
                });
            }
            if m.alt && i.key_pressed(K::ArrowRight) {
                push(Action::Forward(pane));
            }
            if m.alt && i.key_pressed(K::ArrowUp) {
                push(Action::Up(pane));
            }
            if i.key_pressed(K::F5) || (m.command && i.key_pressed(K::R)) {
                push(Action::Refresh(pane));
            }
            if (m.command && i.key_pressed(K::L)) || (m.alt && i.key_pressed(K::D)) {
                push(Action::EditPath(pane));
            }
            if m.command && i.key_pressed(K::A) {
                push(Action::SelectAll(pane));
            }
            // No pane, unlike its neighbours: the window's preference, and every pane follows.
            if m.command && i.key_pressed(K::H) {
                push(Action::ToggleHidden);
            }
            if m.command && i.key_pressed(K::E) {
                push(Action::ToggleFlat(pane));
            }
            if m.command && i.key_pressed(K::D) && !m.alt {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    push(Action::ToggleBookmark(p.tab().path.clone()));
                }
            }
            // This folder's preview panel. On the pane the keyboard is in, since that is the
            // folder whose selection it would be showing.
            if m.command && i.key_pressed(K::P) {
                push(Action::TogglePreview(pane));
            }
            // **The leftmost key of the number row**, whatever is printed on it: `²` on an AZERTY
            // board, `` ` `` on a QWERTY one. Bound by position, which falls out of how egui
            // resolves a key rather than from anything asked for here — it takes the logical key
            // when it recognises the character and the physical code when it does not, and `²` is
            // not a character it names, so the physical `Backquote` arrives.
            if m.command && i.key_pressed(K::Backtick) {
                push(Action::ToggleConsole(pane));
            }
            // ---- Files ---------------------------------------------------
            //
            // Read as events rather than as key presses, because that is what arrives.
            // `egui-winit` recognises Ctrl+C, Ctrl+X and Ctrl+V itself and queues `Event::Copy`,
            // `Event::Cut` or `Event::Paste` *in place of* the key, so `key_pressed(K::C)` is
            // never true for a copy — which is why these three shortcuts did nothing at all
            // while looking perfectly well wired. See `paste_keystroke` in `main.rs` for the
            // paste half, which arrives only because this program puts it back.
            for event in &i.events {
                match event {
                    // Shift+Delete is a *permanent delete*, and on Windows `egui-winit`
                    // recognises it as a legacy cut: it queues `Event::Cut` for it, the very same
                    // event Ctrl+X produces, with nothing to tell them apart but the modifiers.
                    // Guarding this arm with `!m.shift` and leaving `key_pressed(Delete)` to
                    // catch the rest was how Shift+Delete came to do nothing at all — the key
                    // does not arrive either.
                    //
                    // `!m.command` is the discriminator and the direction it fails matters. With
                    // Ctrl held this is Ctrl+X, or Ctrl+Shift+X with a thumb resting on Shift,
                    // and reading either of those as "delete this for ever" would be the worst
                    // mistake this program could make. So anything ambiguous is a cut, which
                    // moves nothing until something pastes.
                    egui::Event::Cut if m.shift && !m.command => push(Action::Delete {
                        pane,
                        permanent: true,
                    }),
                    egui::Event::Cut => push(Action::Cut(pane)),
                    egui::Event::Copy => push(Action::Copy(pane)),
                    egui::Event::Paste(_) => push(Action::Paste(pane)),
                    _ => {}
                }
            }
            if i.key_pressed(K::Delete) {
                // Shift is the difference between the Recycle Bin and gone.
                push(Action::Delete {
                    pane,
                    permanent: m.shift,
                });
            }
            if i.key_pressed(K::F2) {
                push(Action::BeginRename(pane));
            }
            // **Undo and redo, of file operations** — the copy, move, rename, delete and new
            // folder this window has performed, not of anything typed. See
            // [`crate::shell::ops::history`].
            //
            // `Ctrl+Y` and `Ctrl+Shift+Z` both redo. Explorer's is `Ctrl+Y` and that is what
            // Windows users reach for; `Ctrl+Shift+Z` is what everybody who has used anything
            // else reaches for, it collides with nothing here, and a redo that answers only one
            // of the two is a redo half its users conclude is missing.
            //
            // `Shift` is tested on the undo half rather than left out, for the same reason
            // `Ctrl+Shift+T` is up at the top of this function: without it `Ctrl+Shift+Z` fires
            // both, and undo-then-redo in one keystroke is a keystroke that appears to do
            // nothing at all.
            if m.command && i.key_pressed(K::Z) {
                push(Action::Undo { redo: m.shift });
            }
            if m.command && i.key_pressed(K::Y) {
                push(Action::Undo { redo: true });
            }
            if m.command && m.shift && i.key_pressed(K::N) {
                push(Action::NewFolder(pane));
            }

            if m.command && m.shift && i.key_pressed(K::C) {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    let mut paths = p.tab().selection_paths();
                    if paths.is_empty() {
                        paths.push(p.tab().path.clone());
                    }
                    push(Action::CopyPaths(paths));
                }
            }
        });

        // ---- The listing's own keys --------------------------------------
        let Some(index) = self.panes.iter().position(|p| p.id == pane) else {
            return;
        };
        // **How far one step of the cursor goes, and through which order.**
        //
        // In the tiles view the cursor walks the order the *tiles* are in, which is the blocks the
        // view laid out rather than the display order — see [`crate::ui::grid::Layout::walk`]. That
        // is the whole of what makes `Down` on a folder go to the first tile inside it and `End` go
        // to the last thing on the pane, neither of which is a step along the display order at all.
        //
        // The two numbers here are what the same keys mean in a listing of *rows*, and they are the
        // fallback: one row for `Down`, a screenful for `PageDown`, and in the tiles view a whole
        // line of tiles for the first of them — the column count the view last laid out. `lines` is
        // how many steps a page is, which is the same question in both views.
        let (step, page, lines) = self
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| {
                let tab = p.tab();
                let height = (p.rect.height() - 80.0).max(1.0);
                if tab.view_mode.is_icons() {
                    let columns = tab.grid.columns.max(1) as isize;
                    let lines = (height / crate::ui::grid::CELL_H).max(1.0) as usize;
                    (columns, lines as isize * columns, lines)
                } else {
                    let lines = (height / crate::pane::ROW_HEIGHT).max(1.0) as usize;
                    (1, lines as isize, lines)
                }
            })
            .unwrap_or((1, 20, 20));

        let mut open: Option<(bool, PathBuf)> = None;
        let mut typed: Vec<char> = Vec::new();

        {
            let tab = self.panes[index].tab_mut();
            ctx.input(|i| {
                use crate::ui::grid::Step;

                let extend = i.modifiers.shift;
                if i.key_pressed(K::ArrowDown) {
                    tab.walk(Step::Down, 1, step, extend);
                }
                if i.key_pressed(K::ArrowUp) {
                    tab.walk(Step::Up, 1, -step, extend);
                }
                // **The page keys are the folders in a tree** and a screenful everywhere else, and
                // `Ctrl` is the difference between the next folder at any depth and the next one no
                // deeper than this — see [`Tab::page`], which is where both halves are decided.
                if i.key_pressed(K::PageDown) {
                    tab.page(true, i.modifiers.command, lines, page, extend);
                }
                if i.key_pressed(K::PageUp) {
                    tab.page(false, i.modifiers.command, lines, -page, extend);
                }
                // **`Ctrl` with either of these is the same gesture**, and on purpose: `Home` and
                // `End` are Explorer's two keys for the ends of a listing, `Ctrl+Home` and
                // `Ctrl+End` are what anybody who has used an editor or a tree reaches for, and
                // there is nothing else in this window for the pair with a modifier to mean. So
                // neither modifier is tested for, and `Shift` still extends as it does on every
                // other key here.
                if i.key_pressed(K::Home) {
                    tab.move_cursor_to_edge(false, extend);
                }
                if i.key_pressed(K::End) {
                    tab.move_cursor_to_edge(true, extend);
                }
                // **Left and Right work a tree**, which is what those two keys mean in every tree
                // control on the platform: Right opens the folder under the cursor, Left shuts it,
                // and Left on something that is not an open folder goes out to the folder it is in.
                //
                // Free to bind here because a details listing has no use for them — nothing scrolls
                // sideways — and the navigation pair is `Alt+Left` and `Alt+Right`, which is read
                // further up with the modifier. Both do nothing at all in a listing that is not a
                // tree, which is what `Tab::set_collapsed` answers `false` for.
                //
                // No `extend`: opening a branch is not a selection gesture, and `Shift+Left` in a
                // tree that grew four hundred rows would select whatever the arithmetic landed on.
                //
                // In a **grid** they are the tile beside this one instead, which is the same
                // statement from the other end: the two keys go to whichever axis the view has, and
                // a grid of tiles is the one listing here with two. **Except on a folder's row in a
                // tree**, where they stay the tree's — a row is a line to itself, so there is
                // nothing beside it to go to, and opening the branch is worth more than either key
                // could otherwise do. So in the tiles view of a tree the pair means one thing on the
                // rows and another on the tiles, which is what each of the two is drawn as.
                let on_a_row = tab.is_tree() && tab.cursor.is_some_and(|at| tab.is_dir_at(at));
                let sideways = tab.view_mode.is_icons() && !on_a_row;
                if sideways {
                    if i.key_pressed(K::ArrowRight) {
                        tab.walk(Step::Next, 1, 1, extend);
                    }
                    if i.key_pressed(K::ArrowLeft) {
                        tab.walk(Step::Prev, 1, -1, extend);
                    }
                } else {
                    // **Both keys do a second thing when the branch cannot answer them**, which is
                    // what makes the pair a way through the tree rather than two switches: a folder
                    // that is already open has nothing to open, and one that is already shut has
                    // nothing to shut, so the key moves instead. `set_collapsed_at_cursor` answering
                    // `false` is exactly that condition — and it covers a folder with nothing in it
                    // too, where neither direction has anything to do.
                    //
                    // `Right` goes to what is inside: the first thing under the folder, or the next
                    // folder along where there is nothing under it. `Left` goes back out — see
                    // [`Tab::step_out`], which is the half the two views answer differently.
                    if !extend && i.key_pressed(K::ArrowRight) && !tab.set_collapsed_at_cursor(false)
                    {
                        tab.walk(Step::Next, 1, 1, false);
                    }
                    if !extend && i.key_pressed(K::ArrowLeft) && !tab.set_collapsed_at_cursor(true) {
                        tab.step_out();
                    }
                }
                if i.key_pressed(K::Escape) {
                    tab.clear_selection();
                }
                if i.key_pressed(K::Enter) {
                    if let Some(at) = tab.cursor {
                        if let Some(path) = tab.target_at(at) {
                            open = Some((tab.is_dir_at(at), path));
                        }
                    }
                }
                // Type-ahead: anything printable that is not a shortcut.
                if !i.modifiers.command && !i.modifiers.alt {
                    for event in &i.events {
                        if let egui::Event::Text(text) = event {
                            typed.extend(text.chars());
                        }
                    }
                }
            });

            let now = ctx.input(|i| i.time);
            for ch in typed {
                tab.type_ahead(ch, now);
            }
        }

        if let Some((is_dir, path)) = open {
            self.actions.push(if is_dir {
                Action::Navigate { pane, path }
            } else {
                Action::Open(path)
            });
        }
    }
}
