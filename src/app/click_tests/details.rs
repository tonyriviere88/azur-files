//! The details view: rows, columns, and what a click on one does.

use super::*;

#[test]
fn a_row_is_clickable_across_its_whole_width() {
    let mut h = Harness::new();
    let pane = h.pane_rect(0);
    let y = h.row_center(0, 0).y;

    for (label, x) in [
        ("the glyph", pane.left() + 14.0),
        ("the name", pane.left() + 120.0),
        ("the middle", pane.center().x),
        ("the far right", pane.right() - 12.0),
    ] {
        h.app.panes[0].tab_mut().clear_selection();
        h.click_at(pos2(x, y));
        assert_eq!(
            h.tab(0).selected_count,
            1,
            "clicking {label} (x={x}) did not select the row"
        );
        assert_eq!(h.tab(0).cursor, Some(0), "and it is the first row");
    }
}

#[test]
fn double_clicking_anywhere_on_a_row_opens_it() {
    let mut h = Harness::new();
    assert!(h.tab(0).is_dir_at(0), "the first row should be a folder");
    let pane = h.pane_rect(0);

    for x in [pane.left() + 120.0, pane.center().x, pane.right() - 12.0] {
        let done = h.double_click_at(pos2(x, h.row_center(0, 0).y));
        assert!(
            done.contains(&"Navigate"),
            "a double click at x={x} has to open the folder, got {done:?}"
        );
        h.app.panes[0].tab_mut().navigate(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        h.settle();
        h.wait();
    }
}

#[test]
fn clicking_below_the_rows_clears_the_selection() {
    let mut h = Harness::new();
    h.click_at(h.row_center(0, 0));
    assert_eq!(h.tab(0).selected_count, 1);

    let pane = h.pane_rect(0);
    h.click_at(pos2(pane.center().x, pane.bottom() - 40.0));
    assert_eq!(
        h.tab(0).selected_count,
        0,
        "empty space below the rows cancels a selection"
    );
}

#[test]
fn ctrl_click_adds_to_the_selection() {
    let mut h = Harness::new();
    h.click_at(h.row_center(0, 0));
    h.click_with(
        h.row_center(0, 2),
        PointerButton::Primary,
        Modifiers::COMMAND,
    );
    assert_eq!(h.tab(0).selected_count, 2);
}

#[test]
fn shift_click_selects_a_range() {
    let mut h = Harness::new();
    h.click_at(h.row_center(0, 0));
    h.click_with(h.row_center(0, 3), PointerButton::Primary, Modifiers::SHIFT);
    assert_eq!(h.tab(0).selected_count, 4);
}

/// A pane on a folder of six files that **nothing else writes to**, for the keyboard tests below.
///
/// Not the repository root the harness opens by default, and the difference is not tidiness: a
/// re-read of a folder puts the cursor and the selection back to nothing — [`Tab::set_dir`] — and
/// the folder this suite runs in is one `cargo` is writing `target/` into the whole time. A watch
/// event landing between two keystrokes made the *next* arrow start from an empty cursor, which is a
/// test that fails once a run and never in the same place twice.
fn on_a_quiet_folder() -> Harness {
    let root = crate::sandbox::fresh("keyboard-cursor");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"] {
        std::fs::write(root.join(name), b"x").expect("a file in the sandbox");
    }
    Harness::opening(vec![root])
}

