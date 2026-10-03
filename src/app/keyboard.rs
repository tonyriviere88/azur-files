//! Every shortcut in the window, in one place.
//!
//! Read after the frame is drawn, so a field that took a keystroke has already said so and the
//! same key does not do two things at once.

use super::*;
use crate::pane::Carry;

/// Whether a Windows key is held.
///
/// `false` off Windows, and that is the honest answer rather than a gap: all three shortcuts it
/// gates are about a Win32 window's own geometry. See `win::super_down` for why the platform is asked
/// at all — egui does not carry the modifier — and for which combinations the shell keeps.
fn super_down() -> bool {
    #[cfg(windows)]
    return crate::win::super_down();
    #[cfg(not(windows))]
    false
}

impl App {
    /// Space and the arrows, for the video that has the keyboard.
    ///
    /// **Which player that is comes from the last press**, not from a focus ring: a press over a video
    /// canvas gives it the keys and a press anywhere else takes them away, decided by each canvas about
    /// itself as it is drawn — see [`crate::preview::Player::keys`]. A video filling the screen has them
    /// whether or not it was clicked, because there is nothing else on screen to have clicked.
    ///
    /// The keys are **consumed**, so the listing does not also see them: the arrows are how a selection
    /// moves, and a video that took over the keyboard without saying so would otherwise scrub *and*
    /// move the selection out from under itself. Everything else falls through untouched, so `Ctrl+P`,
    /// `F5` and the rest still work while a video has the keys.
    ///
    /// Held keys step at the player's own cadence rather than the machine's repeat rate — see
    /// [`crate::preview::video::STEP`] — which is also what makes a tap exactly one step.
    fn video_keys(&mut self, ctx: &egui::Context) {
        use egui::Key as K;

        // Read before anything is borrowed, and *not* consumed yet: whether these keys are the
        // video's depends on finding a player that wants them, and a key consumed for a player that
        // turns out not to exist is a key the listing never sees.
        //
        // **Bare keys only**, which is not fussiness: `Alt+Left` is Back and has been since long
        // before this panel existed, and a video that swallowed it would take the window's history
        // away for as long as it was on screen.
        let (space, back, forward, now) = ctx.input(|i| {
            let plain = i.modifiers.is_none();
            (
                plain && i.key_pressed(K::Space),
                plain && i.key_down(K::ArrowLeft),
                plain && i.key_down(K::ArrowRight),
                i.time,
            )
        });
        if !space && !back && !forward {
            return;
        }

        let fullscreen = self.fullscreen_video;
        for pane in self.panes.iter_mut() {
            let forced = fullscreen == Some(pane.id);
            let active = pane.active;
            let Some(tab) = pane.tabs.get_mut(active) else {
                continue;
            };
            let Some(player) = tab.preview.keyed_player(forced) else {
                continue;
            };
            if space {
                player.toggle();
            }
            // Both at once is neither, which is what a keyboard hands you when a hand moves between
            // the two — and it is cheaper to say so than to pick a winner.
            match (back, forward) {
                (true, false) => player.step(-crate::preview::video::STEP, now),
                (false, true) => player.step(crate::preview::video::STEP, now),
                _ => {}
            }
            // Held down, so the next step has to be able to arrive: this program is idle between
            // events and a key that is merely *still* down produces none.
            if back || forward {
                ctx.request_repaint();
            }
            ctx.input_mut(|i| {
                if space {
                    i.consume_key(egui::Modifiers::NONE, K::Space);
                }
                for key in [K::ArrowLeft, K::ArrowRight] {
                    i.consume_key(egui::Modifiers::NONE, key);
                }
            });
            return;
        }
    }

