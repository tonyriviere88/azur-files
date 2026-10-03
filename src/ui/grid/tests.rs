use super::*;
use crate::fs::dir::{DirBuilder, FLAG_DIR};
use crate::pane::FlatMode;
use std::sync::Arc;

/// A flat listing of `count` files, and a tab showing it as tiles.
fn flat(count: usize) -> Tab {
    let mut builder = DirBuilder::new(r"C:\x");
    for i in 0..count {
        builder.push(&format!("file{i:03}.png"), 10, 0, 0);
    }
    let mut tab = Tab::new(r"C:\x");
    tab.apply(Arc::new(builder.finish(0)));
    tab
}

/// The flat grid is arithmetic and nothing else: as many columns as fit, as many lines as it
/// takes, and **no copy of the display order**.
///
/// That last one is the claim worth pinning. A `cells` vector for the common case would be a
/// second copy of every row — 1.2 MB for a flattened `C:\Program Files` — to record that cell
/// *n* is position *n*. An empty `cells` is what says so, and this is what would catch a copy
/// coming back.
#[test]
fn a_flat_folder_is_one_grid_and_costs_no_second_order() {
    let tab = flat(50);
    let mut layout = Layout::default();
    // Wide enough for four columns and no more: `4 * CELL_W` plus the two gutters, and a point
    // short of the fifth.
    let width = space::S3 * 2.0 + CELL_W * 4.0 + CELL_W * 0.5;
    layout.ensure(&tab, width);

    assert_eq!(layout.columns, 4, "four tiles fit across {width}");
    assert!(layout.cells.is_empty(), "a flat listing needs no second order");
    assert_eq!(layout.blocks.len(), 1, "one grid over the whole folder");
    // Fifty files in four columns is thirteen lines, the last of them holding two.
    assert_eq!(layout.height(), 13.0 * CELL_H);

    // Every position is somewhere, and on the line the division says.
    for position in 0..50 {
        let (top, height) = layout.locate(position).expect("every tile is placed");
        assert_eq!(height, CELL_H);
        assert_eq!(top, (position / 4) as f32 * CELL_H);
    }
    assert!(layout.locate(50).is_none(), "and nothing past the end is");
}

/// A pane too narrow for one tile still gets one column rather than none.
///
/// `columns_in` divides, and a division that can answer zero is a `div_ceil` by zero and a
/// `chunks(0)` panic waiting for somebody to drag a splitter far enough. Reachable: a pane goes
/// down to 96 points before `App::pane` stops drawing its contents at all.
#[test]
fn a_pane_too_narrow_for_a_tile_still_has_one_column() {
    for width in [0.0, 1.0, 20.0, CELL_W - 1.0] {
        assert_eq!(columns_in(width, 0), 1, "at {width} wide");
        assert!(step_of(width, 0, 1) >= CELL_W - 0.01);
    }
    let tab = flat(3);
    let mut layout = Layout::default();
    layout.ensure(&tab, 10.0);
    assert_eq!(layout.columns, 1);
    assert_eq!(layout.height(), 3.0 * CELL_H);
}

