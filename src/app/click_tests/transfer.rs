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
    if !crate::shell::links::write_shortcut(&link, &folder) {
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

/// A right drag asks even inside the folder the files are already in.
///
/// A left drag there means "move this to where it already is", which is nothing, and dropping
/// a folder into itself is nothing whatever the button. But right-dragging a file onto its own
/// folder is how Explorer is asked for a copy of it, and filtering that out before the question
/// was asked meant a right drag inside a folder did nothing at all -- which is the most obvious
/// way anybody would try the gesture.
#[test]
fn a_right_drag_can_land_in_the_folder_it_started_in() {
    let here = std::path::PathBuf::from(r"C:\Temp");
    let file = here.join("one.txt");
    let elsewhere = std::path::PathBuf::from(r"C:\Other\two.txt");

    // A left drag inside the same folder has nothing to do.
    assert!(App::droppable(vec![file.clone()], &here, false).is_empty());
    // The same drag with the right button is a question worth asking.
    assert_eq!(
        App::droppable(vec![file.clone()], &here, true),
        vec![file.clone()]
    );
    // A folder dropped into itself is nothing either way.
    assert!(App::droppable(vec![here.clone()], &here, true).is_empty());
    assert!(App::droppable(vec![here.clone()], &here, false).is_empty());
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
    h.app.file_drag = Some((pane, drag));

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
        h.app.file_drag = Some((pane, drag));
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
        resolved,
        Onto::Folder(folder.clone()),
        "the row resolved to {resolved:?} rather than to the folder it shows"
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
        assert_eq!(
            resolved,
            Onto::Folder(
                h.app.pane_mut(pane).expect("the pane").tab().path.clone()
            ),
            "away from a row, a drop belongs to the folder being shown"
        );
    }
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
