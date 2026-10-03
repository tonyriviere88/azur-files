//! The clipboard and drag and drop, through the shell's own interfaces and against real files.

use super::*;

/// **A folder shortcut opens in this window.**
///
/// Handing a `.lnk` to the shell is what it gets by default, and for one pointing at a
/// folder that means Explorer opening over the top of this program — a file manager whose
/// rows open a *different* file manager. Both gestures are driven for real here, because
/// what could break is the wiring rather than the resolving: `Enter` through the keyboard
/// path, and a middle click through the pointer's.
#[cfg(windows)]
#[test]
fn opening_a_folder_shortcut_stays_in_this_window() {
    let root = crate::sandbox::dir("open-lnk");
    let folder = root.join("somewhere");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&folder).expect("a directory in the temp folder");
    let link = root.join("somewhere.lnk");
    if !crate::shell::links::write_shortcut(&link, &folder, "") {
        println!("the shell would not write a shortcut here; skipping");
        crate::sandbox::remove(&root);
        return;
    }

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();

    let position = {
        let tab = h.app.panes[0].tab();
        let dir = tab.dir.as_ref().expect("the listing");
        tab.order
            .iter()
            .position(|&i| dir.name(i as usize) == "somewhere.lnk")
            .expect("the shortcut is in the listing")
    };
    assert!(
        h.app.panes[0].tab().is_shortcut_at(position),
        "the row is not recognised as a shortcut, so nothing below can work"
    );
    assert!(
        !h.app.panes[0].tab().is_dir_at(position),
        "a `.lnk` is a file as far as the enumeration is concerned — if it were not, this \
         test would be passing for the wrong reason"
    );

    // ---- Enter ---------------------------------------------------------
    h.app.panes[0].tab_mut().select_only(position);
    h.frame(Vec::new());
    h.take_journal();
    h.frame(vec![Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    let done = h.take_journal();
    assert!(
        done.contains(&"Navigate"),
        "Enter on a folder shortcut went to the shell instead of navigating, got {done:?}"
    );
    h.settle();
    assert_eq!(
        h.app.panes[0].tab().path,
        folder,
        "it navigated somewhere else"
    );

    // ---- And a middle click, into a tab of its own ----------------------
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();
    let tabs = h.app.panes[0].tabs.len();
    let at = h.row_center(0, position);
    h.take_journal();
    let done = h.click_with(at, PointerButton::Middle, Modifiers::NONE);
    assert!(
        done.contains(&"OpenNewTab"),
        "a middle click on a folder shortcut did nothing, got {done:?}"
    );
    h.settle();
    assert_eq!(
        h.app.panes[0].tabs.len(),
        tabs + 1,
        "no tab was opened for it"
    );
    assert_eq!(h.app.panes[0].tab().path, folder);

    crate::sandbox::remove(&root);
}

#[test]
fn dragging_a_name_picks_the_file_up_and_dragging_beside_it_bands() {
    let mut h = Harness::new();
    assert!(h.tab(0).order.len() > 4, "the crate root has rows to drag");

    // ---- From the name: the file is picked up ------------------------
    let row = h.row_center(0, 0);
    let pane = h.pane_rect(0);
    // Just past the icon, which is where the first row's name starts.
    let on_name = pos2(pane.left() + 46.0, row.y);
    let done = h.drag(on_name, pos2(on_name.x + 60.0, on_name.y + 90.0));
    assert!(
        done.contains(&"DragOut"),
        "dragging a name has to pick the file up, got {done:?}"
    );
    // The OLE drag itself is not started here and cannot be: it needs a window, to find
    // the pointer gesture it is following. What is being tested is that the gesture is
    // read as a drag of the file, which is `DragOut` being dispatched at all.
    assert!(h.app.file_drag.is_none(), "and no drag left in flight");

    // ---- From the blank space on the same row: a band ----------------
    //
    // The gap between the end of the name and the Size column, which is the widest
    // blank stretch of any row and the one a user reaches for. *Not* the far right: the
    // last column is fitted to its content, so a date long enough fills it right up to
    // the edge — and a press there is genuinely on the row's ink, as this test used to
    // discover the hard way when the glyph advances moved by a pixel.
    h.wait();
    let blank = pos2(pane.left() + pane.width() * 0.45, row.y);
    let done = h.drag(blank, pos2(blank.x - 40.0, blank.y + crate::pane::ROW_HEIGHT * 3.5));
    assert!(
        !done.contains(&"DragOut"),
        "a drag from the empty part of a row is a band, not a drag of the file: {done:?}"
    );
    assert!(
        h.tab(0).selected_count >= 3,
        "the band should have swept the rows it crossed, got {}",
        h.tab(0).selected_count
    );
}

/// Ctrl+C then Ctrl+V, in the folder you are already looking at.
///
/// The most ordinary thing anybody does with a clipboard, and the one case the first
/// end-to-end test skipped: it copied from one folder and pasted into another, which is the
/// *easy* half. Pasting into the folder the file is already in is where the shell has to be
/// told not to ask, and where a paste that quietly does nothing is hardest to notice.
///
/// Driven by real key events rather than by pushing actions, so the bindings are on trial
/// too -- `Ctrl+C` and `Ctrl+V` being wired to the right actions is part of what is claimed.
#[test]
#[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
#[cfg(windows)]
fn copy_and_paste_in_the_same_folder_makes_a_copy() {
    use crate::shell::clipboard;

    let _serialised = crate::shell::serialised();
    // The paste below has to be the shell's real one. Inside `target/sandbox`; see
    // `crate::shell::ops::FOR_REAL`.
    let _for_real = crate::shell::ops::for_real();
    crate::shell::init();
    clipboard::settle_for_tests();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("samefolder");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    std::fs::write(root.join("one.txt"), b"one").expect("write");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();

    assert_eq!(
        h.app.pane_mut(pane).expect("the pane").tab().order.len(),
        1,
        "one file to start with"
    );

    // Selected by *clicking* it, which is how anybody selects a file -- and which is the
    // difference between this test and the version that set the selection directly and
    // passed against a broken program. A click gives the row keyboard focus, and every
    // shortcut in this program was switched off while anything at all had focus.
    let body = h.app.panes[0].rect;
    let mut clicked = false;
    for step in 0..60 {
        let at = egui::pos2(body.left() + 60.0, body.top() + 40.0 + step as f32 * 4.0);
        if !body.contains(at) {
            break;
        }
        h.click_at(at);
        if h.app.pane_mut(pane).expect("the pane").tab().selected_count == 1 {
            clicked = true;
            break;
        }
    }
    assert!(clicked, "could not find the row to click");
    eprintln!("PROBE focused after the click: {:?}", h.ctx.memory(|m| m.focused()));

    /// One frame carrying what pressing Ctrl+C or Ctrl+V *actually* delivers.
    ///
    /// Not `Event::Key`. `egui-winit` recognises these combinations itself and queues
    /// `Event::Copy` or `Event::Paste` in place of the key press, returning before the key
    /// event is ever added -- so a test that synthesises `Event::Key { key: C }` is testing a
    /// keystroke this program will never receive. This one did, and it passed against a
    /// program in which Ctrl+C and Ctrl+V did nothing whatsoever.
    ///
    /// The modifiers still go on the input, since `InputState::modifiers` is what the other
    /// shortcuts read.
    fn shortcut(h: &mut Harness, event: Event) {
        h.modifiers = Modifiers {
            command: true,
            ctrl: true,
            ..Modifiers::NONE
        };
        h.frame(vec![event]);
        h.modifiers = Modifiers::NONE;
    }

    // Three times over, because once is what it managed. A file operation ends in a
    // re-read of the folder, the re-read used to clear the selection, and a cleared selection
    // is nothing to copy — so the second Ctrl+C copied nothing and everything after it was
    // a paste of whatever the first round had left on the clipboard.
    for round in 1..=3 {
        shortcut(&mut h, Event::Copy);
        assert!(
            clipboard::has_files(),
            "round {round}: Ctrl+C put nothing on the clipboard; the program said {:?}",
            h.app.notice
        );
        assert_eq!(
            h.app
                .pane_mut(pane)
                .expect("the pane")
                .tab()
                .selection_paths()
                .len(),
            1,
            "round {round}: the file stopped being selected, so there was nothing to copy"
        );

        // What `paste_keystroke` in `main.rs` puts back when the clipboard holds files rather
        // than text, which is the case that matters here.
        shortcut(&mut h, Event::Paste(String::new()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while h.app.ops.in_progress().is_some() {
            h.frame(Vec::new());
            assert!(
                std::time::Instant::now() < deadline,
                "round {round}: the paste never finished"
            );
        }
        h.settle();

        let mut names: Vec<String> = std::fs::read_dir(&root)
            .expect("read the folder back")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names.len(),
            round + 1,
            "round {round}: Ctrl+C then Ctrl+V left {names:?}; the program said {:?}",
            h.app.notice
        );
    }

    clipboard::clear();
    crate::sandbox::remove(&root);
}

/// Which action each of the clipboard events becomes, and Shift+Delete among them.
///
/// On Windows `egui-winit` recognises Ctrl+X, Ctrl+C, Ctrl+V, Ctrl+Insert, Shift+Insert *and
/// Shift+Delete* itself, and queues `Event::Cut`, `Event::Copy` or `Event::Paste` for all of
/// them -- no key event at all. So Shift+Delete arrives as a `Cut`, indistinguishable from
/// Ctrl+X except by the modifiers, and the arm that handled cuts was guarding itself with
/// `!m.shift`. Shift+Delete therefore did nothing whatsoever: the cut arm refused it and the
/// `Delete` key it was hoping for never came.
///
/// The last case is the one worth keeping: with Ctrl held it stays a cut. Reading a stray
/// Ctrl+Shift+X as "delete this for ever" would be the worst mistake this program could make,
/// so ambiguity resolves to the recoverable answer.
#[test]
fn the_clipboard_events_map_to_the_right_actions() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.select_only(0);
    }

    let fired = |h: &mut Harness, mods: Modifiers, event: Event| -> Vec<&'static str> {
        h.app.journal = Some(Vec::new());
        h.modifiers = mods;
        h.frame(vec![event]);
        h.modifiers = Modifiers::NONE;
        h.app.journal.clone().unwrap_or_default()
    };

    let ctrl = Modifiers {
        command: true,
        ctrl: true,
        ..Modifiers::NONE
    };
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let ctrl_shift = Modifiers {
        command: true,
        ctrl: true,
        shift: true,
        ..Modifiers::NONE
    };

    assert!(fired(&mut h, ctrl, Event::Copy).contains(&"Copy"));
    assert!(fired(&mut h, ctrl, Event::Cut).contains(&"Cut"));
    assert!(fired(&mut h, ctrl, Event::Paste(String::new())).contains(&"Paste"));
    // Shift+Delete: a permanent delete, not a cut.
    let shift_delete = fired(&mut h, shift, Event::Cut);
    assert!(
        shift_delete.contains(&"Delete"),
        "Shift+Delete produced {shift_delete:?}"
    );
    assert!(
        !shift_delete.contains(&"Cut"),
        "and it must not also cut: {shift_delete:?}"
    );
    // With Ctrl held it is a cut, whatever else is down.
    let both = fired(&mut h, ctrl_shift, Event::Cut);
    assert!(both.contains(&"Cut"), "Ctrl+Shift+X produced {both:?}");
    assert!(
        !both.contains(&"Delete"),
        "and it must never delete: {both:?}"
    );
}

