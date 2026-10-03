//! The large-icon view: a grid of tiles, each with the shell's own thumbnail on it.
//!
//! Explorer's *Large icons*, and the same three things it is for: seeing what a picture actually is
//! without opening it, seeing a folder of them at a glance, and pointing at one with a mouse rather
//! than reading a column of names. [`crate::ui::filelist`] is the other half of the choice — see
//! [`crate::pane::ViewMode`], which is where it lives.
//!
//! # What it shares with the details view, and what it does not
//!
//! **Everything about the listing.** The order, the filter, the sort, the selection, the cursor and
//! the rubber band are all [`Tab`]'s and none of them knows which view is drawing. A tile is
//! identified by its *position in the display order*, exactly as a row is, so every gesture in the
//! program — a paste, a delete, a drag out, a shell menu — is the same code reached from a different
//! rectangle.
//!
//! **Nothing about the geometry.** A row is a band across the pane and a tile is a rectangle in two
//! axes, which changes three things that look small and are not:
//!
//! - *Hit-testing* is `(column, row)` arithmetic rather than a division by the row height.
//! - *The band* covers a rectangle of cells rather than a range of rows, which is why
//!   [`Tab::apply_band_over`] takes the coverage this view works out.
//! - *The height is not the row count times anything*, once a tree is involved. Which is the whole
//!   of the next section.
//!
//! # The tree, and why there is a [`Layout`] at all
//!
//! Flattened as a **list**, this is one grid over the whole order and the arithmetic is a division:
//! nothing is cached, nothing is walked, and a folder of 300,000 files costs the same as a folder of
//! forty. That is the common case and it stays free.
//!
//! Flattened as a **tree**, the shape the user asked for is: *the folders are rows, as in the details
//! view, and the files in each folder are a grid of their own.* So the pane is a run of blocks —
//! a folder row, then a grid, then the folder rows under it — of which the grids have a height that
//! depends on how many columns fit at that indent. There is no closed form for "how tall is this"
//! and no closed form for "which block is at y", so both are precomputed into [`Layout`] and looked
//! up: the total once, and the visible range by binary search over the blocks' tops.
//!
//! It is rebuilt when the order changes or the pane is resized, and not otherwise — see
//! [`Layout::ensure`] and [`crate::pane::Tab::order_gen`].
//!
//! **A folder's files come immediately after its own row and before the folders under it.** Which is
//! the only arrangement that works: the alternative puts a folder's own contents below its entire
//! subtree, so looking at what is in the folder you just opened means scrolling past everything
//! inside everything inside it. The files of the folder being *listed* are therefore the first thing
//! on the pane, which is the same rule with nothing above it.

use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space, typography};
use egui::{pos2, vec2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};

use crate::app::Action;
use crate::fs::fmt;
use crate::icons;
use crate::pane::{PaneId, Tab, ROW_HEIGHT};
use crate::shell::icons::Icons;
use crate::shell::thumbs::Thumbs;
use crate::theme::Theme;
use crate::ui::filelist::{self, Outcome};
use crate::ui::truncated;

/// The picture box on a tile, in points.
///
/// Ninety-six, which is what Explorer calls *Large icons* and what [`crate::shell::thumbs::CELL`]
/// asks the shell for. Big enough to tell two photographs apart, which is the whole reason to be in
/// this view, and small enough that a screenful is still a hundred-odd files rather than a dozen.
pub const TILE: f32 = 96.0;

/// A tile's slot, before the leftover width is spread between them.
///
/// The picture plus air either side. It is the *minimum* a cell gets: [`step_of`] divides whatever
/// the pane is actually wide between however many of these fit, so the row of tiles is spread across
/// the pane rather than packed left with a ragged gutter at the end — which is what Explorer does and
/// is the difference between a grid and a left-aligned pile.
pub const CELL_W: f32 = TILE + space::S4 * 2.0;

/// How many lines of the name a tile shows.
///
/// Two. One is not enough for the names people actually have — `2026-04-report-final-v3.xlsx` is
/// gone by the middle — and three makes the grid taller than it is wide for no gain, because past two
/// lines the eye is reading rather than glancing and this is a view for glancing. Explorer shows two
/// as well, and cuts with an ellipsis exactly here.
const LABEL_LINES: usize = 2;
const LABEL: f32 = typography::LINE_BODY * LABEL_LINES as f32;

/// A cell's height: air, the picture, air, two lines of name, air.
pub const CELL_H: f32 = space::S3 + TILE + space::S2 + LABEL + space::S3;

/// What a tile's own box gives up at each side, so two neighbours never touch.
const ITEM_INSET: f32 = space::S2;

/// How big a painted glyph is drawn inside the picture box, while the shell's answer is on its way
/// or where there was not one.
///
/// Not the whole box. A painted glyph is a *symbol* and a thumbnail is a *picture*: filling the tile
/// with a 96-point folder would make the placeholder louder than the thing it stands in for, and the
/// grid would flicker from bold to quiet as the answers landed. Two thirds is the size the same
/// glyphs are drawn at in every other piece of chrome in this window, scaled up.
const GLYPH_SHARE: f32 = 0.62;

/// How big a git badge is on a tile, and where it sits.
///
/// Bigger than the listing's [ten points](crate::ui::filelist) because the thing it is pinned to is
/// six times the size — the same *proportion* of the picture box, in the same bottom-left corner the
/// shell puts its own overlays in.
const BADGE: f32 = 18.0;

