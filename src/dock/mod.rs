//! The layout tree: how the panes divide the window.
//!
//! A binary tree of splits, which is the smallest structure that expresses
//! everything dragging a tab to an edge can ask for — side by side, stacked, and
//! any nesting of the two — while still having exactly one arrangement per state,
//! so there is nothing to normalise and nothing to get out of sync.
//!
//! Splits are addressed by the route taken to reach them (`[0, 1]` = the second
//! child of the first child) rather than by an id of their own. A route is derived
//! from the tree during layout and used in the same frame, so it cannot go stale —
//! whereas an id would need allocating, storing and reclaiming on every collapse.

use egui::{pos2, vec2, Pos2, Rect};

use crate::pane::{PaneId, Side};

/// What shows between two panes: one point of [`crate::ui::seam`], the same line the sidebar
/// is separated by.
///
/// It was `tokens::space::S2` — four points of canvas, which read as a gap between two cards.
/// A window divided into panes is not a row of cards, so what divides them is a line.
pub const GAP: f32 = crate::ui::SEAM;

/// How wide a splitter is to the pointer. Far wider than the line it sits on, because a
/// one-pixel grab target is not a grab target.
const GRAB: f32 = 4.0;

/// One node of the layout.
pub enum Node {
    Leaf(PaneId),
    Split {
        /// Children side by side, rather than stacked.
        horizontal: bool,
        /// Fraction of the space the first child gets, in [`RATIO_MIN`]`..=`[`RATIO_MAX`].
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// What a removal left behind for the caller to finish.
///
/// A run of same-way splits that has lost a pane has to be evened out at its *top*, and a node has no
/// way of knowing whether it is the top: that depends on its parent's axis. So the axis is carried
/// back up the recursion and the first node that does not divide it is the one that acts. See
/// [`Node::remove_at`].
enum Removed {
    /// A pane went and the run dividing this axis is still waiting to be evened out.
    Run(bool),
    /// A pane went and its run has been dealt with, or there was none left to deal with.
    Done,
}

/// A draggable divider, resolved for one frame.
pub struct Splitter {
    /// The grab area, which is wider than the visible gap.
    pub rect: Rect,
    pub horizontal: bool,
    /// Route from the root: `0` = first child, `1` = second.
    pub route: Vec<u8>,
}

/// What dropping a dragged tab on a pane would do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    /// Add the tab to this pane's own strip.
    Into,
    /// Split the pane, putting the tab on this side.
    Split(Side),
}

impl Zone {
    /// Every zone a pane has, which is what a drag draws its hints for. The centre first,
    /// because it is the one that means "leave the layout alone".
    pub const ALL: [Zone; 5] = [
        Zone::Into,
        Zone::Split(Side::Left),
        Zone::Split(Side::Right),
        Zone::Split(Side::Top),
        Zone::Split(Side::Bottom),
    ];
}

impl Node {
    /// Divide `rect` up, collecting where each pane goes and where each divider is.
    pub fn layout(&self, rect: Rect, panes: &mut Vec<(PaneId, Rect)>, splitters: &mut Vec<Splitter>) {
        panes.clear();
        splitters.clear();
        let mut route = Vec::new();
        self.layout_into(rect, &mut route, panes, splitters);
    }