/// **`Ctrl` with the arrows moves the cursor and nothing else, and `Ctrl+Space` picks the row it
/// stopped on.** The keyboard's half of building a selection out of rows that are not neighbours.
///
/// Driven through the real keyboard rather than through `Tab`, because the claim is as much about
/// the modifier reaching the listing as about what the listing does with it: `Ctrl+Up` and
/// `Ctrl+Down` are two thirds of the window shortcuts that carry the Windows key, and `Ctrl+Space`
/// arrives in the middle of the type-ahead's characters.
#[test]
fn ctrl_moves_the_cursor_without_the_selection_and_space_picks_a_row() {
    let mut h = on_a_quiet_folder();
    h.click_at(h.row_center(0, 0));
    assert_eq!((h.tab(0).cursor, h.tab(0).selected_count), (Some(0), 1));

    // ---- Ctrl+Down: the cursor moves, the selection does not ------------
    for expected in [1, 2] {
        h.chord(egui::Key::ArrowDown, Modifiers::COMMAND);
        assert_eq!(h.tab(0).cursor, Some(expected), "Ctrl+Down moves the cursor");
        assert_eq!(
            h.tab(0).selected_count, 1,
            "and leaves the selection where it was"
        );
        assert!(h.tab(0).is_selected(0), "which is still the first row");
    }
    h.chord(egui::Key::ArrowUp, Modifiers::COMMAND);
    assert_eq!(h.tab(0).cursor, Some(1), "and Ctrl+Up is the same going back");
    assert_eq!(h.tab(0).selected_count, 1);

    // ---- Ctrl+Space: flip the row under the cursor ----------------------
    h.chord(egui::Key::Space, Modifiers::COMMAND);
    assert_eq!(h.tab(0).selected_count, 2, "Ctrl+Space adds the focused row");
    assert!(h.tab(0).is_selected(1) && h.tab(0).is_selected(0), "both of them");
    assert_eq!(h.tab(0).cursor, Some(1), "and the cursor has not moved");
    // And again takes it back off, which is what makes it a toggle rather than an add.
    h.chord(egui::Key::Space, Modifiers::COMMAND);
    assert_eq!(h.tab(0).selected_count, 1, "Ctrl+Space again unpicks it");
    assert!(!h.tab(0).is_selected(1));

    // ---- A bare arrow still takes the selection with it -----------------
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(h.tab(0).cursor, Some(2));
    assert_eq!(h.tab(0).selected_count, 1, "a bare Down is one row, alone");
    assert!(h.tab(0).is_selected(2), "the one it landed on");

    // ---- And Shift still extends, with Ctrl held or not -----------------
    //
    // `Ctrl+Shift+Down` is an extend reached for with `Ctrl` already down from picking rows out one
    // at a time, so Shift wins the pair rather than the two cancelling out.
    h.chord(egui::Key::ArrowDown, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.tab(0).cursor, Some(3));
    assert_eq!(h.tab(0).selected_count, 2, "Ctrl+Shift+Down still extends");
}

/// **The dashed cursor ring is drawn on a selected row too**, which is the only thing on screen
/// that says where `Ctrl` with the arrows has left the cursor once it is inside the selection.
///
/// Asked of the pixels rather than of `Tab::cursor`, because the bug this is for is a listing whose
/// state is right and whose rows all look the same: the ring used to be suppressed on anything
/// selected, so `Ctrl+Down` through a selection drew nothing at all.
///
/// **Counted by ink rather than by where it landed.** A ring is a run of dashes, each a line segment
/// of its own, and which row they are in is a question about the scroll offset and the group headers
/// as much as about the ring — while the *colour* is the claim: [`filelist::cursor_ink`] gives the
/// ring on a selected row the row's own text colour, and every other row in the listing the grey.
#[test]
fn the_cursor_ring_shows_on_a_selected_row() {
    use crate::ui::filelist::cursor_ink;

    let mut h = on_a_quiet_folder();
    // Rows 0 and 1 selected, cursor on 1 — so the cursor is on a row that is selected.
    h.click_at(h.row_center(0, 0));
    h.click_with(h.row_center(0, 1), PointerButton::Primary, Modifiers::SHIFT);
    assert_eq!((h.tab(0).cursor, h.tab(0).selected_count), (Some(1), 2));

    let dashes = |h: &Harness, ink: egui::Color32| -> usize {
        h.segments().into_iter().filter(|&(_, colour)| colour == ink).count()
    };
    let (on_selected, off) = (
        cursor_ink(&h.app.theme, true),
        cursor_ink(&h.app.theme, false),
    );
    assert!(
        dashes(&h, on_selected) > 4,
        "the row under the cursor has to be ringed even though it is selected"
    );

    // And off the selection it is the grey one instead, which is the other half of the same rule:
    // `Ctrl+Down` onto an unselected row moves only the cursor, so the ring is all there is to see.
    let grey_before = dashes(&h, off);
    h.chord(egui::Key::ArrowDown, Modifiers::COMMAND);
    assert_eq!(
        (h.tab(0).cursor, h.tab(0).selected_count),
        (Some(2), 2),
        "Ctrl+Down has to leave the selection behind for this to be about an unselected row"
    );
    assert_eq!(
        dashes(&h, on_selected), 0,
        "and take the selected row's ring with it — one cursor, one ring"
    );
    assert!(
        dashes(&h, off) > grey_before + 4,
        "the unselected row it landed on wears the grey ring"
    );
}