/// **A folder's files come immediately after its own row, and before the folders under it.**
///
/// The arrangement the whole tree mode is: `a`'s row, then `a`'s files, then `a\b`'s row, then
/// `a\b`'s files. The alternative — a folder's grid after its whole subtree — puts the contents
/// of the folder you just opened below everything inside everything inside it, which is the bug
/// this test exists to stop somebody "tidying" the layout into.
///
/// It also pins the second rule: the **listed** folder's own files are the first thing on the
/// pane, since there is no row above them to follow.
#[test]
fn a_folder_row_is_followed_by_its_own_files_and_then_by_its_folders() {
    let mut builder = DirBuilder::new(r"C:\x");
    builder.push("root.txt", 1, 0, 0);
    builder.push("a", 0, 0, FLAG_DIR);
    builder.push(r"a\one.txt", 1, 0, 0);
    builder.push(r"a\two.txt", 1, 0, 0);
    builder.push(r"a\b", 0, 0, FLAG_DIR);
    builder.push(r"a\b\deep.txt", 1, 0, 0);
    let dir = Arc::new(builder.finish(0));

    let mut tab = Tab::new(r"C:\x");
    tab.flat = true;
    tab.flat_mode = FlatMode::Tree;
    // Off, or `a\b` with one file in it would be merged into `a`'s row and there would be no
    // second row to check the ordering against.
    tab.regroup = false;
    tab.apply(dir.clone());

    let mut layout = Layout::default();
    layout.ensure(&tab, 800.0);
    assert!(!layout.cells.is_empty(), "a tree gathers each folder's files");

    // What the pane is, block by block, as names.
    let named: Vec<String> = layout
        .blocks
        .iter()
        .map(|block| match *block {
            Block::Row { position, .. } => {
                let entry = tab.entry_at(position as usize).expect("a row is a row");
                format!("row {}", dir.name(entry))
            }
            Block::Grid { from, count, .. } => {
                let files: Vec<&str> = (0..count as usize)
                    .filter_map(|i| layout.cell_at(from, i))
                    .filter_map(|position| tab.entry_at(position))
                    .map(|entry| dir.leaf(entry))
                    .collect();
                format!("grid {}", files.join(","))
            }
        })
        .collect();

    assert_eq!(
        named,
        vec![
            "grid root.txt",
            "row a",
            "grid one.txt,two.txt",
            r"row a\b",
            "grid deep.txt",
        ],
        "the blocks came out in the wrong order"
    );

    // And the heights add up: three rows' worth of grid at one line each, two folder rows.
    assert_eq!(layout.height(), CELL_H * 3.0 + ROW_HEIGHT * 2.0);
    // Every row and every tile can be found again, which is what scrolling the cursor into view
    // asks for.
    for position in 0..tab.order.len() {
        assert!(
            layout.locate(position).is_some(),
            "position {position} is not on the pane"
        );
    }
}

/// The tree the two navigation tests are written against.
///
/// Three files in the listed folder and three in `a`, so both of those grids have a full line and a
/// short one at two columns — which is what makes `Down` inside a grid a different answer from
/// `Down` off the bottom of one. `a\b` is a second level, `z` a second branch, and `zz` **an empty
/// folder at the end**: that last one is what makes the end of the pane a folder's row rather than a
/// tile, which a fixture of files could not check.
fn a_tree() -> (Tab, Arc<crate::fs::Dir>) {
    let mut builder = DirBuilder::new(r"C:\x");
    for name in ["top-a.txt", "top-b.txt", "top-c.txt"] {
        builder.push(name, 1, 0, 0);
    }
    builder.push("a", 0, 0, FLAG_DIR);
    for name in ["f1.txt", "f2.txt", "f3.txt"] {
        builder.push(&format!(r"a\{name}"), 1, 0, 0);
    }
    builder.push(r"a\b", 0, 0, FLAG_DIR);
    builder.push(r"a\b\deep.txt", 1, 0, 0);
    builder.push("z", 0, 0, FLAG_DIR);
    builder.push(r"z\last.txt", 1, 0, 0);
    builder.push("zz", 0, 0, FLAG_DIR);
    let dir = Arc::new(builder.finish(0));

    let mut tab = Tab::new(r"C:\x");
    tab.flat = true;
    tab.flat_mode = FlatMode::Tree;
    // Off, or `a\b` would be merged into `a`'s row and the pane would be a level shallower than
    // what these are about.
    tab.regroup = false;
    tab.apply(dir.clone());
    (tab, dir)
}