    fn layout_into(
        &self,
        rect: Rect,
        route: &mut Vec<u8>,
        panes: &mut Vec<(PaneId, Rect)>,
        splitters: &mut Vec<Splitter>,
    ) {
        match self {
            Self::Leaf(id) => panes.push((*id, rect)),
            Self::Split {
                horizontal,
                ratio,
                first,
                second,
            } => {
                let (a, b, bar) = if *horizontal {
                    let usable = (rect.width() - GAP).max(0.0);
                    // Rounded so the divider lands on a whole pixel and its 1px
                    // edges do not go grey.
                    let width = (usable * ratio).round();
                    let a = Rect::from_min_max(rect.min, pos2(rect.left() + width, rect.bottom()));
                    let b = Rect::from_min_max(pos2(a.right() + GAP, rect.top()), rect.max);
                    let bar = Rect::from_min_max(
                        pos2(a.right() - GRAB, rect.top()),
                        pos2(b.left() + GRAB, rect.bottom()),
                    );
                    (a, b, bar)
                } else {
                    let usable = (rect.height() - GAP).max(0.0);
                    let height = (usable * ratio).round();
                    let a = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + height));
                    let b = Rect::from_min_max(pos2(rect.left(), a.bottom() + GAP), rect.max);
                    let bar = Rect::from_min_max(
                        pos2(rect.left(), a.bottom() - GRAB),
                        pos2(rect.right(), b.top() + GRAB),
                    );
                    (a, b, bar)
                };

                splitters.push(Splitter {
                    rect: bar,
                    horizontal: *horizontal,
                    route: route.clone(),
                });

                route.push(0);
                first.layout_into(a, route, panes, splitters);
                route.pop();
                route.push(1);
                second.layout_into(b, route, panes, splitters);
                route.pop();
            }
        }
    }

    /// The ratio of the split at `route`, for a divider drag.
    pub fn ratio_at(&mut self, route: &[u8]) -> Option<&mut f32> {
        let mut node = self;
        for &step in route {
            let Self::Split { first, second, .. } = node else {
                return None;
            };
            node = if step == 0 { first } else { second };
        }
        match node {
            Self::Split { ratio, .. } => Some(ratio),
            Self::Leaf(_) => None,
        }
    }

    /// Split the pane holding `target`, putting `added` on `side` of it.
    ///
    /// **And then even the run out**, which is the whole of what makes a third pane a third of the
    /// window rather than a quarter of it. A binary tree splits one node at a time, so splitting the
    /// right-hand half of a divided window gave `[1/2, 1/4, 1/4]` — three panes, two of them half
    /// the size of the first, because each split only ever halved the space it was handed. See
    /// [`Node::even_run`], which is where the arithmetic that fixes it is written down.
    pub fn split(&mut self, target: PaneId, side: Side, added: PaneId) -> bool {
        if !self.split_at(target, side, added) {
            return false;
        }
        self.even_run(added, side.is_horizontal());
        true
    }

    /// The split itself: the tree walk, without the evening out.
    fn split_at(&mut self, target: PaneId, side: Side, added: PaneId) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                let existing = Self::Leaf(*id);
                let new = Self::Leaf(added);
                let (first, second) = if side.is_first() {
                    (new, existing)
                } else {
                    (existing, new)
                };
                *self = Self::Split {
                    horizontal: side.is_horizontal(),
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.split_at(target, side, added) || second.split_at(target, side, added)
            }
        }
    }

    /// Give every pane in one run of same-way splits the same share of it.
    ///
    /// **A run is what reads as a row or a column**: the maximal chain of splits that divide the
    /// same axis, reachable from each other without passing through a split of the other axis. Three
    /// panes side by side are `h(0, h(1, 2))` — two nodes, one run — and what somebody who has just
    /// dragged a third tab to an edge means by it is three columns of equal width. What the tree
    /// gives without this is `[1/2, 1/4, 1/4]`, because `Node::split_at` halves the *node* it lands
    /// on and knows nothing about the one above it.
    ///
    /// The share is `slots(first) / slots(node)`, applied at every node of the run — which is what
    /// makes it right for a run of either shape rather than only for the right-leaning one a
    /// sequence of splits happens to build: `h(h(0, 1), 2)` comes out `2/3` at the root and `1/2`
    /// inside it, the same three columns.
    ///
    /// **Only the run the new pane joined**, found by walking down to it: a horizontal run nested
    /// inside some other column has nothing to do with this split, and evening it out would move
    /// dividers somebody had placed by hand in a part of the window they were not touching. A split
    /// of the *other* axis is therefore where the walk stops looking and starts recursing —
    /// [`Node::slots`] counts such a child as one slot, whatever is inside it.
    fn even_run(&mut self, pane: PaneId, horizontal: bool) {
        if !self.holds(pane) {
            return;
        }
        // The top of the run: the first node on the way down that divides the axis this split did.
        if matches!(self, Self::Split { horizontal: h, .. } if *h == horizontal) {
            self.even(horizontal);
            return;
        }
        if let Self::Split { first, second, .. } = self {
            first.even_run(pane, horizontal);
            second.even_run(pane, horizontal);
        }
    }

    /// How many panes a subtree contributes to a run of `horizontal` splits.
    ///
    /// One for anything that is not a split of that axis — a pane, or a whole column standing in a
    /// row — because that is exactly what such a subtree is to the run: one slot, of whatever it
    /// contains.
    fn slots(&self, horizontal: bool) -> usize {
        match self {
            Self::Split {
                horizontal: h,
                first,
                second,
                ..
            } if *h == horizontal => first.slots(horizontal) + second.slots(horizontal),
            _ => 1,
        }
    }

    /// The evening out itself, from the top of a run downwards.
    ///
    /// Not clamped to [`RATIO_MIN`]: those two are what a *drag* is held to, and an even share of
    /// nine panes is narrower than a drag would ever produce while still being a share nobody has to
    /// find the edge of — every divider is where the arithmetic says it is. [`HELD_MIN`] is what the
    /// settings file will read such a tree back at.
    fn even(&mut self, horizontal: bool) {
        let Self::Split {
            horizontal: h,
            ratio,
            first,
            second,
        } = self
        else {
            return;
        };
        // Reached by recursing into a child that turned out to divide the other axis, which is where
        // this run ends: that subtree keeps its own dividers.
        if *h != horizontal {
            return;
        }
        let mine = first.slots(horizontal);
        let theirs = second.slots(horizontal);
        *ratio = mine as f32 / (mine + theirs) as f32;
        first.even(horizontal);
        second.even(horizontal);
    }

    /// Whether `pane` is somewhere in this subtree.
    fn holds(&self, pane: PaneId) -> bool {
        match self {
            Self::Leaf(id) => *id == pane,
            Self::Split { first, second, .. } => first.holds(pane) || second.holds(pane),
        }
    }

    /// Take a pane out, collapsing the split it was half of — and even up what is left.
    ///
    /// Returns `false` when `pane` is the root — the caller has to decide what an
    /// empty window means, and here it means the last pane cannot be closed.
    ///
    /// **The evening out is the other half of [`Node::split`]'s**, and it is the same claim from the
    /// other end: three columns that lose one are two columns, and two columns are halves. Without it
    /// the shares left behind are whatever the arithmetic of three was — closing the third of three
    /// left `[1/3, 2/3]`, so a window came back from a close lopsided in a way no gesture had asked
    /// for.
    ///
    /// Only the run that lost a slot — see [`Node::remove_at`], where working out *which* one that is
    /// turns out to be the whole of the problem. So a *column* disappearing out of a row leaves the
    /// row's own dividers alone: no run of the row's is a slot short, and the pane that was sharing the
    /// column simply inherits what the column had.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        match self.remove_at(pane) {
            None => false,
            // The run reaches the root, so the root is the top of it — and after a removal *at* the
            // root the root is the promoted subtree, which is the rest of the run when the split that
            // collapsed was part of a longer one. `even` answers both by no-opping on a node that
            // divides the other axis.
            Some(Removed::Run(axis)) => {
                self.even(axis);
                true
            }
            Some(Removed::Done) => true,
        }
    }

    /// The removal itself, reporting whether a run is still waiting to be evened out.
    ///
    /// **The awkward part is knowing which run lost a pane**, and it is not "the run the surviving
    /// neighbour is in": a split that collapses entirely — `h(C, D)` losing `D` — takes its whole run
    /// with it when it was the only node of one, and the run above *that* has exactly as many slots as
    /// it had before. Evening from the neighbour downwards would find the row the vanished column stood
    /// in and level dividers somebody had placed by hand in a part of the window nothing happened to.
    ///
    /// So the axis is carried back up instead. Every node it passes through that divides the same axis
    /// is part of the same run and passes it on; the first node that divides the *other* axis is
    /// standing above the top of the run, and evens it in the child it came from. A run that reaches
    /// the root is [`Node::remove`]'s to even, which is the only reason this is two functions.
    fn remove_at(&mut self, pane: PaneId) -> Option<Removed> {
        let Self::Split {
            horizontal,
            first,
            second,
            ..
        } = self
        else {
            return None;
        };
        let horizontal = *horizontal;

        for which in [0, 1] {
            let child = if which == 0 { &**first } else { &**second };
            if matches!(child, Self::Leaf(id) if *id == pane) {
                // Promote the sibling into this node's place.
                let sibling = if which == 0 { second } else { first };
                let promoted = std::mem::replace(&mut **sibling, Self::Leaf(pane));
                *self = promoted;
                return Some(Removed::Run(horizontal));
            }
        }

        // Deeper — and **which child** it happened in matters, because a run that ends here is one
        // this node has to even inside that child.
        let (below, child) = match first.remove_at(pane) {
            Some(below) => (below, &mut **first),
            None => (second.remove_at(pane)?, &mut **second),
        };
        Some(match below {
            // The run carries on through this node, so this node is not the top of it.
            Removed::Run(axis) if axis == horizontal => Removed::Run(axis),
            // It stopped below: `child` is the top of whatever is left of it.
            Removed::Run(axis) => {
                child.even(axis);
                Removed::Done
            }
            Removed::Done => Removed::Done,
        })
    }

    /// Every pane, in layout order.
    pub fn panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.panes(out);
                second.panes(out);
            }
        }
    }

    /// How many panes the tree holds.
    ///
    /// Only the tests ask: they assert on the shape of the tree itself, which is a
    /// stronger claim than the length of the pane list built from it. Nothing in the
    /// window needs it — a pane count is `panes.len()` there.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { first, second, .. } => first.count() + second.count(),
        }
    }

    // ---- The settings file ---------------------------------------------
    //
    // A tree in one line, so that the next launch comes up with the panes this one had
    // rather than with every tab stacked in a single pane:
    //
    // ```text
    // h0.500(0,v0.667(1,2))
    // ```
    //
    // `h` and `v` are how the split divides its space, the number after it is the share
    // the first child gets, and a bare integer is a pane. **Panes are numbered by their
    // position in layout order, not by [`PaneId`]** — an id is a counter that runs for the
    // life of one window and means nothing to the next one, whereas "the second pane from
    // the left" survives being written down. `panes` walks the tree in the same order this
    // numbers it, which is what lets the caller line the two up.
    //
    // Hand-rolled, as the rest of the settings file is, and for the same reason: the
    // grammar is four productions and the parser below is shorter than the derive and the
    // format crate a serialised version would need.

    /// Write the tree out for the settings file.
    pub fn encode(&self) -> String {
        let mut text = String::new();
        self.write_into(&mut text, &mut 0);
        text
    }

    fn write_into(&self, text: &mut String, next: &mut usize) {
        use std::fmt::Write as _;
        match self {
            Self::Leaf(_) => {
                let _ = write!(text, "{next}");
                *next += 1;
            }
            Self::Split {
                horizontal,
                ratio,
                first,
                second,
            } => {
                let _ = write!(text, "{}{ratio:.3}(", if *horizontal { 'h' } else { 'v' });
                first.write_into(text, next);
                text.push(',');
                second.write_into(text, next);
                text.push(')');
            }
        }
    }

    /// Read one back, giving the pane numbered `n` the id `ids[n]`.
    ///
    /// `None` for anything that is not a tree over exactly these panes — a truncated line, a
    /// pane named twice, a pane not named at all, a number with no pane to go with it. All of
    /// those are one answer rather than several because there is only one thing to do about
    /// them: open the window the way a first run would. A settings file is not worth refusing
    /// to start over, and a *partly* restored layout would be worse than an honest default —
    /// it would be a window missing one of the folders that were open in it.
    pub fn decode(text: &str, ids: &[PaneId]) -> Option<Self> {
        /// How deeply a settings file may nest. Reached only by a file that was written by
        /// hand, and the reason for the limit is that this parser recurses: without it, a
        /// line of ten thousand `h0.5(` would overflow the stack rather than be rejected.
        const DEPTH: usize = 32;

        fn parse(rest: &mut &str, ids: &[PaneId], taken: &mut [bool], depth: usize) -> Option<Node> {
            if depth > DEPTH {
                return None;
            }
            let horizontal = match rest.as_bytes().first()? {
                b'h' => true,
                b'v' => false,
                _ => {
                    // A pane: its number, and then whatever follows it.
                    let end = rest
                        .find(|c: char| !c.is_ascii_digit())
                        .unwrap_or(rest.len());
                    let which: usize = rest[..end].parse().ok()?;
                    *rest = &rest[end..];
                    let id = *ids.get(which)?;
                    // Named twice: not a tree. Two leaves for one pane would draw the same
                    // pane in two places and every gesture would find the first of them.
                    if std::mem::replace(taken.get_mut(which)?, true) {
                        return None;
                    }
                    return Some(Node::Leaf(id));
                }
            };
            *rest = &rest[1..];

            // The ratio runs up to this split's own bracket. Nothing before it can contain
            // one, because a ratio is digits and a point.
            let open = rest.find('(')?;
            let ratio: f32 = rest[..open].parse().ok()?;
            // `"NaN".parse::<f32>()` succeeds, and a NaN ratio lays out a pane of NaN width
            // that never comes back. The clamp cannot catch it — `f32::clamp` passes NaN
            // through — so it is refused here.
            if !ratio.is_finite() {
                return None;
            }
            *rest = &rest[open + 1..];

            let first = parse(rest, ids, taken, depth + 1)?;
            *rest = rest.strip_prefix(',')?;
            let second = parse(rest, ids, taken, depth + 1)?;
            *rest = rest.strip_prefix(')')?;

            Some(Node::Split {
                horizontal,
                ratio: ratio.clamp(HELD_MIN, HELD_MAX),
                first: Box::new(first),
                second: Box::new(second),
            })
        }

        let mut taken = vec![false; ids.len()];
        let mut rest = text.trim();
        let tree = parse(&mut rest, ids, &mut taken, 0)?;
        // Every pane accounted for, and nothing left over. A pane with no leaf would be a
        // folder the user had open and cannot see; a tail would mean this parsed something
        // other than what was written.
        if !rest.is_empty() || taken.iter().any(|&used| !used) {
            return None;
        }
        Some(tree)
    }
}