/// Nothing under the last tile — a cell and a half of it.
///
/// [`crate::ui::filelist::TAIL`]'s reason, in this view's units: the folder has to be somewhere to
/// right-click, and in a folder taller than the pane every pixel from the top to the status line is
/// otherwise a file. It is also what makes a band easy to start from below the tiles.
const TAIL: f32 = CELL_H * 0.75;

// ---------------------------------------------------------------------------
// Where everything goes
// ---------------------------------------------------------------------------

/// One thing the pane is made of.
#[derive(Clone, Copy, Debug)]
enum Block {
    /// A folder's row in a tree, at this position in the display order.
    Row { position: u32, depth: u32 },
    /// The files inside one folder, as a grid indented to `depth`.
    ///
    /// `from` and `count` index [`Layout::cells`] — or the display order itself, in a listing that is
    /// not a tree and therefore keeps no `cells`.
    Grid {
        from: u32,
        count: u32,
        depth: u32,
        columns: u32,
    },
}

/// Where every tile and every folder row goes, worked out once per order and per width.
///
/// Lives on the [`Tab`], because it describes that tab's order. See the module header for why it is
/// precomputed rather than derived per frame, and [`Layout::ensure`] for what invalidates it.
#[derive(Default)]
pub struct Layout {
    blocks: Vec<Block>,
    /// The top of each block, **plus one more entry**: the height of the whole thing. So
    /// `tops.len() == blocks.len() + 1` always, which is what lets a binary search for the visible
    /// range treat the end like any other boundary.
    tops: Vec<f32>,
    /// The display positions the grid blocks hold, in the order the tiles are drawn.
    ///
    /// **Empty for every listing that is not a tree**, where cell *n* of the one grid block simply *is*
    /// display position *n* — see [`Layout::cell_at`], which is the only reader. That is not a
    /// micro-optimisation: filled in, this would be a second copy of the whole display order, 1.2 MB
    /// for a flattened `C:\Program Files`, to say nothing at all.
    cells: Vec<u32>,

    /// What it was built from. Any of these differing is what makes it stale.
    ///
    /// The order is compared by *generation* and not by length: a click on a column header leaves the
    /// length alone and moves every row. See [`crate::pane::Tab::order_gen`].
    gen: u64,
    width: f32,
    tree: bool,
    built: bool,

    /// How many tiles fit across, where that is one number — which is every listing that is not a
    /// tree. `1` in a tree, where each grid has its own count and the keyboard follows the rows.
    ///
    /// Read by `App`'s keyboard: it is what `Up` and `Down` move by.
    pub columns: usize,
}

impl Layout {
    /// Rebuild if anything it was built from has moved: the order, the width the columns were counted
    /// at, whether it is a tree, and whether it has ever been built at all.
    pub fn ensure(&mut self, tab: &Tab, width: f32) {
        let tree = tab.is_tree();
        let fresh = self.built
            && self.gen == tab.order_gen
            && self.tree == tree
            && (self.width - width).abs() < 0.5;
        if !fresh {
            self.build(tab, width, tree);
        }
    }

    /// How tall the whole thing is, before the tail.
    pub fn height(&self) -> f32 {
        self.tops.last().copied().unwrap_or(0.0)
    }

    fn build(&mut self, tab: &Tab, width: f32, tree: bool) {
        self.blocks.clear();
        self.tops.clear();
        self.cells.clear();
        self.width = width;
        self.gen = tab.order_gen;
        self.tree = tree;
        self.built = true;
        self.columns = 1;

        if !tree {
            let columns = columns_in(width, 0);
            self.columns = columns;
            let mut total = 0.0;
            if !tab.order.is_empty() {
                self.blocks.push(Block::Grid {
                    from: 0,
                    count: tab.order.len() as u32,
                    depth: 0,
                    columns: columns as u32,
                });
                self.tops.push(0.0);
                total = grid_height(tab.order.len(), columns);
            }
            self.tops.push(total);
            return;
        }
        self.build_tree(tab, width);
    }

    /// The tree: a row per folder, and a grid of files under each of them.
    fn build_tree(&mut self, tab: &Tab, width: f32) {
        use std::collections::HashMap;

        /// The folder being listed, which has a grid but no row of its own.
        const ROOT: u32 = u32::MAX;

        // Which folder each file belongs to, as that folder's *position* in the display order.
        //
        // The order is pre-order, so a file drawn at depth `d` sits in whichever folder was last seen
        // at `d - 1` — one pass, one small stack, and no lookup by name. `by_depth` is that stack:
        // the position of the folder currently open at each level.
        //
        // The depths are the ones the rows are **drawn** at (`Tab::row_depth`) and not the ones their
        // paths have, which is what makes a merged chain — `src > main > java` as one row — come out
        // right: its children are one level in from where the chain stands, and its path is three
        // deeper than that.
        let mut files: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut by_depth: Vec<u32> = Vec::new();
        for position in 0..tab.order.len() {
            let depth = tab.row_depth(position);
            if tab.is_dir_at(position) {
                by_depth.truncate(depth);
                // Only reachable from an order that is not pre-order, which nothing here produces —
                // but a `Vec` indexed by user-supplied depth is not somewhere to find out.
                while by_depth.len() < depth {
                    by_depth.push(ROOT);
                }
                by_depth.push(position as u32);
            } else {
                let parent = match depth.checked_sub(1) {
                    Some(above) => by_depth.get(above).copied().unwrap_or(ROOT),
                    None => ROOT,
                };
                files.entry(parent).or_default().push(position as u32);
            }
        }

        let mut y = 0.0;
        // The listed folder's own files, at the top — see the module header.
        if let Some(group) = files.remove(&ROOT) {
            y = self.push_grid(group, 0, width, y);
        }
        for position in 0..tab.order.len() {
            if !tab.is_dir_at(position) {
                continue;
            }
            let depth = tab.row_depth(position);
            self.blocks.push(Block::Row {
                position: position as u32,
                depth: depth as u32,
            });
            self.tops.push(y);
            y += ROW_HEIGHT;
            if let Some(group) = files.remove(&(position as u32)) {
                y = self.push_grid(group, depth + 1, width, y);
            }
        }
        self.tops.push(y);
    }