/// **Where the arrow keys go in a tree, all of it, as one table.**
///
/// The claim is that the four keys walk *what is on the pane* and not the display order, which in a
/// tree are two different arrangements of the same rows — a folder's own files are drawn between its
/// row and its subfolders, and the listed folder's files are drawn above everything. So the reading
/// order below is nothing like `0, 1, 2, …`, and every position in it is named rather than numbered:
/// a test written against display positions would pass against the very bug this is for.
///
/// Two columns, at a width picked for it, because the vertical pair is the only part of this that
/// depends on the geometry: `Down` is a line of tiles inside a grid and the block below it at the
/// bottom of one, and a test at one column could not tell those apart.
#[test]
fn the_arrows_walk_the_pane_and_not_the_display_order() {
    let (tab, dir) = a_tree();

    // Two columns at the top level and two one level in, which is what the assertions below are
    // written against — asked of the arithmetic rather than restated, so a change to `CELL_W` or to
    // the indent makes this fail here instead of somewhere in the middle of the table.
    let width = space::S3 * 2.0 + filelist::INDENT + CELL_W * 2.0 + 1.0;
    assert_eq!(columns_in(width, 0), 2, "at {width} wide");
    assert_eq!(columns_in(width, 1), 2);
    let mut layout = Layout::default();
    layout.ensure(&tab, width);

    let name = |position: usize| -> String {
        tab.entry_at(position)
            .map(|entry| dir.name(entry).to_owned())
            .unwrap_or_else(|| format!("nothing at {position}"))
    };
    let named = |position: Option<usize>| position.map_or_else(|| "-".to_owned(), &name);
    let step = |from: &str, step: Step| -> String {
        let at = (0..tab.order.len())
            .find(|&at| name(at) == from)
            .unwrap_or_else(|| panic!("no `{from}` in the listing"));
        named(layout.walk(at, step, 1))
    };

    // ---- The reading order, walked from one end and then the other ------
    //
    // `Right` from the first place to the last is the pane, in order. Note what it is: the listed
    // folder's three files, *then* `a` — whose display positions are 9, 10, 11 and 0.
    let mut order = vec![named(layout.first())];
    while let Some(&last) = order.last().as_ref() {
        let at = (0..tab.order.len())
            .find(|&at| name(at) == *last)
            .expect("a place on the pane is a row in the listing");
        match layout.walk(at, Step::Next, 1) {
            Some(next) => order.push(name(next)),
            None => break,
        }
    }
    assert_eq!(
        order,
        vec![
            "top-a.txt",
            "top-b.txt",
            "top-c.txt",
            "a",
            r"a\f1.txt",
            r"a\f2.txt",
            r"a\f3.txt",
            r"a\b",
            r"a\b\deep.txt",
            "z",
            r"z\last.txt",
            "zz",
        ],
        "`Right` does not walk the pane in reading order"
    );
    assert_eq!(named(layout.first()), "top-a.txt", "the first place is a file");
    assert_eq!(named(layout.last()), "zz", "and the last one is a folder");

    // And `Left` from the last place is the same thing backwards, which is the property that makes
    // the pair reversible — a cursor that cannot be put back where it was is worse than a slow one.
    let mut back = vec![named(layout.last())];
    while let Some(&last) = back.last().as_ref() {
        let at = (0..tab.order.len())
            .find(|&at| name(at) == *last)
            .expect("a place on the pane is a row in the listing");
        match layout.walk(at, Step::Prev, 1) {
            Some(prev) => back.push(name(prev)),
            None => break,
        }
    }
    back.reverse();
    assert_eq!(back, order, "`Left` is not `Right` the other way round");

    // ---- Left and Right at the edges of a grid --------------------------
    //
    // The two the user meets first: out of the front of a folder's files is that folder, and off
    // the end of them is whatever row comes next.
    assert_eq!(step(r"a\f1.txt", Step::Prev), "a", "left off the first file");
    assert_eq!(step(r"a\f3.txt", Step::Next), r"a\b", "right off the last one");

    // ---- Up and Down, which is where the columns come in ----------------
    for (from, up, down) in [
        // The listed folder's grid: two on the first line, one on the second. Down from either of
        // the first two lands on the third, since a short last line is still a line below.
        ("top-a.txt", "-", "top-c.txt"),
        ("top-b.txt", "-", "top-c.txt"),
        ("top-c.txt", "top-a.txt", "a"),
        // A folder's row: down is what is inside it, up is what is drawn above it.
        ("a", "top-c.txt", r"a\f1.txt"),
        // And its files, the same shape one level in.
        (r"a\f1.txt", "a", r"a\f3.txt"),
        (r"a\f2.txt", "a", r"a\f3.txt"),
        (r"a\f3.txt", r"a\f1.txt", r"a\b"),
        // Down off the last file of a branch is the next folder up the tree, not the next row.
        (r"a\b", r"a\f3.txt", r"a\b\deep.txt"),
        (r"a\b\deep.txt", r"a\b", "z"),
        ("z", r"a\b\deep.txt", r"z\last.txt"),
        (r"z\last.txt", "z", "zz"),
        // The end of the pane, which is where both keys have to stand still.
        ("zz", r"z\last.txt", "-"),
    ] {
        assert_eq!(step(from, Step::Up), up, "up from `{from}`");
        assert_eq!(step(from, Step::Down), down, "down from `{from}`");
    }

    // ---- And a page, which is the same step over and over ---------------
    //
    // Past the end rather than to it, so what is pinned is that it stops at the last place instead
    // of stopping moving.
    let first = layout.first().expect("a pane with something on it");
    assert_eq!(named(layout.walk(first, Step::Down, 100)), "zz");
    let last = layout.last().expect("a pane with something on it");
    assert_eq!(named(layout.walk(last, Step::Up, 100)), "top-a.txt");
    // A page of two, from the top: `top-c.txt` and then `a`.
    assert_eq!(named(layout.walk(first, Step::Down, 2)), "a");
}