/// The narrowest share a splitter *drag* will give a pane, and its mirror.
///
/// A pane at 0.02 of the window is a pane you cannot find the edge of again, so the gesture that
/// could produce one is held here.
pub const RATIO_MIN: f32 = 0.12;
pub const RATIO_MAX: f32 = 0.88;

/// The narrowest share the tree will *hold*, and its mirror — which is not the same question.
///
/// What [`Node::decode`] accepts, and it has to be wider than the drag's clamp because [`Node::even`]
/// produces ratios a drag never would: the first of `n` equal panes gets `1/n`, and the settings file
/// allows two dozen of them. Clamped to the drag's floor, a window left with nine columns would have
/// reopened with the first of them a point wider than the rest and every divider after it out of
/// place — the layout arriving *slightly* wrong, which is worse than either extreme.
///
/// It is still a floor rather than nothing, because this is also what a hand-edited file is held to:
/// `1/32` is narrower than any arrangement this window can build and wide enough to grab.
pub const HELD_MIN: f32 = 1.0 / 32.0;
pub const HELD_MAX: f32 = 1.0 - HELD_MIN;

/// The air between the middle mark and the four around it, as a share of one mark's side.
///
/// **This is what sizes "into this pane", not just what keeps the marks apart.** [`zone_at`] takes
/// the nearest mark, so the region that reaches the middle one is its own square plus half of this
/// in every direction — the boundary sits midway across the air. At the six flat points this used
/// to be, moving a tab into another pane meant aiming at a square with three points of margin
/// round it, in a pane four hundred wide, with a split either side of that. Half a mark of air
/// makes the middle a target rather than a bullseye.
///
/// A share of the mark rather than a figure of its own, so one number governs the whole compass
/// and a pane too small for the full size loses its air in the same proportion as its marks. It
/// also puts the four outer marks nearer the edges they stand for, which is where a reader looks
/// for them.
const COMPASS_AIR: f32 = 0.5;

