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