    /// Add one folder's files as a grid at `top`, and answer where the next block starts.
    fn push_grid(&mut self, group: Vec<u32>, depth: usize, width: f32, top: f32) -> f32 {
        if group.is_empty() {
            return top;
        }
        let columns = columns_in(width, depth);
        let from = self.cells.len() as u32;
        self.cells.extend_from_slice(&group);
        self.blocks.push(Block::Grid {
            from,
            count: group.len() as u32,
            depth: depth as u32,
            columns: columns as u32,
        });
        self.tops.push(top);
        top + grid_height(group.len(), columns)
    }

    /// The display position a grid block's `index`-th cell holds.
    fn cell_at(&self, from: u32, index: usize) -> Option<usize> {
        let at = from as usize + index;
        if !self.tree {
            return Some(at);
        }
        self.cells.get(at).map(|&position| position as usize)
    }

    /// **Every tile of one grid block that a band of `y` touches**, as its display position and the
    /// box that is drawn and clicked.
    ///
    /// The one statement of where a tile is, and the reason it is a method rather than two loops is
    /// that there *were* two loops. The drawing pass and the rubber band both need this — one to paint
    /// a cell, the other to decide whether the band caught it — and the two have to agree exactly or a
    /// band selects a different tile from the one the pointer is over. They had already drifted: the
    /// drawing pass took `ceil` for its last line where the band pass took `floor`, which is a row of
    /// tiles the band could reach and the pointer could not.
    ///
    /// `y_origin` is what the block's top is measured from — the screen for the drawing pass, zero for
    /// the band, whose corners are in content space. Only the lines the window crosses are walked, so a
    /// block holding a folder of 300,000 files costs the tiles on screen and nothing for the rest.
    fn tiles(
        &self,
        block: Block,
        body: Rect,
        y_origin: f32,
        from_y: f32,
        to_y: f32,
    ) -> impl Iterator<Item = (usize, Rect)> + '_ {
        let (from, count, depth, columns) = match block {
            Block::Grid {
                from,
                count,
                depth,
                columns,
            } => (from, count as usize, depth as usize, (columns as usize).max(1)),
            Block::Row { .. } => (0, 0, 0, 1),
        };
        let step = step_of(body.width(), depth, columns);
        let left = body.left() + indent_of(depth) + space::S3;
        let lines = count.div_ceil(columns);
        let line_of = |y: f32| ((y / CELL_H).floor().max(0.0) as usize).min(lines);
        // Inclusive of the line the window's bottom edge lands in, which is what the `+ 1` is: a tile
        // half off the edge is still a tile on screen, and still one a band has reached into.
        let first = line_of(from_y);
        let last = (line_of(to_y) + 1).min(lines);

        (first..last).flat_map(move |line| {
            (0..columns).filter_map(move |column| {
                let index = line * columns + column;
                if index >= count {
                    return None;
                }
                let position = self.cell_at(from, index)?;
                let cell = Rect::from_min_size(
                    pos2(
                        left + column as f32 * step,
                        y_origin + line as f32 * CELL_H,
                    ),
                    vec2(step, CELL_H),
                );
                Some((position, item_of(cell, step)))
            })
        })
    }

    /// Which blocks a content-space band of `y` touches.
    fn blocks_in(&self, from: f32, to: f32) -> std::ops::Range<usize> {
        if self.blocks.is_empty() {
            return 0..0;
        }
        let last_block = self.blocks.len() - 1;
        let first = self
            .tops
            .partition_point(|&top| top <= from)
            .saturating_sub(1)
            .min(last_block);
        let end = self
            .tops
            .partition_point(|&top| top < to)
            .clamp(first + 1, self.blocks.len());
        first..end
    }

    /// Where the line holding `position` sits: its top in content space, and how tall it is.
    ///
    /// For scrolling the cursor into view, which is the one question that goes the other way — every
    /// other lookup here starts from a `y`. Walked rather than indexed: a tree's blocks are a couple
    /// per folder, so this is thousands of comparisons on a keystroke where an index would be a
    /// parallel array over every row for the life of the tab.
    pub fn locate(&self, position: usize) -> Option<(f32, f32)> {
        for (at, block) in self.blocks.iter().enumerate() {
            match *block {
                Block::Row { position: row, .. } if row as usize == position => {
                    return Some((self.tops[at], ROW_HEIGHT));
                }
                Block::Row { .. } => {}
                Block::Grid {
                    from,
                    count,
                    columns,
                    ..
                } => {
                    let columns = (columns as usize).max(1);
                    // Without a `cells` vector it is arithmetic; a tree's cells have to be searched,
                    // and the group is one folder's files.
                    let index = if !self.tree {
                        let start = from as usize;
                        (position >= start && position < start + count as usize)
                            .then(|| position - start)
                    } else {
                        let group = &self.cells[from as usize..(from + count) as usize];
                        group.iter().position(|&cell| cell as usize == position)
                    };
                    if let Some(index) = index {
                        let row = index / columns;
                        return Some((self.tops[at] + row as f32 * CELL_H, CELL_H));
                    }
                }
            }
        }
        None
    }
}