/// **Ctrl+Z, Ctrl+Y and Ctrl+Shift+Z reach the right action, and the third does not do both.**
///
/// Driven through real frames for the reason the test above it is: what breaks in a shortcut is
/// the wiring, and wiring looks correct while doing nothing. Two things here could only be caught
/// this way.
///
/// **`Ctrl+Shift+Z` must not fire undo as well as redo.** `m.command && key_pressed(Z)` is true
/// with Shift down, so without the `redo: m.shift` on that arm the keystroke undoes and redoes in
/// one frame — which cancels out and reads exactly like a shortcut nobody wired up. The same trap
/// `Ctrl+Shift+T` has at the top of [`crate::app::App::keyboard`].
///
/// **Neither may be swallowed on the way in.** `egui-winit` turns Ctrl+C, Ctrl+X and Ctrl+V into
/// `Event::Copy`, `Event::Cut` and `Event::Paste` in place of the key — which is why those three
/// are read as events a few lines above — and a `Z` that arrived as something other than a key
/// would be just as invisible.
#[test]
fn the_undo_shortcuts_reach_undo_and_redo() {
    let mut h = Harness::new();
    h.settle();

    let fired = |h: &mut Harness, mods: Modifiers, key: egui::Key| -> Vec<&'static str> {
        h.app.journal = Some(Vec::new());
        h.modifiers = mods;
        h.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: mods,
        }]);
        h.modifiers = Modifiers::NONE;
        h.app.journal.clone().unwrap_or_default()
    };

    let ctrl = Modifiers {
        command: true,
        ctrl: true,
        ..Modifiers::NONE
    };
    let ctrl_shift = Modifiers {
        shift: true,
        ..ctrl
    };

    let undo = fired(&mut h, ctrl, egui::Key::Z);
    assert!(undo.contains(&"Undo"), "Ctrl+Z produced {undo:?}");
    assert!(!undo.contains(&"Redo"), "and only undo: {undo:?}");

    let redo = fired(&mut h, ctrl, egui::Key::Y);
    assert!(redo.contains(&"Redo"), "Ctrl+Y produced {redo:?}");

    let both = fired(&mut h, ctrl_shift, egui::Key::Z);
    assert!(both.contains(&"Redo"), "Ctrl+Shift+Z produced {both:?}");
    assert!(
        !both.contains(&"Undo"),
        "Ctrl+Shift+Z undid and redid in one frame, which does nothing at all: {both:?}"
    );

    // And with nothing in the history, the keystroke says so rather than going quiet — a Ctrl+Z
    // that does nothing and explains nothing is a Ctrl+Z people conclude is missing.
    assert_eq!(h.app.notice.as_deref(), Some("Nothing to redo"));
    fired(&mut h, ctrl, egui::Key::Z);
    assert_eq!(h.app.notice.as_deref(), Some("Nothing to undo"));
}