#[test]
fn a_column_header_sorts() {
    let mut h = Harness::new();
    let pane = h.pane_rect(0);
    let before = (h.tab(0).sort_by, h.tab(0).ascending);
    let done = h.click_at(pos2(pane.left() + 60.0, h.header_y(0)));
    assert!(done.contains(&"Sort"), "the Name header has to sort, got {done:?}");
    assert_ne!(
        (h.tab(0).sort_by, h.tab(0).ascending),
        before,
        "and the order has to change"
    );
}

#[test]
fn every_column_header_is_reachable() {
    let mut h = Harness::new();
    let y = h.header_y(0);
    for (index, column) in crate::fs::Column::ALL.into_iter().enumerate() {
        // Status is only a column in a synced folder, and the harness's folder is not one.
        if column == crate::fs::Column::Status {
            assert_eq!(h.tab(0).widths[index], 0.0, "an unsynced folder grew a Status column");
            continue;
        }
        let id = Id::new(("th", h.app.panes[0].id, index));
        let pane = h.pane_rect(0);
        let found = (0..pane.width() as i32)
            .step_by(4)
            .map(|dx| pos2(pane.left() + dx as f32, y))
            .find(|at| h.hovers(id, *at));
        assert!(
            found.is_some(),
            "the {} header is not reachable by the pointer",
            column.header()
        );
    }
}

/// Scrolled to the end, the space under the last file is [`filelist::TAIL`].
///
/// The figure is the point of the test, but the *mechanism* is why it is worth having: the slack
/// is a row and a half, `ScrollArea::show_rows` reserves whole rows only, and `filelist::rows`
/// therefore does that function's arithmetic itself over `show_viewport`. Nothing on screen says
/// which of the two it is using — a listing virtualised wrongly looks perfect until it is scrolled
/// — so this measures the one thing that would change if it drifted back.
///
/// Measured off the rows rather than off the scroll extent, and against the body rect the pane
/// was actually given — `Pane::drop_area` is that rect — rather than one added up again from the
/// furniture's heights, which would make this a test of its own arithmetic.
#[test]
fn the_listing_leaves_half_a_row_of_slack_under_the_last_file() {
    let mut h = Harness::new();
    // Short enough that the folder overflows it, so there is an end to scroll to at all.
    h.size = egui::vec2(1024.0, 300.0);
    h.settle();

    let pane = h.app.panes[0].id;
    let body = h.app.panes[0].drop_area;
    let count = h.tab(0).order.len();
    let rows = count as f32 * crate::pane::ROW_HEIGHT;
    assert!(
        rows > body.height(),
        "this folder fits in the pane, so there is no end to scroll to"
    );

    // As far as it will go. `ScrollArea` clamps an offset to the extent it has, which is the
    // extent this is about — so asking for the whole listing's worth is asking for exactly the
    // end, whatever that turns out to be.
    h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(rows);
    h.frame(Vec::new());
    h.frame(Vec::new());

    let slack = body.bottom() - (body.top() + rows - h.tab(0).scroll_y);
    assert!(
        (slack - filelist::TAIL).abs() < 0.5,
        "at the end of the listing the gap under the last row is {slack}, not {}",
        filelist::TAIL
    );
}

