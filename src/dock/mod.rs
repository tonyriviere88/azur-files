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
    pub fn split(&mut self, target: PaneId, side: Side, added: PaneId) -> bool {
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
                first.split(target, side, added) || second.split(target, side, added)
            }
        }
    }

    /// Take a pane out, collapsing the split it was half of.
    ///
    /// Returns `false` when `pane` is the root — the caller has to decide what an
    /// empty window means, and here it means the last pane cannot be closed.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        let Self::Split { first, second, .. } = self else {
            return false;
        };

        for which in [0, 1] {
            let child = if which == 0 { &**first } else { &**second };
            if matches!(child, Self::Leaf(id) if *id == pane) {
                // Promote the sibling into this node's place.
                let sibling = if which == 0 { second } else { first };
                let promoted = std::mem::replace(&mut **sibling, Self::Leaf(pane));
                *self = promoted;
                return true;
            }
        }
        first.remove(pane) || second.remove(pane)
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
                ratio: ratio.clamp(RATIO_MIN, RATIO_MAX),
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

/// The narrowest share a split will give a pane, and its mirror.
///
/// What a splitter drag clamps to, and so what [`Node::decode`] accepts: a ratio outside
/// this is one no gesture in this window can produce, and a pane at 0.02 of the window is a
/// pane you cannot find the edge of again.
pub const RATIO_MIN: f32 = 0.12;
pub const RATIO_MAX: f32 = 0.88;

/// Which part of a pane the pointer is over, for a tab being dragged.
///
/// The edge bands are a fraction of the pane rather than a fixed size, so the
/// gesture feels the same in a narrow pane as in a wide one — but clamped, because
/// a 30% band on a 2000-pixel pane would mean the centre is unreachable, and a 30%
/// band on a 200-pixel one would be too small to hit.
pub fn zone_at(pane: Rect, pointer: Pos2) -> Zone {
    let band = |extent: f32| (extent * 0.30).clamp(28.0, 140.0);
    let (bx, by) = (band(pane.width()), band(pane.height()));

    let from_left = pointer.x - pane.left();
    let from_right = pane.right() - pointer.x;
    let from_top = pointer.y - pane.top();
    let from_bottom = pane.bottom() - pointer.y;

    // Nearest edge wins, so the corners resolve to whichever side the pointer is
    // actually closer to instead of to whichever test ran first.
    let mut best: Option<(f32, Side)> = None;
    for (distance, limit, side) in [
        (from_left, bx, Side::Left),
        (from_right, bx, Side::Right),
        (from_top, by, Side::Top),
        (from_bottom, by, Side::Bottom),
    ] {
        if distance < limit && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, side));
        }
    }

    match best {
        Some((_, side)) => Zone::Split(side),
        None => Zone::Into,
    }
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