/// How many tiles fit across a pane of `width`, at a tree indent of `depth`.
fn columns_in(width: f32, depth: usize) -> usize {
    let usable = width - indent_of(depth) - space::S3 * 2.0;
    ((usable / CELL_W).floor() as usize).max(1)
}

/// How far in a tree block at `depth` starts. The details view's own step, so a folder's tiles line
/// up under the folder's name rather than under some second idea of an indent.
fn indent_of(depth: usize) -> f32 {
    depth as f32 * filelist::INDENT
}

/// How tall a grid of `count` cells in `columns` columns is.
fn grid_height(count: usize, columns: usize) -> f32 {
    let columns = columns.max(1);
    (count.div_ceil(columns)) as f32 * CELL_H
}

/// How wide one cell's slot is, once the leftover width has been spread between them.
fn step_of(width: f32, depth: usize, columns: usize) -> f32 {
    let usable = (width - indent_of(depth) - space::S3 * 2.0).max(CELL_W);
    usable / columns.max(1) as f32
}

/// One tile, or one folder row, placed on screen.
struct Placed {
    /// Its position in the display order.
    position: usize,
    /// A folder's row in a tree, rather than a tile.
    row: bool,
    depth: usize,
    /// What is drawn and what a click lands on: the tile's own box, or the whole width of a row.
    hit: Rect,
}