/// The compass's two measures in a pane of any shape: the side of one mark, and the air around the
/// middle one.
///
/// **All five marks are this one square.** They are read together — five marks arranged in a plus,
/// saying "here are the five places" — and five rectangles of five different proportions is a
/// diagram of nothing. It was one, until the four outer marks ran out to the pane's edges: that
/// made each of them as long as the pane happened to be in that direction, so a wide pane drew two
/// long slabs either side of two stubs.
///
/// Off the *smaller* extent, because the whole figure is `3 × side + 2 × air` in both directions
/// and has to fit inside the pane either way. Clamped: a quarter of a 2000-point pane is a
/// 500-point square, which is a wall rather than a mark, and a quarter of a 90-point one is too
/// small to aim at. The last term is the fit, and it is what a pane too small for even the floor
/// gives way by — never to nothing, because the air is a share of what is left.
fn compass(pane: Rect) -> (f32, f32) {
    let extent = pane.width().min(pane.height());
    let side = (extent * 0.26)
        .clamp(24.0, 88.0)
        .min(extent / (3.0 + 2.0 * COMPASS_AIR));
    (side, side * COMPASS_AIR)
}

/// **The mark a zone is drawn as, and the seed [`zone_at`] measures against.**
///
/// Five equal squares in a plus, centred in the pane: the middle one for dropping *into* it, and
/// one out towards each edge for splitting. Drawn exactly as returned — what you see is what you
/// aim at.
///
/// It is not what you *get*, and the difference is the point. [`preview_rect`] is the result —
/// half the pane for a split — and marks the size of a result could not be the marks: two halves
/// of one pane overlap across the whole middle of it, so every point would be asking for two
/// things at once, and a hint whose area overlaps its neighbour's is a hint that cannot say which
/// one it is.
///
/// The marks are also smaller than the region that *reaches* each of them — see [`zone_at`], which
/// takes the nearest. That is the forgiving direction to be wrong in: a square you have to land
/// inside would be a square you miss, and it would take "throw the tab at the pane's right-hand
/// edge" away, which is the gesture people arrive with.
pub fn hint_rect(pane: Rect, zone: Zone) -> Rect {
    let (side, air) = compass(pane);
    let step = side + air;
    let middle = Rect::from_center_size(pane.center(), vec2(side, side));
    let along = |dx: f32, dy: f32| middle.translate(vec2(dx * step, dy * step));
    match zone {
        Zone::Into => middle,
        Zone::Split(Side::Left) => along(-1.0, 0.0),
        Zone::Split(Side::Right) => along(1.0, 0.0),
        Zone::Split(Side::Top) => along(0.0, -1.0),
        Zone::Split(Side::Bottom) => along(0.0, 1.0),
    }
}