/// **The page keys are the folders, and `Ctrl` is the pair that never goes deeper.**
///
/// A screenful is not a useful page of a tree — the rows a screen holds belong to several different
/// branches — so `PageDown` is the next folder's row and `PageUp` the one before it, wherever in the
/// hierarchy that is. `Ctrl` with either takes only folders no deeper than what the cursor is in, so
/// it crosses the top of the tree instead of descending into every branch on the way past.
///
/// The distinction is the whole test, and it needs a level to skip: from `a`, the next folder at any
/// depth is `a\b` *inside* it, and the next one no deeper is `z`. Both are asserted from a file as
/// well as from a row, because a file's level is the folder it is in rather than the depth its own
/// tile is drawn at — `f2.txt` is one level in and belongs to a folder at the top.
#[test]
fn the_page_keys_walk_the_folders_and_ctrl_never_goes_deeper() {
    let (tab, dir) = a_tree();
    let mut layout = Layout::default();
    layout.ensure(&tab, space::S3 * 2.0 + filelist::INDENT + CELL_W * 2.0 + 1.0);

    let name = |position: usize| -> String {
        tab.entry_at(position)
            .map(|entry| dir.name(entry).to_owned())
            .unwrap_or_else(|| format!("nothing at {position}"))
    };
    let page = |from: &str, down: bool, shallower: bool| -> String {
        let at = (0..tab.order.len())
            .find(|&at| name(at) == from)
            .unwrap_or_else(|| panic!("no `{from}` in the listing"));
        layout
            .next_folder(at, down, shallower)
            .map_or_else(|| "-".to_owned(), &name)
    };

    // (from, PageDown, PageUp, Ctrl+PageDown, Ctrl+PageUp)
    for (from, down, up, across_down, across_up) in [
        // From the listed folder's own files, whose level is the folder being listed: every top-level
        // folder is fair game and there is nothing above them to go back to.
        ("top-a.txt", "a", "-", "a", "-"),
        ("top-c.txt", "a", "-", "a", "-"),
        // **From `a`, the two keys differ**: `a\b` is inside it, `z` is the next one at its level.
        ("a", r"a\b", "-", "z", "-"),
        // From a file in `a`: the same answers, because its level is `a`'s and not its own indent —
        // and back up is `a` itself, which is the folder the file is in.
        (r"a\f2.txt", r"a\b", "a", "z", "a"),
        (r"a\f3.txt", r"a\b", "a", "z", "a"),
        // One level in. Up is `a` either way — it is shallower, so `Ctrl` does not exclude it.
        (r"a\b", "z", "a", "z", "a"),
        (r"a\b\deep.txt", "z", r"a\b", "z", r"a\b"),
        // The second branch, and the empty folder after it. Back across from `z` is `a` and not
        // `a\b`, which is the one place `Ctrl+PageUp` has a whole branch to step over.
        ("z", "zz", r"a\b", "zz", "a"),
        (r"z\last.txt", "zz", "z", "zz", "z"),
        // The end of the tree: nothing below, and back up is the branch it came from.
        ("zz", "-", "z", "-", "z"),
    ] {
        assert_eq!(page(from, true, false), down, "PageDown from `{from}`");
        assert_eq!(page(from, false, false), up, "PageUp from `{from}`");
        assert_eq!(page(from, true, true), across_down, "Ctrl+PageDown from `{from}`");
        assert_eq!(page(from, false, true), across_up, "Ctrl+PageUp from `{from}`");
    }
}