/// **A row says what it is when the pointer rests on it**, including what git says.
///
/// The four columns are on the row already; what the tooltip adds is the part a column cannot hold
/// — the exact byte count, a name too long for the Name column — and the badge's meaning in words.
/// So the assertions are on the name, the exact size, and the git line, and on the tooltip *not*
/// appearing while the pointer is doing something: over a rubber band it would be in the way.
#[test]
#[cfg(windows)]
fn a_row_says_what_it_is_when_the_pointer_rests_on_it() {
    let root = crate::sandbox::dir("tip");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).unwrap();
    // A size with a grouping comma in it, and a name longer than a narrow Name column.
    std::fs::write(root.join("a-rather-long-name.txt"), vec![b'x'; 9605]).unwrap();

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

    // On the name. Not the middle of the row, which is the Keywords cell: that one holds the row's
    // tooltip back, because resting there is how the cell is made into a field — see
    // `ui::filelist::keywords`.
    let over = pos2(h.pane_rect(0).left() + 120.0, h.row_center(0, 0).y);
    // egui holds a tooltip back for `interaction.tooltip_delay` *and* until the pointer has come
    // to rest, so this moves once and then waits.
    h.frame(vec![Event::PointerMoved(over)]);
    for _ in 0..40 {
        h.frame(Vec::new());
    }
    // The tooltip is a **table**, so it is a galley per cell rather than one with newlines in it:
    // `Name` is its first key, and the row's own name is the value beside it. Found by the key,
    // because the value is a string the row itself also paints — see `tooltip_table`.
    let painted = h.texts();
    let lines: Vec<String> = painted.iter().map(|(_, text)| text.clone()).collect();
    let tip = |h: &Harness, key: &str| -> Option<(Pos2, String)> {
        // Inside the tooltip's own area and nowhere else: `Name`, `Size`, `Type` and `Modified`
        // are the column headers as well, four inches up the same window.
        let frame = h.tooltip_rect()?;
        let painted: Vec<(Pos2, String)> = h
            .texts()
            .into_iter()
            .filter(|(at, _)| frame.contains(*at))
            .collect();
        // The keys are drawn down the left-hand column and the values down the right, so the
        // value of a key is the next text along the same line.
        let (at, _) = painted.iter().find(|(_, text)| text == key)?;
        let value = painted
            .iter()
            .filter(|(pos, _)| (pos.y - at.y).abs() < 1.0 && pos.x > at.x)
            .min_by(|(a, _), (b, _)| a.x.total_cmp(&b.x))?;
        Some((*at, value.1.clone()))
    };
    let (at, name) = tip(&h, "Name")
        .unwrap_or_else(|| panic!("no tooltip over the row: {lines:?}"));
    assert_eq!(
        name, "a-rather-long-name.txt",
        "the name is the reason the tooltip exists"
    );

    // **And it is at the pointer.** The response it hangs off is the whole visible block of
    // rows, so anchored the way a button's tooltip is it would come up below the *last* row on
    // screen — hundreds of points from the row it describes, and about a row the pointer is
    // nowhere near. The bound is loose on purpose: what is being asserted is the anchor, not
    // the frame's padding or which way egui flipped it to stay on screen.
    let away = (at - over).abs();
    assert!(
        away.x < 60.0 && away.y < 60.0,
        "the tooltip is at {at:?} and the pointer is at {over:?}: {away:?} away"
    );

    // **And not under the cursor**, which is the other half of being at the pointer: the arrow
    // hangs down and to the right of the position it is pointing at, so a tooltip flush against
    // that position is a tooltip with an arrow drawn over its first word. The rect is the
    // frame's own, not the text's, because the frame is what the cursor would be seen on top of
    // — and it is below the pointer here because row 0 has the whole list under it to open into.
    let frame = h.tooltip_rect().expect("the tooltip is up, so it has an area");
    assert!(
        !frame.contains(over),
        "the pointer is inside the tooltip: {frame:?} around {over:?}"
    );
    assert!(
        frame.top() - over.y >= azur_egui_theme::components::CURSOR_CLEARANCE,
        "the tooltip clears the pointer by {}, less than the arrow is tall: {frame:?}",
        frame.top() - over.y
    );

    let size = tip(&h, "Size").expect("a file has a size").1;
    assert!(
        size.contains("9,605 bytes"),
        "the exact size is the thing the Size column cannot say: {size:?}"
    );
    assert!(
        size.contains("9.38 KB"),
        "and the rounded one is what the column does say: {size:?}"
    );
    assert!(tip(&h, "Type").is_some() && tip(&h, "Modified").is_some());
    assert!(
        tip(&h, "Git").is_none(),
        "there is no repository here, so there is no line about one"
    );
    assert!(
        tip(&h, "In").is_none(),
        "and the folder is not flattened, so every row is in it"
    );

    // And it stays out of the way of a gesture. A press starts a band, and a tooltip over a band
    // is a tooltip over the thing being dragged.
    h.frame(vec![Event::PointerButton {
        pos: over,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerMoved(over + vec2(0.0, 8.0))]);
    let while_down: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    assert!(
        !while_down
            .iter()
            .any(|text| text.contains("9,605 bytes")),
        "the tooltip is up while the button is down: {while_down:?}"
    );
    crate::sandbox::remove(&root);

    // ---- And what git says, which is a line the row can only draw as a badge ----
    //
    // In this repository, which is where the harness opens: `README.md` is a file git has an
    // opinion about whenever this suite is run from a working tree with changes in it. Skipped
    // rather than failed where git has nothing to say — a clean checkout is not a broken tooltip.
    let mut h = Harness::new();
    let row = (0..h.tab(0).order.len()).find(|&at| {
        let tab = h.tab(0);
        tab.entry_at(at)
            .zip(tab.dir.as_ref())
            .and_then(|(entry, dir)| {
                tab.git
                    .as_ref()
                    .and_then(|repo| repo.state(dir.name(entry)))
            })
            .is_some_and(|state| state != crate::git::State::Clean)
    });
    let Some(row) = row else {
        eprintln!("nothing in this working tree has changed: skipping the git half");
        return;
    };
    let name = {
        let tab = h.tab(0);
        let entry = tab.entry_at(row).expect("the row");
        tab.dir.as_ref().expect("a listing").leaf(entry).to_owned()
    };
    let over = h.row_center(0, row);
    h.frame(vec![Event::PointerMoved(over)]);
    for _ in 0..40 {
        h.frame(Vec::new());
    }
    let frame = h.tooltip_rect().expect("the tooltip is up");
    let painted: Vec<(Pos2, String)> = h
        .texts()
        .into_iter()
        .filter(|(at, _)| frame.contains(*at))
        .collect();
    let lines: Vec<String> = painted.iter().map(|(_, text)| text.clone()).collect();
    let value_of = |key: &str| -> Option<String> {
        let (at, _) = painted.iter().find(|(_, text)| text == key)?;
        painted
            .iter()
            .filter(|(pos, _)| (pos.y - at.y).abs() < 1.0 && pos.x > at.x)
            .min_by(|(a, _), (b, _)| a.x.total_cmp(&b.x))
            .map(|(_, text)| text.clone())
    };
    assert_eq!(
        value_of("Name").as_deref(),
        Some(name.as_str()),
        "no tooltip over `{name}`: {lines:?}"
    );
    let said = value_of("Git")
        .unwrap_or_else(|| panic!("`{name}` has changed and the tooltip has no `Git` line"));
    assert!(
        !said.is_empty(),
        "`{name}` has changed, and the tooltip does not say what: {said:?}"
    );
}