/// A right drag — or a copy — can land in the folder the files are already in.
///
/// A move there means "put this where it already is", which is nothing, and the pointer says so by
/// saying nothing: see [`crate::shell::dnd::does_nothing`]. A **copy** there is `one - Copy.txt`,
/// and a right drag is a question that has not been answered yet — so both of those keep what they
/// are carrying, which is what `keep` is. Filtering them out before the question was asked meant a
/// right drag inside a folder did nothing at all — the most obvious way anybody would try the
/// gesture.
#[test]
fn a_right_drag_can_land_in_the_folder_it_started_in() {
    let here = std::path::PathBuf::from(r"C:\Temp");
    let file = here.join("one.txt");
    let elsewhere = std::path::PathBuf::from(r"C:\Other\two.txt");

    // A move inside the same folder has nothing to do.
    assert!(App::droppable(vec![file.clone()], &here, false).is_empty());
    // The same drag as a copy, or with the right button, is a gesture with an answer.
    assert_eq!(
        App::droppable(vec![file.clone()], &here, true),
        vec![file.clone()]
    );
    // A folder dropped into itself is nothing either way.
    assert!(App::droppable(vec![here.clone()], &here, true).is_empty());
    assert!(App::droppable(vec![here.clone()], &here, false).is_empty());
    // And so is a folder dropped into something *inside* it. The pointer refuses that drag whole —
    // see `crate::shell::dnd::refuses` — so what is left here is the drag that named its files only
    // as it landed, and there the folder is dropped and everything else still goes.
    let inside = here.join("sub").join("deeper");
    assert!(App::droppable(vec![here.clone()], &inside, true).is_empty());
    assert_eq!(
        App::droppable(vec![here.clone(), elsewhere.clone()], &inside, false),
        vec![elsewhere.clone()],
        "the folder cannot go inside itself and the file from elsewhere still can"
    );
    // And anything from somewhere else is fine with either button.
    assert_eq!(
        App::droppable(vec![elsewhere.clone()], &here, false),
        vec![elsewhere.clone()]
    );
    // A mixed batch keeps what it can.
    assert_eq!(
        App::droppable(vec![file, elsewhere.clone()], &here, false),
        vec![elsewhere]
    );
}

/// The highlight marks the folder a drop would land in, and nothing that takes no drop.
///
/// A drop can go into the folder being shown or into any folder row in it, and which one it
/// will be is the thing worth showing. Lighting up the whole pane while the pointer sits on a
/// subfolder promises the wrong destination — and so does lighting up the column header,
/// which sorts, or the status line, which counts. The listing is the target.
///
/// Driven by setting the hover point directly, because that is what a drag does to it from
/// wherever it was started: the OLE callbacks write it and the frame reads it.
#[test]
fn the_drop_highlight_marks_the_row_and_not_the_pane() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    let rows = h.app.panes[0].drop_rows.clone();
    let (row, _) = rows
        .iter()
        .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
        .expect("`src` should be one of the folder rows");
    let scale = h.ctx.pixels_per_point();

    // Over the row: the preview covers the row.
    let at = row.center();
    h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
    let drawn = h.app.preview_rect_for_tests(pane, scale);
    assert_eq!(
        drawn,
        Some(*row),
        "over a folder row the highlight has to be that row"
    );

    // Away from any row: the listing, since that is where the drop would go.
    let pane_rect = h.app.panes[0].rect;
    let listing = h.app.panes[0].drop_area;
    assert!(
        listing.top() > pane_rect.top() && listing.bottom() < pane_rect.bottom(),
        "the listing has to stop short of the header above it and the status line below:              listing {listing:?} in pane {pane_rect:?}"
    );
    let below = rows.iter().map(|(r, _)| r.bottom()).fold(f32::MIN, f32::max);
    if below + 4.0 < listing.bottom() {
        let at = egui::pos2(listing.center().x, below + 2.0);
        h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
        assert_eq!(
            h.app.preview_rect_for_tests(pane, scale),
            Some(listing),
            "away from a row the highlight is the listing, whose folder takes the drop"
        );
    }

    // Over the column header, which sorts rather than receives: no highlight, because
    // there is no drop to promise there.
    let at = egui::pos2(listing.center().x, listing.top() - 6.0);
    h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
    assert_eq!(
        h.app.preview_rect_for_tests(pane, scale),
        None,
        "the column header is not a drop target"
    );

    // No drag, no highlight.
    h.app.drop_hover = None;
    assert_eq!(h.app.preview_rect_for_tests(pane, scale), None);
}

/// **The drag says what it is about to do, beside the pointer.**
///
/// The highlight above says *where* a drop would land; this is the other half — *what* it would do
/// when it lands there. *Copy 4 items into src* rather than a cursor with a `+` on it, which over a
/// listing full of folders leaves the interesting half of the question unanswered.
///
/// Driven through the shared block rather than by setting the field, because that is the whole
/// path: the OLE callbacks decide the sentence — see [`crate::shell::dnd::Shared::telling`] — and
/// the frame loop has to pick it up and put it on screen. Setting `drop_telling` directly would
/// test the drawing and skip the wiring, and it is the wiring that has a frame in the middle of
/// it. Which pieces of it are drawn in the accent is `ui::tests`' half of the claim.
#[test]
#[cfg(windows)]
fn a_drag_says_what_the_drop_would_do() {
    let mut h = Harness::new();
    h.settle();
    let screen = Rect::from_min_size(Pos2::ZERO, h.size);
    let scale = h.ctx.pixels_per_point();
    let physical = |at: Pos2| ((at.x * scale) as i32, (at.y * scale) as i32);

    let rows = h.app.panes[0].drop_rows.clone();
    let (row, _) = rows
        .iter()
        .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
        .expect("`src` should be one of the folder rows");
    let at = row.center();

    let told = crate::shell::dnd::Told {
        doing: crate::shell::dnd::Doing::Copy,
        refused: None,
        source: Some("4 items".to_owned()),
        target: "src".to_owned(),
    };
    h.app.drops.hover(Some(physical(at)));
    h.app.drops.tell(Some(told.clone()));
    h.frame(Vec::new());
    assert_eq!(
        h.app.drop_telling.as_ref(),
        Some(&told),
        "the frame loop did not pick the sentence up, so nothing would be drawn"
    );

    let (saying, rect) = h
        .app
        .drag_saying_for_tests(&h.ctx, screen)
        .expect("a drag with something to say has to say it");
    assert_eq!(saying, "Copy 4 items into src");
    // Below and right of the hotspot: the arrow's own ink hangs that way, so anything closer is
    // drawn underneath the cursor.
    assert!(
        rect.left() > at.x && rect.top() > at.y,
        "the words are under the pointer at {at:?} rather than clear of it: {rect:?}"
    );
    assert!(
        screen.contains_rect(rect),
        "{rect:?} is outside the window {screen:?}"
    );

    // In the bottom right corner there is no room below and to the right, and a tooltip drawn off
    // the window is a tooltip nobody reads. It goes back inside instead.
    h.app.drops.hover(Some(physical(screen.max - vec2(2.0, 2.0))));
    h.frame(Vec::new());
    let (_, corner) = h
        .app
        .drag_saying_for_tests(&h.ctx, screen)
        .expect("still a drag, still something to say");
    assert!(
        screen.contains_rect(corner),
        "in the corner the words ran off the window: {corner:?} in {screen:?}"
    );

    // And with nothing to promise — over something that takes no drop — nothing is drawn.
    h.app.drops.tell(None);
    h.frame(Vec::new());
    assert!(
        h.app.drag_saying_for_tests(&h.ctx, screen).is_none(),
        "words were drawn for a drop with nothing to say"
    );
}