/// The visible range is the blocks the viewport crosses, and never fewer.
///
/// A binary search over the blocks' tops, which is the one piece of arithmetic here that fails
/// *silently*: an off-by-one at the top edge leaves a strip of the pane blank, and one at the
/// bottom leaves a tile half drawn as you scroll onto it.
#[test]
fn the_visible_range_covers_every_block_the_viewport_crosses() {
    let mut builder = DirBuilder::new(r"C:\x");
    for i in 0..6 {
        builder.push(&format!("d{i}"), 0, 0, FLAG_DIR);
        builder.push(&format!("d{i}\\f.txt"), 1, 0, 0);
    }
    let mut tab = Tab::new(r"C:\x");
    tab.flat = true;
    tab.flat_mode = FlatMode::Tree;
    tab.regroup = false;
    tab.apply(Arc::new(builder.finish(0)));

    let mut layout = Layout::default();
    layout.ensure(&tab, 800.0);
    assert_eq!(layout.blocks.len(), 12, "six rows and six grids");

    // The whole thing.
    assert_eq!(layout.blocks_in(0.0, layout.height()), 0..12);
    // A window over the middle: every block whose span overlaps it, and the arithmetic is
    // checked against the tops rather than restated.
    let (from, to) = (layout.tops[5] + 1.0, layout.tops[8] - 1.0);
    let range = layout.blocks_in(from, to);
    assert!(range.start <= 5 && range.end >= 8, "{range:?} misses a block");
    for at in range.clone() {
        let (top, bottom) = (layout.tops[at], layout.tops[at + 1]);
        assert!(bottom > from && top < to, "block {at} is not visible at all");
    }
    // A viewport at the very top and one past the very bottom both answer something drawable
    // rather than an empty or inverted range.
    assert!(!layout.blocks_in(0.0, 1.0).is_empty());
    let range = layout.blocks_in(layout.height() + 500.0, layout.height() + 900.0);
    assert!(range.start <= range.end, "{range:?} is inverted");
}

/// A picture keeps its aspect inside the tile, and a small one is not blown up.
#[test]
fn a_picture_is_fitted_and_never_upscaled() {
    let box_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(TILE, TILE));

    // Wider than tall: the width fills the box and the height follows the aspect.
    let wide = fit(box_rect, [96, 54]);
    assert_eq!(wide.width(), TILE);
    assert!((wide.height() - 54.0).abs() <= 1.0, "{wide:?}");
    assert_eq!(wide.center(), box_rect.center(), "and it stays centred");

    // Square fills it.
    assert_eq!(fit(box_rect, [96, 96]).size(), vec2(TILE, TILE));

    // Smaller than the box: its own size, in the middle. A 32-pixel icon blown up to 96 is a
    // blur, and a blur is a worse answer than a small icon.
    let small = fit(box_rect, [32, 32]);
    assert_eq!(small.size(), vec2(32.0, 32.0));
    assert_eq!(small.center(), box_rect.center());

    // Nothing at all: the box, rather than a rect of zero size or a division by zero.
    assert_eq!(fit(box_rect, [0, 0]), box_rect);
}

/// A band over a grid covers the tiles it overlaps, and the gaps between them cover nothing.
///
/// The second half is the point: a rubber band has to be startable *between* two tiles, which
/// means the gap belongs to the folder and not to either neighbour. A coverage test written
/// against the whole cell rather than the tile's own box would select whichever tile the band
/// began in, and dragging one would look like a drag of a file.
#[test]
fn a_band_covers_the_tiles_it_crosses_and_not_the_gaps() {
    let tab = {
        let mut tab = flat(12);
        tab.band = Some(crate::pane::Band {
            anchor: pos2(0.0, 0.0),
            current: pos2(0.0, 0.0),
            base: Vec::new(),
        });
        tab
    };
    let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(space::S3 * 2.0 + CELL_W * 4.0, 600.0));
    let mut layout = Layout::default();
    layout.ensure(&tab, body.width());
    assert_eq!(layout.columns, 4);

    let step = step_of(body.width(), 0, 4);
    let cell_of = |column: usize, line: usize| {
        Rect::from_min_size(
            pos2(
                body.left() + space::S3 + column as f32 * step,
                line as f32 * CELL_H,
            ),
            vec2(step, CELL_H),
        )
    };
    let band_over = |tab: &mut Tab, from: egui::Pos2, to: egui::Pos2| {
        if let Some(band) = &mut tab.band {
            band.anchor = from;
            band.current = to;
        }
        covered_by_band(&layout, tab, body)
    };

    let mut tab = tab;
    // A band across the first line's first two tiles.
    let a = item_of(cell_of(0, 0), step);
    let b = item_of(cell_of(1, 0), step);
    let got = band_over(&mut tab, a.center(), b.center());
    assert_eq!(got, vec![0, 1], "a band over the first two tiles");

    // A band held inside the gap between two tiles selects nothing at all.
    let gap = (a.right() + b.left()) * 0.5;
    let got = band_over(
        &mut tab,
        pos2(gap - 0.5, a.center().y),
        pos2(gap + 0.5, a.center().y),
    );
    assert!(got.is_empty(), "a band in the gap selected {got:?}");

    // And one dragged down the left column takes the left tile of each line, and only those.
    let last = item_of(cell_of(0, 2), step);
    let got = band_over(&mut tab, a.center(), last.center());
    assert_eq!(got, vec![0, 4, 8], "down the left column");
}