/// **A shortcut says where it points and what it runs it with — in the row, and in the tooltip.**
///
/// Two facts and two places for them. The Name column has one line to work in, so it runs the
/// target and the command line together after the name, dimmed; the tooltip has room to give each
/// a labelled line of its own, and it is where the *whole* target lives — the dimmed half of a
/// Name cell is the first thing that cell gives up when the column is narrow, and it is elided
/// from the front, which is exactly the case where the rest of the path is what was wanted.
///
/// Skipped rather than failed where the shell will not write a `.lnk`, the same as the other tests
/// that need a real one: no shortcut is not a broken tooltip.
#[test]
#[cfg(windows)]
fn a_shortcut_row_says_where_it_points_and_what_it_runs() {
    let root = crate::sandbox::dir("tip-lnk");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    let target = root.join("target.txt");
    std::fs::write(&target, b"x").expect("a file to point at");
    let link = root.join("shortcut.lnk");
    let arguments = "/quiet \"two words\"";
    if !crate::shell::links::write_shortcut(&link, &target, arguments) {
        eprintln!("the shell would not write a shortcut here; skipping");
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

    let row = (0..h.tab(0).order.len())
        .find(|&at| {
            let tab = h.tab(0);
            tab.entry_at(at)
                .zip(tab.dir.as_ref())
                .is_some_and(|(entry, dir)| dir.leaf(entry) == "shortcut.lnk")
        })
        .expect("the shortcut is not in its own folder's listing");
    let over = h.row_center(0, row);

    // The target is read on a worker — a `.lnk` means COM, and one pointing at an unreachable
    // share is the classic Explorer hang — so the row asks on the frame it is drawn and the
    // answer lands a frame or two later. Frames are free here; the wait is for another thread.
    for _ in 0..400 {
        h.frame(vec![Event::PointerMoved(over)]);
        if h.tab(0).links.values().any(|target| target.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        h.tab(0).links.values().any(|target| target.is_some()),
        "the shortcut was never resolved, so there is nothing for either half of this to show"
    );

    // The row's own dimmed half: the target *and* the command line, which is what makes two
    // shortcuts to one program readable as two different things.
    //
    // The middle of it is not asserted, because the path is elided from the front to fit the
    // column — which is the behaviour, and the reason the tooltip below carries the whole of it.
    // What is asserted is the shape: the name, then the target, then what it is run with.
    let shown: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    let tail = format!("tip-lnk\\target.txt {arguments}");
    assert!(
        shown
            .iter()
            .any(|text| text.starts_with("shortcut.lnk > ") && text.ends_with(&tail)),
        "the Name cell does not read `shortcut.lnk > …{tail}`: {shown:?}"
    );

    // And the tooltip, which egui holds back until the pointer has come to rest.
    for _ in 0..40 {
        h.frame(Vec::new());
    }
    let frame = h.tooltip_rect().expect("the tooltip is up over the shortcut");
    let painted: Vec<(Pos2, String)> = h
        .texts()
        .into_iter()
        .filter(|(at, _)| frame.contains(*at))
        .collect();
    let lines: Vec<String> = painted.iter().map(|(_, text)| text.clone()).collect();
    let value_of = |key: &str| -> Option<String> {
        let (at, _) = painted.iter().find(|(_, text)| text == key)?;
        painted
            .iter()
            .filter(|(pos, _)| (pos.y - at.y).abs() < 1.0 && pos.x > at.x)
            .min_by(|(a, _), (b, _)| a.x.total_cmp(&b.x))
            .map(|(_, text)| text.clone())
    };
    assert_eq!(
        value_of("Target").as_deref(),
        Some(target.to_string_lossy().as_ref()),
        "the tooltip has no `Target` line, or it is the wrong path: {lines:?}"
    );
    assert_eq!(
        value_of("Arguments").as_deref(),
        Some(arguments),
        "the command line is its own line, not the tail of the path: {lines:?}"
    );
    crate::sandbox::remove(&root);
}

/// The listing keeps [`filelist::TAIL`] of nothing under it, in a folder of any size.
///
/// The folder's own menu — the one with `New` on it — is what you get by right-clicking a
/// part of the listing that is not a file. In a folder taller than the pane
/// there was no such part: every pixel from the header to the status line was a row, and the
/// gap at the end was whatever the last row happened to leave, which was frequently nothing.
#[test]
#[cfg(windows)]
fn the_listing_keeps_room_under_it_for_the_folder() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("tall");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    for i in 0..60 {
        std::fs::write(root.join(format!("file-{i:02}.txt")), b"x").expect("a file");
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

    let body = h.app.panes[0].drop_area;
    let count = h.tab(0).order.len();
    assert!(
        count as f32 * crate::pane::ROW_HEIGHT > body.height(),
        "{count} rows fit inside the pane, so this proves nothing"
    );

    // To the end of it, which is where there used to be nothing to click.
    h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(100_000.0);
    h.frame(Vec::new());
    h.frame(Vec::new());

    let tail = h
        .ctx
        .read_response(Id::new(("rows-empty", pane)))
        .map(|r| r.rect)
        .expect("nothing is listening below the last row");
    assert!(
        (tail.height() - filelist::TAIL).abs() < 1.0,
        "the space under the last file is {:.0} points, not {:.0}",
        tail.height(),
        filelist::TAIL
    );

    let raised = h.click_with(tail.center(), PointerButton::Secondary, Modifiers::NONE);
    assert!(raised.contains(&"ShellMenu"), "{raised:?}");
    assert!(
        h.app
            .asking
            .as_ref()
            .expect("the menu is still on its way")
            .items
            .is_empty(),
        "the space under the last file gave a file's menu"
    );

    crate::sandbox::remove(&root);
}

/// **What a several-second decompression has to show for itself**, on the pixels: the figure is on
/// the status line, it displaces the arithmetic that was there, and it goes when the work does.
///
/// Read off the painted text rather than off [`App::extracting`], because the sentence existing and
/// the sentence being drawn are different claims and it is the second one that was asked for — the
/// same reason `the_measured_total_is_on_the_status_line` reads the figure back this way.
///
/// The extraction is a [`crate::archive::extract::pretend`] rather than a real one. Catching a real
/// decompression mid-flight would mean racing a worker thread from the test, and a test that waits
/// for a race is one that fails on a faster machine.
#[test]
fn an_extraction_in_flight_is_drawn_on_the_status_line() {
    let mut h = Harness::new();
    h.settle();

    let on_the_line = |h: &Harness| -> Vec<String> {
        let floor = h.pane_rect(0).bottom() - crate::ui::filelist::STATUS_HEIGHT;
        h.texts()
            .iter()
            .filter(|(at, _)| at.y > floor)
            .map(|(_, text)| text.clone())
            .collect()
    };
    // The figures it is about to replace. Asserted first so that their absence below is a change
    // rather than a fixture that never had them.
    assert!(
        on_the_line(&h).iter().any(|text| text.ends_with(" ms")),
        "the fixture's status line has no scan figure to displace: {:?}",
        on_the_line(&h)
    );

    let sentence = "Extracting 41.2 MB of 144 MB…";
    {
        let _held = crate::archive::extract::pretend(43_200_512, 151_000_000);
        h.frame(Vec::new());
        let drawn = on_the_line(&h);
        assert!(
            drawn.iter().any(|text| text == sentence),
            "`{sentence}` is not on the status line; it has {drawn:?}"
        );
        assert!(
            !drawn.iter().any(|text| text.ends_with(" ms")),
            "how long the folder took is not what somebody waiting on an archive needs: {drawn:?}"
        );
    }

    // And off again on the very next frame, so nothing is left claiming work that is over.
    h.frame(Vec::new());
    let after = on_the_line(&h);
    assert!(
        !after.iter().any(|text| text == sentence),
        "the readout outlived the extraction: {after:?}"
    );
    assert!(
        after.iter().any(|text| text.ends_with(" ms")),
        "and the figures it displaced have to come back: {after:?}"
    );
}

/// **A row found by typing its name lands two rows clear of the bottom edge**, not on it.
///
/// Brought only to the edge of the view, the row the type-ahead found sat on the listing's very last
/// line — under the status line's hairline, and cropped. So a jump asks for context; see
/// [`crate::pane::Tab::scroll_context`]. Driven through the window's own text events, so it is the
/// whole route from the keystroke to the scroll offset the frame used.
#[test]
fn a_row_found_by_typing_is_not_left_on_the_edge() {
    let root = crate::sandbox::fresh("typeahead-scroll");
    for n in 0..200 {
        std::fs::write(root.join(format!("a{n:03}.txt")), b"x").expect("a file in the sandbox");
    }
    let mut h = Harness::opening(vec![root]);
    for ch in ["a", "1", "5", "0"] {
        h.frame(vec![Event::Text(ch.to_owned())]);
    }
    // One more for the scroll the jump asked for, and one for the offset it left to be recorded.
    h.frame(Vec::new());
    h.frame(Vec::new());

    let tab = h.tab(0);
    let at = tab.cursor.expect("the type-ahead found a row");
    let name = tab.dir.as_ref().map(|dir| dir.name(tab.order[at] as usize).to_owned());
    assert_eq!(name.as_deref(), Some("a150.txt"), "it found the row that was typed");

    let rows_top = h.pane_content_top(0)
        + crate::ui::breadcrumb::HEIGHT
        + crate::ui::filelist::HEADER_HEIGHT;
    // Give or take the one hairline between the rows and the status line, which the rows' own rect
    // runs over: measured at a point, and without the context the gap is two whole rows.
    let bottom_of_view = h.pane_rect(0).bottom() - crate::ui::filelist::STATUS_HEIGHT + 1.0;
    let row = crate::pane::ROW_HEIGHT;
    // The row two below the one found, whose bottom has to be on show.
    let context_bottom = rows_top + row * (at as f32 + 1.0 + crate::ui::filelist::SCROLL_CONTEXT)
        - tab.scroll_y;
    assert!(
        context_bottom <= bottom_of_view + 0.5,
        "the found row is at the edge: two rows below it end at {context_bottom}, the view at \
         {bottom_of_view} (scrolled to {})",
        tab.scroll_y
    );
    assert!(
        rows_top + row * at as f32 - tab.scroll_y >= rows_top,
        "and the row itself is not off the top"
    );
}

/// **A click on a Keywords cell is a click on the row until the pointer has rested there.** Then the
/// same click opens the field on it, and `Escape` takes the field away with nothing kept.
///
/// See `ui::filelist::keywords` for why: the column is a quarter of every row, and a listing where a
/// quarter of each row opened a text field would be one where selecting a file was a matter of aim.
#[test]
fn a_keywords_cell_edits_only_once_the_pointer_has_rested_on_it() {
    let mut h = Harness::new();
    let keywords = crate::fs::Column::Keywords.index();
    assert!(
        h.tab(0).widths[keywords] > 0.0,
        "the repository is on NTFS, so its listing has the column"
    );
    let header = Id::new(("th", h.app.panes[0].id, keywords));
    let pane = h.pane_rect(0);
    let y = h.header_y(0);
    let left = (0..pane.width() as i32)
        .step_by(2)
        .map(|dx| pane.left() + dx as f32)
        .find(|&x| h.hovers(header, pos2(x, y)))
        .expect("the Keywords header is on screen");
    let cell = pos2(left + 16.0, h.row_center(0, 0).y);

    h.wait();
    let done = h.click_at(cell);
    assert!(!done.contains(&"BeginKeywords"), "a click on arrival edited: {done:?}");
    assert_eq!(h.tab(0).selected_count, 1, "and it has to select the row instead");
    assert!(h.tab(0).keywords.is_none());

    // The pointer stays where it is: resting on the cell, for longer than the arm.
    h.wait();
    let done = h.click_at(cell);
    assert!(done.contains(&"BeginKeywords"), "a click on a rested cell has to edit: {done:?}");
    h.frame(Vec::new());
    assert!(h.tab(0).keywords.is_some(), "and the field is open");

    h.chord(egui::Key::Escape, Modifiers::NONE);
    h.frame(Vec::new());
    assert!(h.tab(0).keywords.is_none(), "Escape takes the field away");
    assert!(
        !h.take_journal().contains(&"CommitKeywords"),
        "and keeps nothing"
    );
}