/// **A refused drop still says what it would have done, and the destination stops promising it.**
///
/// The two halves of signing one: the sentence turns into *Cannot copy …* and wears a mark —
/// `ui::tests::a_refused_drop_wears_a_mark_and_an_allowed_one_does_not` is that half — and the
/// accent wash over the folder underneath **goes away**. A place lighting up to accept a drop it is
/// refusing is worse than one that says nothing at all, and it is the half that breaks quietly: the
/// highlight is worked out from the pointer's position and the zone's rectangle, neither of which
/// knows anything about a refusal.
#[test]
#[cfg(windows)]
fn a_refused_drop_stops_the_destination_promising_it() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let scale = h.ctx.pixels_per_point();

    let rows = h.app.panes[0].drop_rows.clone();
    let (row, folder) = rows
        .iter()
        .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
        .expect("`src` should be one of the folder rows");
    let at = row.center();
    h.app
        .drops
        .hover(Some(((at.x * scale) as i32, (at.y * scale) as i32)));

    // Allowed: the row is the highlight, which is what `the_drop_highlight_marks_the_row_and_not`
    // `_the_pane` covers in full.
    let told = |refused: Option<crate::shell::dnd::Refused>| crate::shell::dnd::Told {
        doing: crate::shell::dnd::Doing::Move,
        refused,
        source: Some(crate::fs::display_name(folder)),
        target: "main".to_owned(),
    };
    h.app.drops.tell(Some(told(None)));
    h.frame(Vec::new());
    assert_eq!(
        h.app.preview_rect_for_tests(pane, scale),
        Some(*row),
        "an allowed drop has to light the row it would land in"
    );

    // Refused: the words change, say which refusal it is, and the highlight goes.
    h.app
        .drops
        .tell(Some(told(Some(crate::shell::dnd::Refused::Inside))));
    h.frame(Vec::new());
    let (saying, _) = h
        .app
        .drag_saying_for_tests(&h.ctx, Rect::from_min_size(Pos2::ZERO, h.size))
        .expect("a refusal is still something to say");
    assert_eq!(saying, "Cannot move src into main, which is inside it");
    assert_eq!(
        h.app.preview_rect_for_tests(pane, scale),
        None,
        "the folder is still promising a drop it will not take"
    );
    // The sidebar's own highlight answers the same way, and from the same flag.
    assert_eq!(h.app.bookmarks_preview(scale), None);
}

/// **A drag changes nothing about how the rows it picked up are drawn.**
///
/// The gesture is drawn *at the pointer* — the stack of icons above it, the sentence below, the
/// destination lit up under it — and the listing is left exactly as it was. A row's style says
/// what the row **is**: selected, hidden, waiting on a paste. Being in the air for a second is not
/// one of those, and the pointer is where the eye already is.
///
/// **Two panes on the same folder is the case that settles it.** They are two listings of the same
/// names with separate selections, so a mark matched by name — the only thing a listing has, since
/// it holds names and not paths — lights up rows in a pane that has selected nothing. Which is
/// what this asserts by comparing the whole frame: not "no dashes" but *nothing at all* different,
/// so no treatment of a dragged row can creep back in under a different colour.
#[test]
fn a_drag_leaves_the_rows_it_picked_up_alone() {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut h = Harness::opening(vec![here.clone(), here]);
    h.settle();
    let pane = h.app.panes[0].id;

    // The first row of the first pane, picked up. Its name is in the second pane's listing too.
    h.app.panes[0].tab_mut().select_only(0);
    let items = h.app.panes[0].tab().selection_paths();
    assert_eq!(items.len(), 1, "one row selected, one path to drag");

    // Every shell icon landed and the window genuinely idle first, because what is compared below
    // is whole frames: an icon arriving replaces a row's painted glyph with an atlas quad, and that
    // is the one other thing that redraws a listing while nothing at all is happening.
    let _ = h.pictures_once_settled();

    // Then two frames of the same window, because the claim is that a frame with a drag in it is
    // identical to one without — which says nothing unless two frames without one are.
    h.frame(Vec::new());
    let look = |h: &Harness| (h.rects(), h.segments());
    let before = look(&h);
    h.frame(Vec::new());
    assert_eq!(look(&h), before, "the window is not still between frames");

    // And with the drag this window started in the air, both listings are drawn the same way.
    let (drag, _finish) = crate::shell::dnd::Drag::pretend();
    h.app.file_drag = Some(super::super::Dragging::new(pane, items, drag));
    h.frame(Vec::new());
    assert_eq!(
        look(&h),
        before,
        "a drag in flight redrew the listing it came out of"
    );
}

