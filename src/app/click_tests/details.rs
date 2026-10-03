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

    let over = h.row_center(0, 0);
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

/// The listing keeps three rows of nothing under it, in a folder of any size.
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
        (tail.height() - 3.0 * crate::pane::ROW_HEIGHT).abs() < 1.0,
        "the space under the last file is {:.0} points, not three rows",
        tail.height()
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
