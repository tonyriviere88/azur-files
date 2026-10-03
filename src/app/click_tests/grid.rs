//! The tiles view: where the arrows go, and what a mode change does to the scroll.

use super::*;

/// In the grid, `Down` goes to the tile below and `Right` to the one beside it.
///
/// **Which is a different number of rows for each key**, and getting it wrong is a listing where
/// the arrow keys walk the folder in an order that has nothing to do with what is on screen. So
/// the step is measured against the column count the view actually laid out — `Layout::columns` —
/// rather than against a number restated here, and the pane is deliberately left at the harness's
/// own width so that more than one column fits.
///
/// `Home` and `End` are checked as well, because they are the two that must *not* change: the
/// first tile and the last are still the first and last rows of the order however it is arranged.
#[test]
fn in_the_grid_the_arrows_move_by_a_line_of_tiles() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    // `src`, which has enough files to fill more than one line of tiles.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(&ctx, Action::Navigate { pane, path });
    h.settle();
    h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
    h.settle();

    let columns = h.tab(0).grid.columns;
    assert!(
        columns > 1,
        "the harness's pane fits only {columns} column, so this test cannot tell a line from a row"
    );
    assert!(h.tab(0).order.len() > columns * 2, "and not enough rows to move through");

    h.frame(tap(egui::Key::Home));
    assert_eq!(h.tab(0).cursor, Some(0), "Home is the first tile");
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(h.tab(0).cursor, Some(1), "Right is the next tile along");
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(
        h.tab(0).cursor,
        Some(1 + columns),
        "Down should be a whole line of tiles"
    );
    h.frame(tap(egui::Key::ArrowUp));
    assert_eq!(h.tab(0).cursor, Some(1));
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(h.tab(0).cursor, Some(0));
    // And off the left edge of the first line is still the first tile rather than an underflow.
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(h.tab(0).cursor, Some(0));
    h.frame(tap(egui::Key::End));
    assert_eq!(h.tab(0).cursor, Some(h.tab(0).order.len() - 1), "End is the last");

    // Back in the details view the same two keys are one row and the tree's, which is the other
    // half of the rule.
    h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
    h.settle();
    h.frame(tap(egui::Key::Home));
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(h.tab(0).cursor, Some(1), "a row is one step in the details view");
}

/// **A scrolled grid switched from one flatten mode to the other still gets its pictures.**
///
/// The bug this is for, and it is a *paint-on-demand* bug rather than a caching one. Scroll a grid
/// of tiles far enough and every cell of the thumbnail atlas is held by a file you have gone past.
/// Switch the flatten mode and the tiles are all new, all their cells are held by those files, and
/// the cells only become reusable once a frame has gone by without them being drawn. Nothing on
/// screen is moving, so nothing asks for that frame — and the window sits on a grid of painted
/// glyphs until you scroll and force some frames by hand. Which is exactly what it did.
///
/// So the shape of the test is the shape of the report: fill the atlas, then switch, then run frames
/// **without touching anything** and require the pictures to arrive. `Thumbs::poll` moving to the end
/// of the frame is what makes that possible, and the repaint booked on a refused request is what
/// makes it happen when the answer needs a frame that nobody else would ask for.
///
/// **What is counted is textured quads**, which in this view is exactly "tiles with a picture": a
/// tile that has one draws an image out of the atlas, and a tile that has not draws a painted glyph
/// out of the font atlas. So the first show establishes the number, and the number has to come back.
///
/// The folder is a few hundred `.txt` files in the sandbox rather than anything real. What matters
/// is the *count* — enough tiles to fill the atlas twice over between the two views — and `.txt` is
/// the cheapest thing the shell reliably draws: no thumbnail handler, so it answers from the icon
/// every time and the test does not depend on what is in the files.
///
/// **What this does and does not prove**, because that is worth being straight about. It holds the
/// end-to-end invariant: switch a scrolled grid's mode, touch nothing, and the pictures come back.
/// It does *not* discriminate against the version of the bug that was found, on this fixture and
/// this window — 420 files over a 2560-wide pane never fills the atlas, so there is no starvation
/// for the ordering to rescue, and the test passes against both. What discriminates is
/// `shell::thumbs`' own `a_visible_cell_is_never_taken_from_the_tile_drawing_it`, which fails
/// outright on the two-frame rule this replaced. This one is here for the *next* change: it is the
/// only test that runs the whole path with frames drawn only when the window asks for them.
///
/// `#[ignore]`d because it drives several hundred real shell calls and takes a few seconds, which is
/// not what `cargo test` is for.
#[test]
#[ignore = "drives a few hundred real shell calls; run explicitly"]
fn a_scrolled_grid_that_changes_mode_still_fills_in() {
    // Twenty folders of twenty files: enough rows that a scrolled tree and the top of a list have
    // nothing in common, which is the whole condition.
    let root = crate::sandbox::dir("tiles-refill");
    for folder in 0..20 {
        let sub = root.join(format!("f{folder:02}"));
        std::fs::create_dir_all(&sub).expect("the sandbox is writable");
        for file in 0..20 {
            let at = sub.join(format!("{folder:02}-{file:02}.txt"));
            if !at.exists() {
                std::fs::write(&at, b"x").expect("the sandbox is writable");
            }
        }
    }

    let mut h = Harness::with_panes(1);
    // Big enough to want a lot of tiles at once, which is the condition the report has.
    h.size = vec2(2560.0, 1392.0);
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();

    h.app.perform(&ctx, Action::Navigate { pane, path: root });
    h.settle();
    h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.settle();
    assert!(h.tab(0).order.len() > 400, "not enough rows to fill the atlas");

    // ---- The first show, which is the case that always worked ----------
    let first = h.pictures_once_settled();
    assert!(
        first > 50,
        "only {first} tiles drew a picture on the first show — this window is too small to \
         tell the bug from the arithmetic"
    );

    // ---- Fill the atlas with files the list will not be showing --------
    h.app.perform(&ctx, Action::SetFlatMode(crate::pane::FlatMode::Tree));
    h.settle();
    for step in 1..=8 {
        h.app.panes[0].tab_mut().scroll_to = Some(step as f32 * 1200.0);
        let _ = h.pictures_once_settled();
    }

    // ---- The switch back, and then **nothing but frames** --------------
    //
    // No pointer, no keys, no scrolling. This is the whole report: the window is left alone, and it
    // has to fill itself in.
    h.app.perform(&ctx, Action::SetFlatMode(crate::pane::FlatMode::List));
    let again = h.pictures_once_settled();
    assert_eq!(
        again, first,
        "{} of {first} tiles never got a picture back after the mode changed, without the view \
         being touched",
        first.saturating_sub(again)
    );
}