/// **The pane a drag started in is published, and a hushed drop is not drawn.**
///
/// The two halves this side owns of the rule in [`crate::shell::dnd::Shared::silent`] — a folder
/// over its own row in its own pane is refused without a word said about it. In between them sits
/// the drop target, which decides it: only the callbacks know where the pointer is at the moment it
/// matters, and only the drag *source* can hold the cursor back. So what is asserted here is that
/// the pane goes out and that the answer comes back and is obeyed;
/// `crate::shell::dnd::tests::the_pane_a_drag_came_out_of_hears_nothing_about_it` drives the
/// deciding through the real `IDropTarget`.
#[test]
#[cfg(windows)]
fn the_pane_a_drag_started_in_goes_out_and_a_hushed_drop_is_not_drawn() {
    let mut h = Harness::with_panes(2);
    h.settle();
    let screen = Rect::from_min_size(Pos2::ZERO, h.size);
    let scale = h.ctx.pixels_per_point();
    let (from, other) = (h.app.panes[0].id, h.app.panes[1].id);
    assert_ne!(from, other, "two panes to drag between");
    let physical = |at: Pos2| ((at.x * scale) as i32, (at.y * scale) as i32);
    let middle_of = |h: &Harness, pane: usize| h.app.panes[pane].rect.center();

    // ---- Nothing in flight: there is no pane to hush anything in ----
    let (here, there) = (middle_of(&h, 0), middle_of(&h, 1));
    h.frame(Vec::new());
    assert!(
        !h.app.drops.started_in(physical(here)),
        "a window with no drag of its own in it published a pane for one"
    );

    // ---- A folder picked up in the first pane ----
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let (drag, _finish) = crate::shell::dnd::Drag::pretend();
    h.app.file_drag = Some(super::super::Dragging::new(from, vec![folder], drag));
    h.frame(Vec::new());
    assert!(
        h.app.drops.started_in(physical(here)),
        "the pane the drag came out of was not published, so nothing can be hushed"
    );
    assert!(
        !h.app.drops.started_in(physical(there)),
        "the other pane is inside the published one, which would hush the whole window"
    );

    // ---- And the answer that comes back is obeyed ----
    h.app.drops.hover(Some(physical(here)));
    h.app.drops.tell(Some(crate::shell::dnd::Told {
        doing: crate::shell::dnd::Doing::Move,
        refused: Some(crate::shell::dnd::Refused::Itself),
        source: Some("src".to_owned()),
        target: "src".to_owned(),
    }));
    h.app.drops.be_silent(true);
    h.frame(Vec::new());
    assert!(
        h.app.drag_saying_for_tests(&h.ctx, screen).is_none(),
        "a hushed drop still put its sentence on screen"
    );
    // The refusal itself is untouched by the hush: the row must not light up promising a drop it
    // will not take.
    assert_eq!(h.app.preview_rect_for_tests(from, scale), None);

    h.app.drops.be_silent(false);
    h.frame(Vec::new());
    let (saying, _) = h
        .app
        .drag_saying_for_tests(&h.ctx, screen)
        .expect("a refusal worth saying is drawn");
    assert_eq!(saying, "Cannot move src into itself");

    // ---- And a hush with nothing to say still puts nothing on screen ----
    //
    // The other case of [`crate::shell::dnd::Shared::silent`]: a move into the folder the items are
    // already in is refused with no sentence at all — see
    // `crate::shell::dnd::tests::a_drop_that_would_do_nothing_is_not_offered` — so the highlight
    // has to stand down from the hush itself rather than from a reason it can read.
    h.app.drops.tell(None);
    h.frame(Vec::new());
    assert!(
        h.app.preview_rect_for_tests(from, scale).is_some(),
        "a drag over a folder that would take it has to promise the drop"
    );
    h.app.drops.be_silent(true);
    h.frame(Vec::new());
    assert_eq!(
        h.app.preview_rect_for_tests(from, scale),
        None,
        "a folder lit up for a drop that would do nothing"
    );
    assert!(h.app.drag_saying_for_tests(&h.ctx, screen).is_none());
}

/// A drag in flight keeps asking for frames, and ends by re-reading what a move emptied.
///
/// This is the whole reason the drag runs on a thread of its own. While one is running the
/// pointer belongs to OLE: not one mouse or keyboard event reaches winit, so nothing in
/// egui's own event flow would ever ask for a repaint — and with no repaint there is no
/// highlight of the folder the drop will land in and no sign of the selection the drag just
/// made. Feedback that only appears once the gesture is over is not feedback.
#[test]
fn a_drag_in_flight_keeps_the_window_painting() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    // Down to a window that has stopped asking for frames, which is the baseline the
    // assertion below needs: a freshly opened one is still finishing its icons and its
    // animations, and against *that* every frame looks like a repaint somebody wanted.
    assert!(h.quiesce(), "the window should settle into asking for nothing");

    let (drag, finish) = crate::shell::dnd::Drag::pretend();
    h.app.file_drag = Some(super::super::Dragging::new(pane, Vec::new(), drag));

    for _ in 0..3 {
        h.frame(Vec::new());
        assert!(
            h.ctx.has_requested_repaint(),
            "a drag in flight has to keep the frames coming, or nothing about it is visible"
        );
    }

    // A move took the files out of this folder and OLE does not say which, so it is re-read.
    finish
        .send(Some(crate::shell::clipboard::Effect::Move))
        .unwrap();
    h.frame(Vec::new());
    assert!(h.app.file_drag.is_none(), "the drag is over and let go of");
    assert!(
        h.take_journal().contains(&"Refresh"),
        "and the folder the files left is re-read"
    );
}

/// A second drag picks files up, and a third, and every one after that.
///
/// The release that ends a drag is consumed by `DoDragDrop`'s own loop and never reaches
/// this window, so egui went on believing the button was held — and a press arriving while
/// a button is already down starts no drag. One drag per window, then nothing, until some
/// unrelated click happened to put the state right.
///
/// The sequence below is the real one: press, travel, *no release*, the drag ends by
/// itself. Every earlier test released the button, which is precisely why a suite of them
/// stayed green while dragging twice did not work.
#[test]
fn a_second_drag_still_picks_the_files_up() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let body = h.app.panes[0].rect;

    for round in 0..3 {
        // Just past the icon, which is where a row's name starts and where a drag of the
        // file rather than a rubber band begins.
        let from = pos2(body.left() + 46.0, h.row_center(0, round).y);
        let done = h.drag_and_hold(from, pos2(from.x + 60.0, from.y + 90.0));
        assert!(
            done.contains(&"DragOut"),
            "round {round}: a drag of a name has to pick the file up, got {done:?}"
        );

        // The drag a real window would have started, and its end. This is the only step
        // the platform does for us and the harness cannot.
        let (drag, finish) = crate::shell::dnd::Drag::pretend();
        h.app.file_drag = Some(super::super::Dragging::new(pane, Vec::new(), drag));
        finish.send(None).unwrap();
        h.frame(Vec::new());
        h.wait();
    }
}