/// Which part of a pane the pointer is over, for a tab being dragged.
///
/// **The nearest mark wins**, measured to [`hint_rect`] and zero once inside it. So the marks are
/// what the pointer is aimed at while the whole pane stays live: the left-hand edge, and the
/// corners either side of it, are all nearer the left mark than anything else, which is what keeps
/// the throw-it-at-the-edge gesture working with a compass small enough to look like one.
///
/// The other side of that is what [`COMPASS_AIR`] is for: every boundary sits midway between two
/// marks, so how much of the pane means "into it" is decided by how far the four are pushed off
/// the middle one.
///
/// A partition, so nothing overlaps and nothing is dead. Where two marks are exactly equidistant
/// the earlier one in [`Zone::ALL`] takes it, which puts the centre — the answer that leaves the
/// layout alone — ahead of a split, and settles the diagonals of a square pane on the left and
/// right rather than on nothing.
pub fn zone_at(pane: Rect, pointer: Pos2) -> Zone {
    Zone::ALL
        .into_iter()
        .min_by(|&a, &b| {
            let reach = |zone| hint_rect(pane, zone).distance_sq_to_pos(pointer);
            reach(a).total_cmp(&reach(b))
        })
        .unwrap_or(Zone::Into)
}

/// The rect a drop preview should highlight.
pub fn preview_rect(pane: Rect, zone: Zone) -> Rect {
    match zone {
        Zone::Into => pane,
        Zone::Split(side) => {
            let half = |extent: f32| (extent - GAP) * 0.5;
            match side {
                Side::Left => Rect::from_min_size(pane.min, vec2(half(pane.width()), pane.height())),
                Side::Right => Rect::from_min_max(
                    pos2(pane.right() - half(pane.width()), pane.top()),
                    pane.max,
                ),
                Side::Top => Rect::from_min_size(pane.min, vec2(pane.width(), half(pane.height()))),
                Side::Bottom => Rect::from_min_max(
                    pos2(pane.left(), pane.bottom() - half(pane.height())),
                    pane.max,
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests;