/// The picture box and the label box inside a tile's own box.
///
/// One statement of the pair, so the frame drawn round a picture and the picture itself cannot end up
/// measured from different edges. Where the *tile* is comes from [`Layout::tiles`]; this is only what
/// is inside one.
fn parts(item: Rect) -> (Rect, Rect) {
    let picture = Rect::from_center_size(
        pos2(
            item.center().x.round(),
            (item.top() + space::S3 + TILE * 0.5).round(),
        ),
        vec2(TILE, TILE),
    );
    let label = Rect::from_min_max(
        pos2(item.left() + space::S2, picture.bottom() + space::S2),
        pos2(item.right() - space::S2, item.bottom() - space::S2),
    );
    (picture, label)
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Draw the tiles inside `body`, and wire up everything a click there can mean.
///
/// The mirror of [`crate::ui::filelist`]'s `rows`, and built the same way for the same reasons: one
/// scroll area told the total height, only the visible blocks drawn, **one** interaction over the
/// whole viewport with the cell derived from the pointer, and every textured quad held back to the
/// end of the frame so the primitive stream is not cut in two at every tile.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    zone: &crate::fs::time::LocalZone,
    body: Rect,
    pane: PaneId,
    tab: &mut Tab,
    focused: bool,
    icons_cache: &mut Icons,
    thumbs: &mut Thumbs,
    cut: &[std::path::PathBuf],
    out: &mut Vec<Action>,
    outcome: &mut Outcome,
) {
    let Some(dir) = tab.dir.clone() else { return };

    // Taken out so the layout can be read while the tab is borrowed mutably, and put back at the
    // end. It is the tab's own cache; this is only where it is used.
    let mut layout = std::mem::take(&mut tab.grid);
    layout.ensure(tab, body.width());
    let content = layout.height();

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    // Cells are painted at exact rects, so anything egui adds per item would put the arithmetic and
    // the painting out of step the moment something scrolled. The listing does the same.
    child.spacing_mut().item_spacing = egui::Vec2::ZERO;

    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("tiles", pane))
        .auto_shrink([false, false]);

    // Keyboard movement has to bring the cursor with it — nudged into view rather than centred, as
    // in the details view. What is different here is only how a position becomes a `y`: a tile is on
    // a line whose top the layout knows, where a row's is a multiplication.
    if let Some(offset) = tab.scroll_to.take() {
        scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        tab.scroll_to_cursor = false;
    } else if tab.scroll_to_cursor {
        if let Some((top, height)) = tab.cursor.and_then(|at| layout.locate(at)) {
            let view = body.height();
            let mut offset = tab.scroll_y;
            if top < offset {
                offset = top;
            } else if top + height > offset + view {
                offset = top + height - view;
            }
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        tab.scroll_to_cursor = false;
    }

    let output = scroll.show_viewport(&mut child, |ui, viewport| {
        ui.set_height(content + TAIL);
        let origin = ui.max_rect().top();
        let visible = layout.blocks_in(viewport.min.y, viewport.max.y);

        // ---- Where everything on screen is, before anything is drawn ------
        //
        // Two passes, and the split is not tidiness: the fill under a tile depends on whether the
        // pointer is over it, and "which tile is the pointer over" is a question about geometry that
        // the drawing would otherwise have to answer half way through itself.
        let mut placed: Vec<Placed> = Vec::with_capacity(64);
        for at in visible.clone() {
            let top = origin + layout.tops[at];
            match layout.blocks[at] {
                Block::Row { position, depth } => placed.push(Placed {
                    position: position as usize,
                    row: true,
                    depth: depth as usize,
                    hit: Rect::from_min_size(
                        pos2(body.left(), top),
                        vec2(body.width(), ROW_HEIGHT),
                    ),
                }),
                // Only the lines the viewport crosses, and the arithmetic for that is
                // [`Layout::tiles`]' — the same call the rubber band makes.
                block @ Block::Grid { depth, .. } => {
                    let above = layout.tops[at];
                    placed.extend(
                        layout
                            .tiles(
                                block,
                                body,
                                top,
                                viewport.min.y - above,
                                viewport.max.y - above,
                            )
                            .map(|(position, hit)| Placed {
                                position,
                                row: false,
                                depth: depth as usize,
                                hit,
                            }),
                    );
                }
            }
        }

        // One interaction for everything on screen, and the cell worked out from the pointer. A
        // widget per tile would cost an id, a hit-test and an animation slot each, for a highlight
        // that geometry gives for nothing.
        let sheet = Rect::from_min_max(
            pos2(body.left(), origin + viewport.min.y),
            pos2(body.right(), origin + viewport.max.y),
        );
        let response = ui.interact(sheet, Id::new(("tiles-hit", pane)), Sense::click_and_drag());

        let at_of = |at: egui::Pos2| -> Option<usize> {
            placed
                .iter()
                .position(|cell| cell.hit.contains(at))
        };
        let hovered = response.hover_pos().and_then(at_of);
        // Where the button went down, while it is down — *not* the cell under the pointer. egui only
        // calls a press a drag once it has travelled, by which time the pointer is a cell or two
        // along, so deciding from the pointer would pick up the file the drag arrived at. The
        // details view has the same note.
        let press = ui.input(|i| {
            i.pointer
                .any_down()
                .then(|| i.pointer.press_origin())
                .flatten()
        });
        let pressed = press.and_then(at_of);

        // Held back and drawn in one run after the loop: every picture is a textured quad and
        // everything else on a tile comes out of the font atlas, so drawing one inside the loop
        // would break the frame's primitive stream at every cell. `crate::shell::icons::Icons::uv`
        // is where that reasoning lives, and it is the reason there is an atlas at all.
        let mut pictures: Vec<(egui::TextureId, Rect, Rect, Color32)> = Vec::new();
        let mut badges: Vec<(Rect, crate::git::State, Color32)> = Vec::new();
        // A frame round the pictures that really are pictures — see `frame_it` below.
        let mut frames: Vec<Rect> = Vec::new();
        // The cell being renamed, if one on screen is: where its field goes, and how far right it
        // may grow.
        let mut renaming: Option<(usize, Rect, f32)> = None;
        // Every visible twisty and the row it opens, for the click below.
        let mut twisties: Vec<(Rect, usize)> = Vec::new();
        let git = tab.git.clone();
        let mut chain_text = String::new();

        for (index, cell) in placed.iter().enumerate() {
            let position = cell.position;
            let Some(entry_index) = tab.entry_at(position) else {
                continue;
            };
            let entry = &dir.entries[entry_index];
            let selected = tab.selected.get(entry_index).copied().unwrap_or(false);
            let is_hovered = hovered == Some(index);
            let renaming_here = matches!(tab.renaming, Some((at, _)) if at == entry_index);

            // The same fill ladder as a row, on a different shape: rounded, because a tile is a
            // card-sized thing floating in a grid rather than a band welded to the pane's edges.
            // The row of a tree keeps the details view's square band, because that is what it is.
            let corners = if cell.row {
                CornerRadius::ZERO
            } else {
                CornerRadius::same(radius::SMALL)
            };
            let mut under = t.bg.layer;
            if !renaming_here {
                let fill = match (selected, focused) {
                    (true, false) => Some(crate::ui::row_fill_quiet(t)),
                    _ => crate::ui::row_fill(t, selected, is_hovered),
                };
                if let Some(fill) = fill {
                    ui.painter().rect_filled(cell.hit, corners, fill);
                    under = fill;
                }
                if selected && cell.row {
                    crate::ui::selection_bar(ui.painter(), cell.hit, t);
                }
            }
            if focused && tab.cursor == Some(position) && !selected {
                filelist::cursor_ring(ui.painter(), cell.hit.shrink(1.0), t.stroke.strong);
            }

            let pending_cut = !cut.is_empty()
                && cut
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == dir.name(entry_index)))
                && cut.iter().any(|p| p.parent() == Some(dir.path.as_path()));
            let dim = entry.is_hidden() || pending_cut;
            let name_color = if dim { t.text.tertiary } else { t.text.primary };
            let kind = fmt::kind_of(dir.ext(entry_index), entry.is_dir());

            if entry.is_dir() {
                outcome.drop_rows.push((cell.hit, dir.target(entry_index)));
            }

            // ---- A folder's row in a tree ---------------------------------
            if cell.row {
                let stem = filelist::stem_x(cell.hit, cell.depth);
                let shut = tab.collapsed.contains(dir.name(entry_index));
                if shut || tab.has_children_below(position) {
                    let box_rect = filelist::twisty_rect(cell.hit, cell.depth);
                    let chevron: azur_icons::Icon<'_> = if shut {
                        &azur_icons::chevron_right
                    } else {
                        &azur_icons::chevron_down
                    };
                    chevron(
                        ui.painter(),
                        crate::ui::icon_rect(cell.hit, box_rect.left(), filelist::TWISTY),
                        if is_hovered {
                            t.text.primary
                        } else {
                            t.text.secondary
                        },
                    );
                    twisties.push((box_rect, position));
                }
                let glyph_x = stem + filelist::TWISTY;
                let box_rect = crate::ui::icon_rect(cell.hit, glyph_x, filelist::GLYPH);
                match icons_cache
                    .kind(dir.ext(entry_index), true)
                    .and_then(|icon| icons_cache.uv(ui.ctx(), icon))
                {
                    Some((texture, uv)) => pictures.push((
                        texture,
                        uv,
                        box_rect,
                        if dim {
                            Color32::from_white_alpha(110)
                        } else {
                            Color32::WHITE
                        },
                    )),
                    None => icons::folder(
                        ui.painter(),
                        box_rect,
                        if dim { t.text.disabled } else { t.folder },
                    ),
                }
                if let Some(state) = git
                    .as_ref()
                    .and_then(|repo| repo.state(dir.name(entry_index)))
                {
                    badges.push((filelist::badge_rect(box_rect), state, under));
                }

                let name_left = glyph_x + filelist::GLYPH + space::S3;
                let name_right = cell.hit.right() - space::S3;
                let text_row = cell.hit.translate(vec2(0.0, -filelist::CELL_LIFT));
                if renaming_here {
                    renaming = Some((
                        entry_index,
                        Rect::from_min_max(
                            pos2(name_left, text_row.top()),
                            pos2(name_right, text_row.bottom()),
                        ),
                        cell.hit.right(),
                    ));
                } else if name_right > name_left {
                    // A merged chain reads exactly as it does in the details view: the folders in
                    // front dimmed, the row's own name in body ink.
                    let merged = tab.row_merged(position);
                    let galley = if merged > 0 {
                        filelist::chain_cell(
                            ui.painter(),
                            dir.name(entry_index),
                            merged,
                            t.fonts.body.clone(),
                            name_color,
                            t.text.secondary,
                            name_right - name_left,
                            &mut chain_text,
                        )
                    } else {
                        truncated(
                            ui.painter(),
                            dir.leaf(entry_index),
                            t.fonts.body.clone(),
                            name_color,
                            name_right - name_left,
                        )
                    };
                    crate::ui::text_left(
                        ui.painter(),
                        Rect::from_min_max(
                            pos2(name_left, text_row.top()),
                            pos2(name_right, text_row.bottom()),
                        ),
                        galley,
                    );
                }
                continue;
            }

            // ---- A tile ---------------------------------------------------
            let (picture, label) = parts(cell.hit);
            let target = dir.target(entry_index);
            // The shell's own answer for this file: a thumbnail where it has one, its large icon
            // where it has not. `None` on the first ask, which is what the painted glyph is for.
            match thumbs.get(&target, entry.modified, tab.view) {
                Some(thumb) if thumb.size[0] > 0 && thumb.size[1] > 0 => {
                    let fitted = fit(picture, thumb.size);
                    pictures.push((
                        thumb.texture,
                        thumb.uv,
                        fitted,
                        if dim {
                            Color32::from_white_alpha(110)
                        } else {
                            Color32::WHITE
                        },
                    ));
                    // A hairline round the picture, and **only** where the file really is one.
                    //
                    // A photograph with a white sky or a dark one bleeds into the pane without it,
                    // which is why Explorer frames its thumbnails; an *icon* is a shape on
                    // transparency, and a box drawn round one would be a box round nothing. The
                    // file's own extension is what tells them apart, since the shell does not say
                    // which of the two it handed back.
                    if crate::preview::kind_of(
                        dir.leaf(entry_index),
                        dir.ext(entry_index),
                        false,
                    ) == Some(crate::preview::Kind::Picture)
                    {
                        frames.push(fitted);
                    }
                }
                _ => {
                    let glyph: azur_icons::Icon<'_> = if entry.is_dir() && entry.is_link() {
                        &icons::folder_link
                    } else if entry.is_dir() {
                        &icons::folder
                    } else {
                        icons::for_kind(kind)
                    };
                    let color = if dim { t.text.disabled } else { t.kind(kind) };
                    glyph(
                        ui.painter(),
                        Rect::from_center_size(
                            picture.center(),
                            egui::Vec2::splat((TILE * GLYPH_SHARE).round()),
                        ),
                        if entry.is_dir() && !dim {
                            t.folder
                        } else {
                            color
                        },
                    );
                }
            }
            if let Some(state) = git
                .as_ref()
                .and_then(|repo| repo.state(dir.name(entry_index)))
            {
                badges.push((
                    Rect::from_min_size(
                        pos2(picture.left(), picture.bottom() - BADGE),
                        egui::Vec2::splat(BADGE),
                    ),
                    state,
                    under,
                ));
            }

            if renaming_here {
                // Over the *first* line of the name, which is where the name starts.
                renaming = Some((
                    entry_index,
                    Rect::from_min_max(
                        label.min,
                        pos2(label.right(), label.top() + typography::LINE_BODY),
                    ),
                    body.right() - space::S3,
                ));
            } else {
                let galley = wrapped(
                    ui.painter(),
                    dir.leaf(entry_index),
                    t.fonts.body.clone(),
                    name_color,
                    label.width(),
                );
                // Centred by the *layout job*, so each of the two lines is centred rather than a
                // left-aligned block being centred as a whole — which on a name that wraps to a
                // short second line is the difference between a caption and a ragged edge.
                ui.painter().galley(
                    crate::ui::snap(
                        ui.painter(),
                        pos2(label.center().x, label.top()),
                    ),
                    galley,
                    Color32::PLACEHOLDER,
                );
            }
        }

        // Every picture on screen, in one run — see where `pictures` is declared.
        //
        // **Sorted by texture first.** The thumbnails come off however many pages of
        // [`crate::shell::thumbs`]' atlas the window has needed, and a tree's folder rows come off the
        // icon atlas as well — so an unsorted batch breaks the primitive stream wherever two
        // neighbouring tiles happen to have landed on different pages. Grouped, it is one draw call per
        // page in use, which is the whole point of there being an atlas at all.
        //
        // Reordering is safe because nothing here overlaps anything else here: every quad sits in its
        // own tile or its own row, and the frames and badges are painted afterwards.
        if !pictures.is_empty() {
            pictures.sort_by_key(|(texture, ..)| *texture);
            let painter = ui.painter();
            for (texture, uv, rect, tint) in &pictures {
                painter.image(*texture, *rect, *uv, *tint);
            }
        }
        // Then the frames over them, then the badges over that: a badge belongs on top of the
        // picture it is about, and a frame belongs on the picture's own edge.
        for rect in &frames {
            ui.painter().rect_stroke(
                *rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, t.stroke.subtle),
                StrokeKind::Inside,
            );
        }
        if !badges.is_empty() {
            let painter = ui.painter();
            for (rect, state, under) in &badges {
                icons::git_badge(painter, *rect, *state, t.git(*state), *under);
            }
        }
        // And last of all, over everything: a field is allowed to be wider than the name it stands
        // in for, and a widget drawn mid-loop would have the next tile painted on top of it.
        if let Some((entry_index, at, limit)) = renaming {
            filelist::rename_over(ui, t, pane, tab, entry_index, at, limit, out);
        }

        // ---- What the tile is, in words -----------------------------------
        //
        // The listing's own tooltip, unchanged: the name in full, where it is, its type, its exact
        // size, when it changed, and what git says. A tile shows less than a row does — there are no
        // columns on it at all — so it is worth *more* here than it is there.
        let busy = tab.renaming.is_some()
            || tab.band.is_some()
            || ui.input(|i| i.pointer.any_down() || i.pointer.any_released());
        if let Some(cell) = hovered.filter(|_| !busy).and_then(|at| placed.get(at)) {
            let mut scratch = String::new();
            if let Some(about) = filelist::row_tooltip(tab, zone, cell.position, &mut scratch) {
                azur_egui_theme::components::tooltip_at_pointer_ui(response.clone(), |ui| {
                    crate::ui::tooltip_table(ui, t, &about);
                });
            }
        }

        // ---- Clicks ------------------------------------------------------
        let modifiers = ui.input(|i| i.modifiers);
        if tab.renaming.is_some() {
            // The field has the keyboard and the pointer; a click outside it is handled by the
            // field losing focus, not by moving the selection.
            return;
        }
        if response.clicked() || response.secondary_clicked() {
            out.push(Action::Focus(pane));
        }
        // A twisty first, and it returns: opening a folder is not selecting it. The details view
        // draws the same distinction and says why at length.
        if response.clicked() {
            let at = response
                .interact_pointer_pos()
                .or_else(|| ui.ctx().pointer_interact_pos());
            if let Some(&(_, position)) = at
                .and_then(|at| twisties.iter().find(|(hit, _)| hit.contains(at)))
            {
                out.push(Action::ToggleCollapsed { pane, position });
                return;
            }
        }

        // Which cell the context menu is *for*, as a display position. `None` means the folder's own
        // menu — the one with `New folder` and `Paste` in it.
        let mut menu_row = hovered.and_then(|at| placed.get(at)).map(|cell| cell.position);
        if let Some(cell) = hovered.and_then(|at| placed.get(at)) {
            let position = cell.position;
            if response.clicked() {
                if modifiers.command {
                    tab.toggle(position);
                } else if modifiers.shift {
                    tab.select_range_to(position);
                } else {
                    tab.select_only(position);
                }
                if tab.is_dir_at(position) {
                    outcome.prefetch = tab.target_at(position);
                }
            }
            if response.double_clicked() {
                if let Some(path) = tab.target_at(position) {
                    if tab.is_dir_at(position) {
                        out.push(Action::Navigate { pane, path });
                    } else {
                        out.push(Action::Open(path));
                    }
                }
            }
            if response.middle_clicked() {
                if let Some(path) = tab.target_at(position) {
                    if tab.is_dir_at(position) {
                        out.push(Action::NavigateNewTab { pane, path });
                    } else if tab.is_shortcut_at(position) {
                        out.push(Action::OpenNewTab(path));
                    }
                }
            }
            // A right click on a tile is about that file unless it is already in the selection, in
            // which case it is about the selection — the same rule the details view follows, and
            // simpler here because a tile has no columns to land between: its box *is* the file.
            if response.secondary_clicked() && !tab.is_selected(position) {
                tab.select_only(position);
            }
        } else if response.clicked() || response.secondary_clicked() {
            // The gaps between the tiles are the folder's background as much as the space below the
            // last one is, so a click there cancels the selection and a right click there asks for
            // the folder's own menu.
            tab.clear_selection();
            menu_row = None;
        }

        // ---- Dragging: the files, or a band -------------------------------
        //
        // Which one comes from where the button went *down*: on a tile, or in the space between
        // them. Keyed on the pressed cell and not the hovered one, for the reason `press` is
        // collected at all.
        let dragging = response.drag_started_by(egui::PointerButton::Primary)
            || response.drag_started_by(egui::PointerButton::Secondary);
        if dragging {
            match pressed.and_then(|at| placed.get(at)).map(|cell| cell.position) {
                Some(grabbed) => {
                    if !tab.is_selected(grabbed) {
                        tab.select_only(grabbed);
                    }
                    out.push(Action::DragOut {
                        pane,
                        items: tab.selection_paths(),
                    });
                }
                None => {
                    filelist::start_band(ui, body, tab, press);
                    out.push(Action::Focus(pane));
                }
            }
        }

        filelist::context_menu(ui, &response, pane, tab, menu_row, out);
    });

    tab.scroll_y = output.state.offset.y;

    // Everything below the last tile: the canvas a short folder leaves, and the [`TAIL`] a long one
    // leaves once it is scrolled to the end. Part of the listing — clicking it cancels the
    // selection, right-clicking it is how the folder's own menu is reached — and the sheet above
    // deliberately covers only the blocks, so nothing up there is listening for it.
    let bottom = body.top() + content - output.state.offset.y;
    let empty = Rect::from_min_max(
        pos2(body.left(), bottom.clamp(body.top(), body.bottom())),
        pos2(output.inner_rect.right(), body.bottom()),
    );
    if empty.height() > 1.0 && empty.width() > 1.0 {
        let response = child.interact(
            empty,
            Id::new(("tiles-empty", pane)),
            Sense::click_and_drag(),
        );
        if response.clicked() {
            tab.clear_selection();
            out.push(Action::Focus(pane));
        }
        if response.secondary_clicked() {
            out.push(Action::Focus(pane));
        }
        filelist::context_menu(&child, &response, pane, tab, None, out);
        if response.drag_started_by(egui::PointerButton::Primary)
            || response.drag_started_by(egui::PointerButton::Secondary)
        {
            let origin = child.input(|i| i.pointer.press_origin());
            filelist::start_band(&child, body, tab, origin);
            out.push(Action::Focus(pane));
        }
    }

    // There is more above, and there is more below. Measured off the content rather than off the
    // scroll area's own extent, because [`TAIL`] of that extent is deliberately empty.
    let above = output.state.offset.y;
    let below = (content - output.inner_rect.height() - above).max(0.0);
    azur_egui_theme::components::scroll_fades(
        &child.painter_at(output.inner_rect),
        output.inner_rect,
        t.bg.layer,
        above,
        below,
    );

    if tab.band.is_some() && filelist::band_move(&mut child, body, tab, content) {
        let covered = covered_by_band(&layout, tab, body);
        tab.apply_band_over(covered);
        filelist::band_paint(&child, t, body, tab);
    }

    tab.grid = layout;
}