    /// The three window shortcuts that carry the Windows key: `Ctrl+Win+Up`, `Down` and `Left`.
    ///
    /// **Before the fields and the menus**, and unlike everything else in [`App::keyboard`] they are
    /// not stood down by a focused text field: none of the three is a thing a field could mean, and a
    /// window that could not be resized because the filter box had the caret would be a window with a
    /// mode nobody asked for. They are still *consumed*, because two of them are arrow keys and the
    /// listing moves its cursor on those — see the walk further down.
    ///
    /// # Why the modifier is read off the keyboard
    ///
    /// `egui::Modifiers` has no Windows key in it — `egui-winit` drops winit's `SUPER` — so it is
    /// sampled from the platform on the frame the arrow arrives. See `win::super_down`.
    ///
    /// # `Ctrl+Win+Left` mostly does not arrive at all
    ///
    /// `Ctrl+Win+Up` and `Ctrl+Win+Down` are keystrokes like any other. **`Ctrl+Win+Left` is Windows'
    /// own "previous virtual desktop"**, claimed by the shell before any window is offered it: there is
    /// no press to read, so there is nothing here that can make it work. A `WH_KEYBOARD_LL` hook to
    /// take the chord out of the chain first was written, tried, and did not deliver it either — see
    /// `win::super_down`, which records that so the next person does not write it again.
    ///
    /// It stays bound because it costs a line and a machine with virtual desktops switched off does
    /// deliver it. **`Ctrl+B` is the panel's shortcut**, read with the ordinary ones further down: it
    /// is the sidebar key in every editor on the machine, and nothing here wanted `B`.
    fn window_keys(&mut self, ctx: &egui::Context) {
        use egui::Key as K;

        // The cheap question first, and the reason is that the expensive one is a syscall: nothing
        // here can be true without Ctrl and an arrow, and asking the platform about the Windows key
        // on every frame of a scroll would be a syscall per frame for nothing.
        let arrow = ctx.input(|i| {
            i.modifiers.command.then(|| {
                [K::ArrowUp, K::ArrowDown, K::ArrowLeft]
                    .into_iter()
                    .find(|&key| i.key_pressed(key))
            })
        });
        let Some(arrow) = arrow.flatten() else { return };
        if !super_down() {
            return;
        }

        self.actions.push(match arrow {
            K::ArrowUp => Action::Window(WindowAction::SpanScreens),
            K::ArrowDown => Action::Window(WindowAction::ResetSize),
            _ => Action::ToggleSidebar,
        });
        // So the listing does not also see it. `Modifiers::COMMAND` and not the modifiers as they
        // arrived, because the Windows key is not among them — it is not a thing egui can match on.
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, arrow));
    }

    pub(super) fn keyboard(&mut self, ctx: &egui::Context) {
        use egui::Key as K;

        // A focused text field owns the keyboard. Escape is the one key that still
        // has to get through, or a filter box becomes a trap.
        //
        // Read here rather than where it is used, because the video's keys are asked the same
        // question first.
        // Keywords count: they are a field over a row exactly as a rename is, and re-reading the folder
        // under one is the same edit thrown away.
        let renaming = self.panes.iter().any(|p| {
            p.tabs
                .iter()
                .any(|t| t.renaming.is_some() || t.keywords.is_some())
        });
        // A menu on screen owns the keyboard: its own arrows and Enter are handled where
        // it is drawn, and the listing must not move underneath it at the same time.
        let typing =
            renaming || self.menu.is_some() || ctx.memory(|m| m.focused()).is_some();

        // **The video's own keys, before anything else can claim them.** Space and the arrows, for the
        // player that was last clicked — see [`App::video_keys`].
        //
        // Not out from under a field, though, and the reason is that the player holds the keys
        // until something else is *clicked*: `` Ctrl+` `` moves the keyboard into the console
        // without the pointer ever leaving the video, so a space typed at the prompt would pause
        // the film instead of arriving. A video filling the screen keeps them regardless — there is
        // nothing else on screen to have typed into.
        if !typing || self.fullscreen_video.is_some() {
            self.video_keys(ctx);
        }

        // **A video filling the screen takes the rest of the keyboard with it**, and gives back one
        // key.
        //
        // Everything below this line acts on a window that is not on screen: `Ctrl+W` would shut a tab
        // nobody can see, `F5` would re-read a folder nobody is looking at, and the arrows would move
        // a selection that is the only thing keeping the video open. `Escape` is the way out, and it
        // has to be here rather than further down for the same reason — the handler below it would
        // have used the key for something else first.
        if let Some(pane) = self.fullscreen_video {
            if ctx.input(|i| i.key_pressed(K::Escape)) {
                self.actions.push(Action::ToggleVideoFullscreen(pane));
            }
            return;
        }

        // **A permanent delete waiting on its yes or no owns the keyboard**, as the shell's dialog
        // does: Enter is Delete and Escape is Cancel, and nothing else reaches the listing until it
        // is answered. The keys matter more than they look — Enter is what a habit of the shell's
        // dialog presses next, and let through to the listing it would *open* the file about to be
        // deleted. See [`crate::shell::ops::fast`].
        if let Some(transfer) = self.ops.confirming() {
            use crate::shell::ops::fast::Steer;
            let none = egui::Modifiers::NONE;
            if ctx.input_mut(|i| i.consume_key(none, K::Enter)) {
                self.actions.push(Action::Steer {
                    transfer,
                    steer: Steer::Confirm(true),
                });
            } else if ctx.input_mut(|i| i.consume_key(none, K::Escape)) {
                self.actions.push(Action::Steer {
                    transfer,
                    steer: Steer::Confirm(false),
                });
            }
            return;
        }

        // **The window's own three, above the fields and the menus** — and below the fullscreen
        // return above, deliberately: resizing the window under a video that is filling the screen is
        // not something either gesture means. See [`App::window_keys`].
        self.window_keys(ctx);

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
            // **`Ctrl+1`…`9` no longer pick a tab by number**, and that was a trade rather than a
            // tidy-up: the number row's first two keys went to the pane's own switches further down,
            // and a range where `1` and `2` mean one thing and `3`…`9` another is a range nobody can
            // hold. `Ctrl+Tab` and `Ctrl+Shift+Tab` are the way between tabs, and clicking one is the
            // way to a particular tab — which is what the numbers were competing with, on a bar where
            // every tab is on screen and named. [`Action::ActivateTab`] is still what a click pushes.
            //
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
            // The panel down the left, which is the window's preference for the same reason. **The one
            // that works**: `Ctrl+Win+Left` is what was asked for and the shell takes it for switching
            // virtual desktop, so this is the binding beside it — `Ctrl+B` is the sidebar key in every
            // editor on the machine, and nothing here wanted `B`. See [`App::window_keys`].
            if m.command && i.key_pressed(K::B) {
                push(Action::ToggleSidebar);
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
            //
            // `Space` is the other way in, read further down with the type-ahead because that is
            // where the two things a space can mean are told apart.
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
            // **And the two keys beside it, for the two switches beside the console's.** `Ctrl+1` is
            // the view, `Ctrl+2` is the measurement, and with `Ctrl+²` before them the three keys are
            // the three leftmost of the number row in the order the three buttons sit in at the left
            // end of the status line — see [`crate::ui::filelist::status_line`], which is where that
            // order is decided and where each key is named in a tooltip.
            //
            // They cost `Ctrl+1` and `Ctrl+2` as tab numbers, which is noted up in the tabs block.
            //
            // The view switch **toggles**, exactly as the button does, so the key that turned the
            // tiles on is the key that turns them off — a second binding for the way back would be a
            // second thing to remember. The mode comes off the pane the keyboard is in, since that is
            // the listing being switched.
            if m.command && i.key_pressed(K::Num1) {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    push(Action::SetView {
                        pane,
                        mode: p.tab().view_mode.toggled(),
                    });
                }
            }
            // No guard for This PC here, unlike the button that is drawn disabled there:
            // [`Action::ToggleSizes`] refuses it at the other end, which is the one place the answer
            // cannot be got wrong twice.
            if m.command && i.key_pressed(K::Num2) {
                push(Action::ToggleSizes(pane));
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
        let mut panel = false;
        // A folder opened or shut from the keyboard, for the other half of a diff to follow.
        let mut followed: Option<(String, bool)> = None;

        {
            let tab = self.panes[index].tab_mut();
            let diffing = tab.diff.is_some();
            ctx.input(|i| {
                use crate::ui::grid::Step;

                let extend = i.modifiers.shift;
                // **What a cursor key does to the selection**, which is the whole of what the two
                // modifiers decide — see [`Carry`]. `Shift` grows the selection to where the cursor
                // lands, `Ctrl` moves the cursor and leaves the selection alone, and a bare key takes
                // the selection with it.
                //
                // **`Shift` wins the pair.** `Ctrl+Shift+Down` is an extend reached for on a machine
                // where `Ctrl` is already held down from picking rows out one at a time, and a
                // focus-only move that also refused to extend would be the one chord here that does
                // nothing at all.
                //
                // `shifted` is the same question **without** the third mode, for the keys where `Ctrl`
                // already means something: it is the difference between the next folder at any depth
                // and the next one no deeper than this on the page keys, and on `Home` and `End` it is
                // deliberately not read at all — see below. So `Carry::Focus` is offered by the keys
                // that had nothing else for the modifier to mean.
                let shifted = if extend { Carry::Extend } else { Carry::Select };
                let carry = if extend {
                    Carry::Extend
                } else if i.modifiers.command {
                    Carry::Focus
                } else {
                    Carry::Select
                };
                if i.key_pressed(K::ArrowDown) {
                    tab.walk(Step::Down, 1, step, carry);
                }
                if i.key_pressed(K::ArrowUp) {
                    tab.walk(Step::Up, 1, -step, carry);
                }
                // **The page keys are the folders in a tree** and a screenful everywhere else, and
                // `Ctrl` is the difference between the next folder at any depth and the next one no
                // deeper than this — see [`Tab::page`], which is where both halves are decided.
                if i.key_pressed(K::PageDown) {
                    tab.page(true, i.modifiers.command, lines, page, shifted);
                }
                if i.key_pressed(K::PageUp) {
                    tab.page(false, i.modifiers.command, lines, -page, shifted);
                }
                // **`Ctrl` with either of these is the same gesture**, and on purpose: `Home` and
                // `End` are Explorer's two keys for the ends of a listing, `Ctrl+Home` and
                // `Ctrl+End` are what anybody who has used an editor or a tree reaches for, and
                // there is nothing else in this window for the pair with a modifier to mean. So
                // neither modifier is tested for, and `Shift` still extends as it does on every
                // other key here.
                if i.key_pressed(K::Home) {
                    tab.move_cursor_to_edge(false, shifted);
                }
                if i.key_pressed(K::End) {
                    tab.move_cursor_to_edge(true, shifted);
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
                    // `carry` and not `shifted`: in the tiles view these two are `Down` and `Up` on
                    // the other axis, and a `Ctrl` that moved the cursor on one axis while resetting
                    // the selection on the other would be a gesture you cannot use to cross a grid.
                    if i.key_pressed(K::ArrowRight) {
                        tab.walk(Step::Next, 1, 1, carry);
                    }
                    if i.key_pressed(K::ArrowLeft) {
                        tab.walk(Step::Prev, 1, -1, carry);
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
                    //
                    // In a folder diff the other half follows, which needs the folder's name before
                    // the order it is looked up in is rebuilt.
                    let at_cursor = |tab: &Tab| {
                        let at = tab.cursor.filter(|_| diffing)?;
                        tab.name_at(at).map(str::to_owned)
                    };
                    for (key, shut) in [(K::ArrowRight, false), (K::ArrowLeft, true)] {
                        if extend || !i.key_pressed(key) {
                            continue;
                        }
                        let name = at_cursor(tab);
                        if tab.set_collapsed_at_cursor(shut) {
                            followed = name.map(|name| (name, shut));
                        } else if shut {
                            tab.step_out();
                        } else {
                            tab.walk(Step::Next, 1, 1, Carry::Select);
                        }
                    }
                }
                if i.key_pressed(K::Escape) {
                    tab.clear_selection();
                }
                // **`Ctrl+Space` flips the row under the cursor**, and is why `Ctrl` with the arrows
                // is worth having: a cursor that moves without the selection is only useful if
                // something can then pick out the row it stopped on. It is the keyboard's
                // `Ctrl`-click, and Explorer's key for the same thing.
                //
                // **The press, and not the repeat**, read off the events for exactly the reason the
                // bare space below is: `key_pressed` counts key-*repeat* events among its presses, so
                // a thumb resting on the bar would flip the row on and off at the machine's repeat
                // rate. `repeat: false` is the only thing that tells the two apart.
                //
                // No `Space` reaches the type-ahead with `Ctrl` held — the characters below are read
                // only when it is not — so this is the whole of what the chord does.
                if i.modifiers.command
                    && !i.modifiers.alt
                    && i.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Key {
                                key: K::Space,
                                pressed: true,
                                repeat: false,
                                ..
                            }
                        )
                    })
                {
                    tab.toggle_at_cursor();
                }
                if i.key_pressed(K::Enter) {
                    if let Some(at) = tab.cursor {
                        if let Some(path) = tab.target_at(at) {
                            open = Some((tab.is_dir_at(at), path));
                        }
                    }
                }
                // Type-ahead: anything printable that is not a shortcut.
                //
                // **`Space` is the second key for this folder's preview panel** — `Ctrl+P` is the
                // first, further up — and it is read *here*, among the characters, because this is
                // the only place that knows which of its two meanings it has. A space is a letter
                // of most names on a Windows disk, so while a word is in flight it belongs to the
                // word: `annual r` has to keep finding `Annual Report.pdf` rather than open the
                // panel halfway through typing it. That is the rule the platform's own listings
                // follow, and the second it lasts is the gap [`Tab::type_ahead`] already measures —
                // letters typed earlier in this same frame count too, which is why `word` is
                // tracked through the loop rather than asked once.
                //
                // **The press, and not only the character**: a held space goes on typing spaces,
                // which is what it has always done, and only the first of them is the panel.
                //
                // **Read off the events rather than through `key_pressed`**, which is not the same
                // question however much it looks like it: `InputState::num_presses` counts
                // key-*repeat* events too — its own documentation says so — so a thumb resting on the
                // bar reads as one press per repeat and flapped the panel open and shut at the
                // machine's repeat rate. `repeat: false` is the discriminator, and it is the only
                // thing that distinguishes the two.
                //
                // Bare only — a modified space stays the letter it was, which is nothing this listing
                // can find and so nothing that happens.
                if !i.modifiers.command && !i.modifiers.alt {
                    let space = i.modifiers.is_none()
                        && i.events.iter().any(|event| {
                            matches!(
                                event,
                                egui::Event::Key {
                                    key: K::Space,
                                    pressed: true,
                                    repeat: false,
                                    ..
                                }
                            )
                        });
                    let mut word = tab.typing_a_name(i.time);
                    for event in &i.events {
                        if let egui::Event::Text(text) = event {
                            for ch in text.chars() {
                                if ch == ' ' && space && !word {
                                    panel = true;
                                } else {
                                    word = true;
                                    typed.push(ch);
                                }
                            }
                        }
                    }
                }
            });

            let now = ctx.input(|i| i.time);
            for ch in typed {
                tab.type_ahead(ch, now);
            }
        }

        if panel {
            self.actions.push(Action::TogglePreview(pane));
        }
        if let Some((name, shut)) = followed {
            self.follow_collapse(pane, &name, shut);
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