/// Only the two buttons that mean something drag, and the thumb buttons navigate.
///
/// A middle-button or thumb-button drag over the listing used to pick files up and start an
/// OLE drag, because `drag_started()` without a button is true for any of them.
#[test]
fn only_the_left_and_right_buttons_drag_and_the_thumbs_navigate() {
    use egui::PointerButton as B;

    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let body = h.app.panes[0].rect;

    // A middle-button drag across a row does nothing at all.
    for button in [B::Middle, B::Extra1, B::Extra2] {
        h.app.journal = Some(Vec::new());
        let from = egui::pos2(body.left() + 60.0, body.top() + 60.0);
        h.frame(vec![Event::PointerMoved(from)]);
        h.frame(vec![Event::PointerButton {
            pos: from,
            button,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for step in 1..6 {
            h.frame(vec![Event::PointerMoved(from + vec2(0.0, step as f32 * 12.0))]);
        }
        let drawn = h.app.pane_mut(pane).expect("the pane").tab().band.is_some();
        let journal = h.app.journal.clone().unwrap_or_default();
        h.frame(vec![Event::PointerButton {
            pos: from,
            button,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(
            !drawn,
            "{button:?} drew a selection band; only the left and right buttons should"
        );
        assert!(
            !journal.contains(&"DragOut"),
            "{button:?} started a file drag: {journal:?}"
        );
    }

    // The thumb buttons navigate instead.
    h.app.journal = Some(Vec::new());
    h.frame(vec![Event::PointerButton {
        pos: body.center(),
        button: B::Extra1,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    assert!(
        h.app.journal.clone().unwrap_or_default().contains(&"Back"),
        "the first thumb button should go back: {:?}",
        h.app.journal
    );
    h.app.journal = Some(Vec::new());
    h.frame(vec![Event::PointerButton {
        pos: body.center(),
        button: B::Extra2,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    assert!(
        h.app.journal.clone().unwrap_or_default().contains(&"Forward"),
        "the second thumb button should go forward: {:?}",
        h.app.journal
    );
}

/// Dropping onto a folder row means *into that folder*.
///
/// The destination used to be worked out again when the drop landed, from the pane under the
/// pointer, which made every drop go into the folder being shown. Dragging a file onto a
/// folder is the one gesture where that is exactly wrong. The zones the application publishes
/// are what the OLE callbacks answer from, so they are what this checks: a folder row has to
/// be a target of its own, and it has to win over the pane it sits in.
#[test]
#[cfg(windows)]
fn a_folder_row_is_its_own_drop_target() {
    use crate::shell::dnd::Onto;

    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    // The crate's own folder, which has `src` in it.
    let rows = h.app.panes[0].drop_rows.clone();
    assert!(
        !rows.is_empty(),
        "the listing reported no folder rows at all, so nothing can be dropped onto one"
    );
    let (row, folder) = rows
        .iter()
        .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
        .expect("`src` should be one of the folder rows");

    h.app.publish_drop_targets(&h.ctx.clone());
    let scale = h.ctx.pixels_per_point();
    let at = (
        (row.center().x * scale) as i32,
        (row.center().y * scale) as i32,
    );
    let resolved = h.app.drops.resolve(at).expect("a zone under the row");
    assert_eq!(
        resolved.onto,
        Onto::Folder(folder.clone()),
        "the row resolved to {resolved:?} rather than to the folder it shows"
    );
    // And it is named, because the same zone answers what the pointer is *told* the drop will do
    // — *Copy to src* — and the callbacks have no way to work a name out for themselves. See
    // `crate::shell::dnd::Region`.
    assert_eq!(
        resolved.name, "src",
        "the row's zone has to name the folder the row is showing"
    );

    // And the pane's own folder is still the target away from any row: the status line at the
    // bottom of the pane is inside the pane and below the last row.
    let pane_rect = h.app.panes[0].rect;
    let below = rows
        .iter()
        .map(|(row, _)| row.bottom())
        .fold(f32::MIN, f32::max);
    if below + 4.0 < pane_rect.bottom() {
        let at = (
            (pane_rect.center().x * scale) as i32,
            ((below + 2.0) * scale) as i32,
        );
        let resolved = h.app.drops.resolve(at).expect("the pane's own zone");
        let showing = h.app.pane_mut(pane).expect("the pane").tab().path.clone();
        assert_eq!(
            resolved.onto,
            Onto::Folder(showing.clone()),
            "away from a row, a drop belongs to the folder being shown"
        );
        assert_eq!(
            resolved.name,
            crate::fs::display_name(&showing),
            "the pane's own zone is named after the folder it is showing"
        );
    }
}

/// **The tab strip is a drop target, and what lands on it becomes tabs.**
///
/// The whole strip and not the tabs in it — see [`crate::shell::dnd::Onto::Tabs`] — so this asks the
/// published zones at both ends: over a tab, where dropping still means the strip, and past the
/// last one, where there is nothing else it could mean. The zone is what the OLE callbacks answer
/// `DragOver` from, so it is the thing worth checking; the sentence they build out of it is
/// `shell::dnd`'s own `a_tab_strip_promises_a_tab_and_counts_them`.
///
/// Then the drop itself, through [`crate::app::App::land`]: **several folders make several tabs**,
/// in the order they were dragged and with the last of them showing, and nothing is copied or moved
/// on the way — which is why this can run against the repository's own folders rather than in a
/// sandbox.
#[test]
#[cfg(windows)]
fn a_tab_strip_takes_folders_and_opens_them_in_tabs() {
    use crate::shell::dnd::Onto;

    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    let (strip_pane, strip) = *h
        .app
        .tab_strips
        .first()
        .expect("the window reported no tab strip at all, so nothing can be dropped on one");
    assert_eq!(strip_pane, pane, "the one pane's strip belongs to it");

    h.app.publish_drop_targets(&h.ctx.clone());
    let scale = h.ctx.pixels_per_point();
    let physical = |at: Pos2| ((at.x * scale) as i32, (at.y * scale) as i32);
    // The one tab that is there, and the empty room past the `+` at the end of the strip.
    let tab = h.app.tab_slots.first().expect("a tab on screen").rect;
    for at in [tab.center(), pos2(strip.right() - 2.0, strip.center().y)] {
        let resolved = h
            .app
            .drops
            .resolve(physical(at))
            .unwrap_or_else(|| panic!("no zone at {at:?} in the strip"));
        assert_eq!(
            resolved.onto,
            Onto::Tabs(pane),
            "{at:?} in the strip resolved to {resolved:?} rather than to the strip"
        );
        // Named in the singular, which is the far end of *Open src in a new tab*. The plural is put
        // on by the callback that counts what the drag is holding, since a zone is published before
        // there is a drag to count.
        assert_eq!(
            resolved.name, "a new tab",
            "a strip's zone has to say what a drop there makes"
        );
        // And the highlight is the whole strip, wherever in it the pointer is: one tab lit up would
        // promise a destination this program does not have.
        h.app.drop_hover = Some(physical(at));
        assert_eq!(h.app.tabs_preview(scale), Some(strip));
    }
    h.app.drop_hover = None;

    // The drop. Two folders that exist, since a tab is opened on a folder that is going to be
    // listed — and both are named, so the order they arrive in is checkable.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dropped = crate::shell::dnd::Dropped {
        items: vec![here.join("src"), here.join("assets")],
        // What the pointer was answered with over a strip, and what it means here is *nothing on
        // disk changes*: see `Onto::Tabs`.
        effect: crate::shell::clipboard::Effect::Link,
        at: physical(strip.center()),
        onto: Onto::Tabs(pane),
        asked: false,
    };
    let before = h.app.panes[0].tabs.len();
    h.app.land(&h.ctx.clone(), dropped);

    let tabs: Vec<PathBuf> = h.app.panes[0]
        .tabs
        .iter()
        .map(|tab| tab.path.clone())
        .collect();
    assert_eq!(
        tabs.len(),
        before + 2,
        "two folders dropped on the strip should be two new tabs, got {tabs:?}"
    );
    assert_eq!(
        &tabs[before..],
        &[here.join("src"), here.join("assets")],
        "the tabs have to arrive in the order the folders were dragged"
    );
    assert_eq!(
        h.app.panes[0].active,
        tabs.len() - 1,
        "the last tab opened is the one on show, as it is for every other way of opening one"
    );
    assert!(
        h.app.ops.in_progress().is_none(),
        "a drop that opens tabs started a file operation: {:?}",
        h.app.ops.in_progress()
    );
}

/// **A drag over a tab brings that tab forward**, so the folder it names can be dropped into.
///
/// A drop onto the strip opens a tab, so a tab is not itself a way into the folder it shows — and
/// without this there would be no way to reach a folder open in a tab you are not looking at: the
/// drag would have to be put down, the tab clicked, and the files picked up again. See
/// [`crate::app::App::reveal_hovered_tab`].
///
/// Driven through whole frames from the hover the OLE callbacks publish, because that is the only
/// thing that knows a drag is over the window at all: OLE has the pointer for the length of the
/// gesture, so egui sees no mouse event and `Response::hovered` is false everywhere.
#[test]
#[cfg(windows)]
fn a_drag_over_a_tab_brings_it_forward() {
    let mut h = Harness::new();
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    h.app.panes[0].tabs.push(Tab::new(here.join("src")));
    h.app.panes[0].active = 1;
    h.settle();

    let scale = h.ctx.pixels_per_point();
    let physical = |at: Pos2| ((at.x * scale) as i32, (at.y * scale) as i32);
    let first = h
        .app
        .tab_slots
        .iter()
        .find(|slot| slot.tab == 0)
        .expect("the first tab is not on screen")
        .rect;

    // A drag from another program, hovering the tab that is not the active one.
    h.app.drops.hover(Some(physical(first.center())));
    h.frame(Vec::new());
    assert_eq!(
        h.app.panes[0].active, 0,
        "hovering a tab with files in the air did not bring it forward"
    );
    assert!(
        h.take_journal().contains(&"ActivateTab"),
        "the tab came forward by some other route than the action every other caller uses"
    );

    // And it stays put while the drag hovers it, rather than being re-activated every frame.
    h.frame(Vec::new());
    assert!(
        !h.take_journal().contains(&"ActivateTab"),
        "the tab was activated again on a frame where nothing about the drag had changed"
    );

    // The drag leaving takes nothing back: what it revealed is where the window now is, exactly as
    // a click on the tab would have left it.
    h.app.drops.hover(None);
    h.frame(Vec::new());
    assert_eq!(h.app.panes[0].active, 0);
}

/// Copy, cut, paste and delete, driven the way the keyboard drives them, on real files.
///
/// The pieces are tested where they live -- `shell::clipboard` for the data object,
/// `shell::ops` for the engine. What is only testable here is the sequence: that Ctrl+C
/// puts the selection on the clipboard, that Ctrl+V into another folder brings it, that a
/// cut leaves its sources alone until something pastes and *then* empties the clipboard,
/// and that Delete goes through the shell.
///
/// Only collision-free operations and a permanent delete of nothing: every case that
/// raises a dialog is in `shell::ops`, behind `run_watching`, because a test with a modal
/// dialog up and nobody to answer it is a test that never finishes.
///
/// Ignored because it takes over the desktop's one clipboard.
#[test]
#[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
#[cfg(windows)]
fn copy_cut_paste_and_delete_end_to_end() {
    use crate::shell::clipboard::{self, Effect};

    let _serialised = crate::shell::serialised();
    // The one test allowed to hand a job to the real shell, and it says so out loud. Everything
    // below happens inside `target/sandbox/keys`; `crate::shell::ops::FOR_REAL` documents what
    // went wrong when this was the default rather than an opt-in.
    let _for_real = crate::shell::ops::for_real();
    // This thread has to be an OLE apartment before it can own the clipboard. In the real
    // program `main` does it before anything else; a test harness does not, and without it
    // `OleSetClipboard` simply refuses and a copy puts nothing anywhere.
    crate::shell::init();
    // Another clipboard test in this process may have left a live data object on the one
    // clipboard the desktop has; this lets go of it and answers what comes of that.
    crate::shell::clipboard::settle_for_tests();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("keys");
    crate::sandbox::remove(&root);
    let from = root.join("from");
    let into = root.join("into");
    std::fs::create_dir_all(&from).expect("sandbox");
    std::fs::create_dir_all(&into).expect("sandbox");
    std::fs::write(from.join("copied.txt"), b"c").expect("write");
    std::fs::write(from.join("moved.txt"), b"m").expect("write");
    std::fs::write(from.join("binned.txt"), b"b").expect("write");
    // For the context menu's Copier and Coller, which name their items and their destination
    // rather than reading either off a pane.
    std::fs::write(from.join("menu-copied.txt"), b"n").expect("write");
    let inner = from.join("target");
    std::fs::create_dir_all(&inner).expect("sandbox");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;

    /// Show a folder, and wait for its listing.
    fn show(h: &mut Harness, pane: PaneId, path: &std::path::Path) {
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: path.to_path_buf(),
            },
        );
        h.settle();
    }

    /// Select one file by name in the shown folder.
    ///
    /// By *display position*, which is what `select_only` takes -- not by entry index.
    /// The two are only the same in an unsorted, unfiltered listing, and a version of this
    /// that passed the entry index selected the wrong row or none at all.
    fn select(h: &mut Harness, pane: PaneId, name: &str) {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        let dir = tab.dir.clone().expect("a listing");
        let position = (0..tab.order.len())
            .find(|p| tab.entry_at(*p).is_some_and(|e| dir.name(e) == name))
            .unwrap_or_else(|| panic!("`{name}` is not in the listing"));
        tab.select_only(position);
        assert_eq!(
            tab.selection_paths().len(),
            1,
            "`{name}` should be the one thing selected"
        );
    }

    /// Drain this thread's message queue, which is what a real window does constantly.
    #[cfg(windows)]
    fn pump() {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    /// Run frames until every file operation has finished.
    fn settle_ops(h: &mut Harness) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while h.app.ops.in_progress().is_some() {
            h.frame(Vec::new());
            assert!(
                std::time::Instant::now() < deadline,
                "a file operation never finished"
            );
        }
        h.settle();
    }

    // ---- Ctrl+C, then Ctrl+V somewhere else ----
    show(&mut h, pane, &from);
    select(&mut h, pane, "copied.txt");
    h.app.perform(&h.ctx.clone(), Action::Copy(pane));
    let on_clipboard = clipboard::get().unwrap_or_else(|| {
        panic!(
            "Ctrl+C put nothing on the clipboard; the program said {:?}",
            h.app.notice
        )
    });
    assert_eq!(on_clipboard.effect, Effect::Copy);

    show(&mut h, pane, &into);
    h.app.perform(&h.ctx.clone(), Action::Paste(pane));
    settle_ops(&mut h);
    assert!(into.join("copied.txt").is_file(), "the paste should have copied it; the program said {:?}", h.app.notice);
    assert!(from.join("copied.txt").is_file(), "and left the original");
    assert!(
        clipboard::has_files(),
        "a copy stays on the clipboard, so it can be pasted twice"
    );

    // ---- Ctrl+X, then Ctrl+V ----
    show(&mut h, pane, &from);
    select(&mut h, pane, "moved.txt");
    pump();
    h.app.perform(&h.ctx.clone(), Action::Cut(pane));
    assert_eq!(
        clipboard::get().map(|p| p.effect),
        Some(Effect::Move),
        "a cut has to say so, or a paste would copy; the program said {:?}",
        h.app.notice
    );
    assert!(
        from.join("moved.txt").is_file(),
        "a cut moves nothing on its own -- that is the whole difference from a move"
    );
    assert!(
        !h.app.cut.is_empty(),
        "and the sources have to be marked, so they can be drawn as pending"
    );

    show(&mut h, pane, &into);
    h.app.perform(&h.ctx.clone(), Action::Paste(pane));
    settle_ops(&mut h);
    assert!(into.join("moved.txt").is_file(), "the paste should have moved it; the program said {:?}", h.app.notice);
    assert!(!from.join("moved.txt").exists(), "and taken it out of the source");
    assert!(
        !clipboard::has_files(),
        "a cut that has been pasted has to leave the clipboard empty, or Ctrl+V again \
         would move files that are no longer where it says"
    );
    assert!(h.app.cut.is_empty(), "and nothing is pending any more");

    // ---- The context menu's Copier and Coller ----
    //
    // The pair the shell's `copy` and `paste` verbs are redirected into -- see
    // `ours_rather_than_the_shell_s`. They differ from Ctrl+C and Ctrl+V in exactly one way and
    // it is the thing worth a test: they act on what the *menu* named. So the pane stays on
    // `from` throughout, and the paste has to land in the selected folder rather than in the
    // folder being shown -- which is what `Action::Paste(pane)` would have done, and what a
    // redirect that reached for the pane instead of the entry would silently do.
    show(&mut h, pane, &from);
    h.app.perform(
        &h.ctx.clone(),
        Action::CopyItems(vec![from.join("menu-copied.txt")]),
    );
    assert_eq!(
        clipboard::get().map(|p| p.effect),
        Some(Effect::Copy),
        "the menu's Copier put nothing on the clipboard; the program said {:?}",
        h.app.notice
    );
    h.app
        .perform(&h.ctx.clone(), Action::PasteIntoFolder(inner.clone()));
    settle_ops(&mut h);
    assert!(
        inner.join("menu-copied.txt").is_file(),
        "the menu's Coller should have pasted into the selected folder; the program said {:?}",
        h.app.notice
    );
    assert!(
        from.join("menu-copied.txt").is_file(),
        "and left the original where it was"
    );
    // Nothing landed in the folder the pane was showing. A paste that reached for the pane
    // would have copied the file onto itself and left a `menu-copied (2).txt` beside it.
    let strays: Vec<String> = std::fs::read_dir(&from)
        .expect("read the source folder back")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("menu-copied") && name != "menu-copied.txt")
        .collect();
    assert!(
        strays.is_empty(),
        "the paste also went into the folder the pane was showing: {strays:?}"
    );
    clipboard::clear();

    // ---- Delete ----
    show(&mut h, pane, &from);
    select(&mut h, pane, "binned.txt");
    h.app.perform(
        &h.ctx.clone(),
        Action::Delete {
            pane,
            permanent: false,
        },
    );
    settle_ops(&mut h);
    assert!(
        !from.join("binned.txt").exists(),
        "Delete should have sent it to the Recycle Bin"
    );

    clipboard::clear();
    crate::sandbox::remove(&root);
}

/// **A fast copy's conflict is answered from its panel**, by a click, and the copy goes on.
///
/// The one thing about the panel a reading of it cannot settle: whether its buttons are reachable
/// in a frame that also has the panes under them. Real files, inside `target/sandbox`, and a real
/// job through `Operations` — the engine is `CopyFile2`, so nothing here asks the shell for
/// anything unless the engine declines, which `the_shell_keeps_what_is_its_own` covers.
#[cfg(windows)]
#[test]
fn a_fast_copy_asks_on_its_panel_and_takes_the_answer_clicked() {
    let root = crate::sandbox::fresh("fast-copy-panel");
    let file = root.join("src").join("one.txt");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "incoming").unwrap();
    let dest = root.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("one.txt"), "existing").unwrap();

    let _for_real = crate::shell::ops::for_real();
    let mut h = Harness::new();
    h.app.ops.set_fast(true);
    let ctx = h.ctx.clone();
    h.app.ops.start(
        crate::shell::ops::Job::Copy {
            items: vec![file],
            into: dest.clone(),
        },
        crate::shell::Owner::default(),
        &ctx,
    );

    let find = |h: &Harness, want: &str| {
        h.texts()
            .into_iter()
            .find(|(_, t)| t == want)
            .map(|(pos, _)| pos)
    };
    let mut keep = None;
    for _ in 0..200 {
        h.frame(Vec::new());
        keep = find(&h, "Keep both");
        if keep.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let keep = keep.expect("the panel never asked about one.txt");
    assert!(
        find(&h, "one.txt is already in dest").is_some(),
        "the question does not name the file"
    );
    h.click_at(keep + vec2(4.0, 4.0));

    let kept = dest.join("one (2).txt");
    for _ in 0..200 {
        h.frame(Vec::new());
        if kept.exists() && h.app.ops.transfers().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "incoming");
    assert_eq!(std::fs::read_to_string(dest.join("one.txt")).unwrap(), "existing");
    assert!(
        h.app.ops.transfers().is_empty(),
        "a copy that did everything is still on its panel"
    );
}

/// **Closing the window during a fast copy asks first**, rather than killing the copy mid-file.
///
/// The close is held off with `CancelClose`, the question goes up on the panel, and *Stop and close*
/// cancels the copy and closes the window only once the copy has stopped. The copy is held on a
/// conflict question, which is what keeps it running for as long as the test needs it to be.
#[cfg(windows)]
#[test]
fn closing_during_a_fast_copy_asks_and_closes_once_it_has_stopped() {
    let root = crate::sandbox::fresh("fast-copy-close");
    let file = root.join("src").join("one.txt");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "incoming").unwrap();
    let dest = root.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("one.txt"), "existing").unwrap();

    let _for_real = crate::shell::ops::for_real();
    let mut h = Harness::new();
    h.app.ops.set_fast(true);
    let ctx = h.ctx.clone();
    h.app.ops.start(
        crate::shell::ops::Job::Copy {
            items: vec![file],
            into: dest.clone(),
        },
        crate::shell::Owner::default(),
        &ctx,
    );
    let find = |h: &Harness, want: &str| {
        h.texts()
            .into_iter()
            .find(|(_, t)| t == want)
            .map(|(pos, _)| pos)
    };
    for _ in 0..200 {
        h.frame(Vec::new());
        if find(&h, "Keep both").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.app.ops.copying(), "the copy is not running");

    let closes = |h: &Harness| {
        h.commands
            .iter()
            .filter(|c| **c == egui::ViewportCommand::Close)
            .count()
    };
    h.commands.clear();
    h.window_events.push(egui::ViewportEvent::Close);
    h.frame(Vec::new());
    assert!(
        h.commands.contains(&egui::ViewportCommand::CancelClose),
        "the close went ahead with the copy still running"
    );
    h.frame(Vec::new());
    let stop = find(&h, "Stop and close").expect("the panel does not ask about closing");
    // Where it was a frame ago. A panel that moves between frames is one whose buttons cannot be
    // pressed: the release lands somewhere else than the press did.
    h.frame(Vec::new());
    assert_eq!(find(&h, "Stop and close"), Some(stop), "the panel is moving");
    assert_eq!(closes(&h), 0);

    let pressed = h.click_at(stop + vec2(4.0, 4.0));
    assert!(pressed.contains(&"Leave"), "the button did not answer: {pressed:?}");
    for _ in 0..200 {
        h.frame(Vec::new());
        if closes(&h) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!h.app.ops.copying(), "the copy was not stopped");
    assert_eq!(closes(&h), 1, "the window did not close once the copy had stopped");
    // Stopped at the question, so nothing was written over.
    assert_eq!(std::fs::read_to_string(dest.join("one.txt")).unwrap(), "existing");
    assert!(!dest.join("one (2).txt").exists());
}