/// A tile's own box inside its slot: the same width wherever the slot is, so a row of tiles reads as
/// a row of equal things rather than as a set of boxes that grow with the gaps.
fn item_of(cell: Rect, step: f32) -> Rect {
    let side = ((step - (CELL_W - ITEM_INSET * 2.0)) * 0.5).max(ITEM_INSET);
    cell.shrink2(vec2(side, space::S1))
}

/// A picture's rect inside the tile's box: as large as fits, in its own aspect, centred.
///
/// Never *upscaled* past its own pixels. A 32-pixel icon the shell had nothing better for, blown up
/// to ninety-six, is a blurred square; drawn at its own size in the middle of the box it reads as a
/// small icon, which is what it is.
fn fit(picture: Rect, size: [u32; 2]) -> Rect {
    let (w, h) = (size[0] as f32, size[1] as f32);
    if w <= 0.0 || h <= 0.0 {
        return picture;
    }
    let scale = (picture.width() / w).min(picture.height() / h).min(1.0);
    Rect::from_center_size(
        picture.center(),
        vec2((w * scale).round().max(1.0), (h * scale).round().max(1.0)),
    )
}

/// A name over at most [`LABEL_LINES`] lines, centred, with an ellipsis for whatever did not fit.
///
/// `break_anywhere`, because a file name is not a sentence: `translations_fr.json` has no space to
/// break at, and a wrapper that only breaks at spaces would put the whole of it on one line and
/// elide half. Explorer breaks mid-name for the same reason.
fn wrapped(
    painter: &egui::Painter,
    name: &str,
    font: egui::FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        name.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.halign = egui::Align::Center;
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(1.0),
        max_rows: LABEL_LINES,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

/// Which cells the rubber band covers.
///
/// The band's corners are in *content* coordinates — see [`crate::pane::Band`] — so this is the one
/// place the layout is asked a question in that space. Only the blocks the band's `y` crosses are
/// looked at, so a band dragged across a folder of 300,000 files tests the cells on screen and a
/// handful either side rather than all of them.
///
/// A folder's **row** in a tree is covered when the band crosses its `y`, whatever its `x` — which is
/// the details view's rule, and right for the same reason: a row spans the pane, so a band that
/// crosses it crosses the row.
fn covered_by_band(layout: &Layout, tab: &Tab, body: Rect) -> Vec<usize> {
    let Some(band) = &tab.band else {
        return Vec::new();
    };
    let (top, bottom) = (
        band.anchor.y.min(band.current.y),
        band.anchor.y.max(band.current.y),
    );
    let (left, right) = (
        band.anchor.x.min(band.current.x),
        band.anchor.x.max(band.current.x),
    );

    let mut covered = Vec::new();
    for at in layout.blocks_in(top, bottom) {
        let block_top = layout.tops[at];
        match layout.blocks[at] {
            Block::Row { position, .. } => {
                if top <= block_top + ROW_HEIGHT && bottom >= block_top {
                    covered.push(position as usize);
                }
            }
            // The tiles the band's `y` reaches, from [`Layout::tiles`] — the same call the drawing
            // pass makes, so the box a band tests is the box the pointer sees. Then the band's own
            // rectangle against each: a band through the *gap* between two tiles selects neither,
            // which is what makes it possible to draw one between them.
            block @ Block::Grid { .. } => {
                covered.extend(
                    layout
                        .tiles(block, body, block_top, top - block_top, bottom - block_top)
                        .filter(|(_, item)| {
                            right >= item.left()
                                && left <= item.right()
                                && bottom >= item.top()
                                && top <= item.bottom()
                        })
                        .map(|(position, _)| position),
                );
            }
        }
    }
    covered
}

#[cfg(test)]
mod tests {
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
}
