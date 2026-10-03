use super::*;

fn ids(node: &Node) -> Vec<PaneId> {
    let mut out = Vec::new();
    node.panes(&mut out);
    out
}

#[test]
fn splitting_right_puts_the_new_pane_second() {
    let mut tree = Node::Leaf(1);
    assert!(tree.split(1, Side::Right, 2));
    assert_eq!(ids(&tree), [1, 2]);
}

#[test]
fn splitting_left_puts_the_new_pane_first() {
    let mut tree = Node::Leaf(1);
    assert!(tree.split(1, Side::Left, 2));
    assert_eq!(ids(&tree), [2, 1]);
}

#[test]
fn splitting_a_nested_pane_finds_it() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    assert!(tree.split(2, Side::Bottom, 3));
    assert_eq!(ids(&tree), [1, 2, 3]);
    assert_eq!(tree.count(), 3);
}

#[test]
fn removing_collapses_the_split() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    tree.split(2, Side::Bottom, 3);

    assert!(tree.remove(3));
    assert_eq!(ids(&tree), [1, 2]);
    assert!(tree.remove(1));
    assert_eq!(ids(&tree), [2]);
    assert!(
        !tree.remove(2),
        "the last pane has nowhere to collapse into"
    );
}

#[test]
fn layout_leaves_one_gap_between_panes() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);

    // Width chosen from `GAP` rather than written down, so the two halves stay whole
    // numbers and the test says something about the split rather than about the constant.
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0 + GAP, 200.0));
    let (mut panes, mut splitters) = (Vec::new(), Vec::new());
    tree.layout(rect, &mut panes, &mut splitters);

    assert_eq!(panes.len(), 2);
    assert_eq!(splitters.len(), 1);
    assert_eq!(panes[0].1.width(), 200.0);
    assert_eq!(panes[1].1.width(), 200.0);
    assert_eq!(panes[1].1.left() - panes[0].1.right(), GAP);
    assert_eq!(panes[0].1.height(), 200.0, "a side split is full height");
}

#[test]
fn ratios_are_reachable_by_route() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    tree.split(2, Side::Bottom, 3);

    let rect = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0));
    let (mut panes, mut splitters) = (Vec::new(), Vec::new());
    tree.layout(rect, &mut panes, &mut splitters);

    for splitter in &splitters {
        assert!(
            tree.ratio_at(&splitter.route).is_some(),
            "route {:?} has to lead to the split it came from",
            splitter.route
        );
    }
    // The nested split is the second child of the root.
    *tree.ratio_at(&[1]).unwrap() = 0.25;
    tree.layout(rect, &mut panes, &mut splitters);
    assert!(panes[1].1.height() < panes[2].1.height());
}

/// A layout survives being written down and read back.
///
/// The panes come back in the same order and the splits with the same shape and the same
/// ratios — which is the whole claim the settings file makes. Round-tripped rather than
/// compared against a literal, because the text is an implementation detail and the
/// window the user gets back is not.
#[test]
fn a_layout_survives_the_settings_file() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    tree.split(2, Side::Bottom, 3);
    *tree.ratio_at(&[]).unwrap() = 0.4;
    *tree.ratio_at(&[1]).unwrap() = 0.75;

    let text = tree.encode();
    assert_eq!(text, "h0.400(0,v0.750(1,2))", "{text}");

    // Deliberately not the ids it was written with: the file numbers panes by position,
    // so a fresh window's ids are what it is read back over.
    let back = Node::decode(&text, &[7, 8, 9]).expect("its own output has to parse");
    assert_eq!(ids(&back), [7, 8, 9]);
    assert_eq!(back.encode(), text, "the shape and the ratios both come back");
}

/// Anything that is not a tree over exactly these panes opens as a first run would.
///
/// Each of these is a settings file somebody could produce — by hand, by a crash
/// half-way through a write, or by opening a file this program wrote and then closing a
/// pane in an older build of it. The failure has to be "the window opens plainly",
/// never a panic and never a window with one of the open folders missing from it.
#[test]
fn a_layout_that_is_not_one_is_refused() {
    let three = [1, 2, 3];
    for text in [
        "",
        "h0.5(0,1",              // truncated
        "h0.5(0 1)",             // no comma
        "h0.5(0,1))",            // a tail
        "x0.5(0,1)",             // not a direction
        "h(0,1)",                // no ratio
        "hNaN(0,1)",             // parses as a float and lays out nothing
        "h0.5(0,0)",             // the same pane twice
        "h0.5(0,1)",             // pane 2 has nowhere to be drawn
        "h0.5(0,v0.5(1,9))",     // a pane that does not exist
    ] {
        assert!(
            Node::decode(text, &three).is_none(),
            "{text:?} was accepted as a layout over three panes"
        );
    }
    // And the one-pane case, which is the shortest legal line there is.
    assert!(matches!(Node::decode("0", &[4]), Some(Node::Leaf(4))));
}

/// A ratio from outside comes back inside the range the tree will hold.
///
/// [`HELD_MAX`] rather than [`RATIO_MAX`], which is the drag's: an even split of many panes gives the
/// first of them a share narrower than any drag, so reading such a file back at the drag's floor
/// would put every divider of a nine-column window a little out of place.
#[test]
fn a_ratio_out_of_range_is_brought_back() {
    let tree = Node::decode("h0.999(0,1)", &[1, 2]).expect("a wild ratio is not a broken file");
    let Node::Split { ratio, .. } = tree else {
        panic!("that is a split")
    };
    assert_eq!(ratio, HELD_MAX);
}

/// The share of each split of a run, in the order the run is walked in.
fn ratios(node: &Node) -> Vec<f32> {
    let mut out = Vec::new();
    fn walk(node: &Node, out: &mut Vec<f32>) {
        if let Node::Split {
            ratio,
            first,
            second,
            ..
        } = node
        {
            out.push(*ratio);
            walk(first, out);
            walk(second, out);
        }
    }
    walk(node, &mut out);
    out
}

/// Where each pane ends up across a window of `width`, in the order they are laid out.
fn widths(node: &Node, width: f32) -> Vec<f32> {
    let (mut panes, mut splitters) = (Vec::new(), Vec::new());
    node.layout(
        Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 200.0)),
        &mut panes,
        &mut splitters,
    );
    panes.iter().map(|(_, rect)| rect.width()).collect()
}

/// And the same down the window.
fn heights(node: &Node, height: f32) -> Vec<f32> {
    let (mut panes, mut splitters) = (Vec::new(), Vec::new());
    node.layout(
        Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, height)),
        &mut panes,
        &mut splitters,
    );
    panes.iter().map(|(_, rect)| rect.height()).collect()
}

/// Every pane an equal share, to within the point a divider is rounded onto.
///
/// A point of slack rather than exact equality, and it is [`Node::layout_into`]'s rounding rather
/// than the ratios': a divider is put on a whole pixel so its one-pixel edges do not go grey, so
/// three panes across 302 points are 101, 100 and 100. The ratios themselves are exact — the tests
/// below assert on those directly.
fn assert_even(shares: &[f32], across: f32) {
    let fair = (across - GAP * (shares.len() - 1) as f32) / shares.len() as f32;
    for share in shares {
        assert!(
            (share - fair).abs() <= 1.0,
            "{shares:?} is not {fair} each"
        );
    }
}

/// A third pane in a row is a third of the row, not a quarter of it.
///
/// The bug this is about: each split halves the node it lands on, so splitting the right-hand half
/// of a divided window used to give `[1/2, 1/4, 1/4]`.
#[test]
fn a_third_pane_takes_a_third_of_the_row() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    assert_eq!(ratios(&tree), [0.5]);

    tree.split(2, Side::Right, 3);
    assert_eq!(ratios(&tree), [1.0 / 3.0, 0.5]);
    let across = 300.0 + GAP * 2.0;
    assert_even(&widths(&tree, across), across);

    tree.split(3, Side::Right, 4);
    assert_eq!(ratios(&tree), [0.25, 1.0 / 3.0, 0.5]);
    let across = 400.0 + GAP * 3.0;
    assert_even(&widths(&tree, across), across);
}

/// Which end the pane was added at makes no difference to the shares.
#[test]
fn a_row_built_leftwards_is_even_too() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Left, 2);
    tree.split(2, Side::Left, 3);
    // `h(h(3, 2), 1)`: the new pane goes *first* each time, so the run leans the other way and the
    // shares have to be read off the slot counts rather than off the order of the splits — two
    // thirds at the root, and a half inside it.
    assert_eq!(ids(&tree), [3, 2, 1]);
    assert_eq!(ratios(&tree), [2.0 / 3.0, 0.5]);
    let across = 300.0 + GAP * 2.0;
    assert_even(&widths(&tree, across), across);
}

/// Adding to the middle of a run evens the run, whichever node the tree hangs it off.
#[test]
fn splitting_the_first_pane_of_a_row_still_evens_it() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    // `h(h(1, 3), 2)`: a left-leaning run, which is the shape the general rule is for — two thirds
    // at the root and a half inside it.
    tree.split(1, Side::Right, 3);
    assert_eq!(ids(&tree), [1, 3, 2]);
    assert_eq!(ratios(&tree), [2.0 / 3.0, 0.5]);
    let across = 300.0 + GAP * 2.0;
    assert_even(&widths(&tree, across), across);
}

/// Three columns that lose one are two halves, not a third and two thirds.
///
/// The close half of the same rule, and the one that is reached by two gestures: closing a pane's last
/// tab, and dragging that tab into another pane so the one it left is empty. Both go through
/// [`Node::remove`].
#[test]
fn closing_one_of_three_columns_evens_the_two_left() {
    for closed in [1, 2, 3] {
        let mut tree = Node::Leaf(1);
        tree.split(1, Side::Right, 2);
        tree.split(2, Side::Right, 3);
        assert!(tree.remove(closed));
        assert_eq!(
            ratios(&tree),
            [0.5],
            "closing pane {closed} of three left the other two uneven"
        );
        let across = 200.0 + GAP;
        assert_even(&widths(&tree, across), across);
    }
}

/// And four that lose one are thirds.
#[test]
fn closing_one_of_four_columns_leaves_thirds() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    tree.split(2, Side::Right, 3);
    tree.split(3, Side::Right, 4);
    assert!(tree.remove(2));
    assert_eq!(ratios(&tree), [1.0 / 3.0, 0.5]);
    let across = 300.0 + GAP * 2.0;
    assert_even(&widths(&tree, across), across);
}

/// A column disappearing out of a row leaves the row's own dividers where they were.
///
/// There is no row that lost a slot — the row still has the same number of columns in it — so the pane
/// that was sharing the column inherits what the column had and nothing else moves. Which is the
/// difference between evening out the run that changed and levelling the window on every close.
#[test]
fn losing_half_a_column_does_not_move_the_row() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    tree.split(1, Side::Bottom, 3);
    // The row, dragged off centre by hand.
    if let Some(ratio) = tree.ratio_at(&[]) {
        *ratio = 0.7;
    }

    assert!(tree.remove(3));
    assert_eq!(ids(&tree), [1, 2]);
    assert_eq!(
        tree.ratio_at(&[]).copied(),
        Some(0.7),
        "the row was levelled by a close that happened inside one of its columns"
    );
}

/// A run that vanishes whole takes nothing else with it.
///
/// The trap in working out which run lost a pane. `h(1, v(2, h(3, 4)))` is a row of two whose second
/// column holds a pane and a *nested* row of two; closing 4 collapses that nested row entirely, so the
/// run it was is gone — and the outer row still has exactly the two columns it had. The obvious
/// implementation, evening out from the surviving neighbour, finds the outer row instead and levels a
/// divider nothing happened to.
#[test]
fn a_run_that_disappears_evens_nothing() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    tree.split(2, Side::Bottom, 3);
    tree.split(3, Side::Right, 4);
    // The outer row, dragged off centre by hand — and it has to be read back after the split above,
    // which is a row of its own and does not touch this one.
    if let Some(ratio) = tree.ratio_at(&[]) {
        *ratio = 0.7;
    }

    assert!(tree.remove(4));
    assert_eq!(
        tree.ratio_at(&[]).copied(),
        Some(0.7),
        "the outer row was levelled by a close in a nested row that vanished with it"
    );
}

/// And a run that is only *shortened* deep in the tree is evened where it is.
///
/// The other side of the same walk: three panes across inside a column, one of them closing. The run
/// that lost a slot is that inner row, not the column it stands in and not the row the column is in.
#[test]
fn a_shortened_run_deep_in_the_tree_is_the_one_evened() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    tree.split(2, Side::Bottom, 3);
    // Pane 3 becomes a row of three, inside the lower half of the right-hand column.
    tree.split(3, Side::Right, 4);
    tree.split(4, Side::Right, 5);
    if let Some(ratio) = tree.ratio_at(&[]) {
        *ratio = 0.7;
    }

    assert!(tree.remove(5));
    assert_eq!(
        tree.ratio_at(&[]).copied(),
        Some(0.7),
        "the outer row is nothing to do with a close three levels down"
    );
    // `[1, 1, 0]` is the inner row's own node: right child of the root, second child of the column.
    assert_eq!(
        tree.ratio_at(&[1, 1]).copied(),
        Some(0.5),
        "the row that lost one of its three panes is still divided as three"
    );
}

/// Rows and columns are the same rule on the other axis.
#[test]
fn a_third_pane_below_takes_a_third_of_the_height() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Bottom, 2);
    tree.split(2, Side::Bottom, 3);
    assert_eq!(ratios(&tree), [1.0 / 3.0, 0.5]);
    let down = 300.0 + GAP * 2.0;
    assert_even(&heights(&tree, down), down);
}

/// A column standing in a row counts as one of the row's panes, and keeps its own divider.
///
/// Which is the whole reason the evening out walks down to the run the new pane joined instead of
/// levelling every split of that axis: the column here was divided by hand, and a split in the row
/// beside it is not a gesture about the column.
#[test]
fn a_split_of_the_other_axis_is_one_slot_and_is_left_alone() {
    let mut tree = Node::Leaf(1);
    tree.split(1, Side::Right, 2);
    // Pane 1 becomes a column of two, dragged to a third / two thirds.
    tree.split(1, Side::Bottom, 3);
    if let Some(ratio) = tree.ratio_at(&[0]) {
        *ratio = 0.3;
    }
    // And now a third *column* of the row.
    tree.split(2, Side::Right, 4);

    assert_eq!(
        tree.ratio_at(&[0]).copied(),
        Some(0.3),
        "the hand-placed divider inside the column moved"
    );
    // Four panes and three columns: the column's two are the same width as each other and as the
    // other two, which is what "one slot" means.
    let across = 300.0 + GAP * 2.0;
    let fair = (across - GAP * 2.0) / 3.0;
    for width in widths(&tree, across) {
        assert!(
            (width - fair).abs() <= 1.0,
            "a pane {width} wide in three columns of {fair}"
        );
    }
}

/// Nesting deep enough to recurse into the stack is refused rather than run.
#[test]
fn a_hand_written_file_cannot_overflow_the_stack() {
    let deep = "h0.5(0,".repeat(10_000);
    assert!(Node::decode(&deep, &[1, 2]).is_none());
}

#[test]
fn the_centre_of_a_pane_is_a_tab_drop() {
    let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
    assert_eq!(zone_at(pane, pane.center()), Zone::Into);
    assert_eq!(zone_at(pane, pos2(5.0, 200.0)), Zone::Split(Side::Left));
    assert_eq!(zone_at(pane, pos2(595.0, 200.0)), Zone::Split(Side::Right));
    assert_eq!(zone_at(pane, pos2(300.0, 3.0)), Zone::Split(Side::Top));
    assert_eq!(zone_at(pane, pos2(300.0, 397.0)), Zone::Split(Side::Bottom));
}

/// A corner is nearer one of the two splits that meet there than it is to the other, and that is
/// the one it asks for — which is what keeps "throw the tab at that end of the pane" working with
/// a compass small enough to look like one.
#[test]
fn a_corner_goes_to_the_nearer_mark() {
    // Wider than it is tall, so the left and right marks are the near ones at every corner.
    let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
    assert_eq!(zone_at(pane, pos2(6.0, 6.0)), Zone::Split(Side::Left));
    assert_eq!(zone_at(pane, pos2(6.0, 394.0)), Zone::Split(Side::Left));
    assert_eq!(zone_at(pane, pos2(594.0, 6.0)), Zone::Split(Side::Right));
    assert_eq!(zone_at(pane, pos2(594.0, 394.0)), Zone::Split(Side::Right));
    // And the other way up, where the near ones are top and bottom.
    let tall = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 600.0));
    assert_eq!(zone_at(tall, pos2(6.0, 6.0)), Zone::Split(Side::Top));
    assert_eq!(zone_at(tall, pos2(394.0, 594.0)), Zone::Split(Side::Bottom));
}

/// **All five marks are the same square, and they sit in a plus around the pane's centre.**
///
/// Which is what makes them read as one compass rather than as five rectangles: equal, square,
/// evenly spaced, symmetric about both axes. It held for the middle alone once and not for the
/// four around it — they ran out to the pane's edges, so each was as long as the pane happened to
/// be in that direction and a wide pane drew two slabs either side of two stubs.
///
/// The side comes off the pane's smaller extent, so this has to hold on a pane of any shape —
/// including one more than twice as wide as it is tall, where a square cut from the width would
/// not fit at all.
#[test]
fn the_compass_is_five_equal_squares_in_a_plus() {
    for size in [
        vec2(600.0, 400.0),
        vec2(2000.0, 600.0),
        vec2(180.0, 90.0),
        vec2(300.0, 900.0),
        vec2(400.0, 400.0),
    ] {
        let pane = Rect::from_min_size(pos2(40.0, 20.0), size);
        let middle = hint_rect(pane, Zone::Into);
        assert_eq!(middle.center(), pane.center(), "the middle is not in the middle");
        let side = middle.width();
        for zone in Zone::ALL {
            let mark = hint_rect(pane, zone);
            assert!(
                (mark.width() - side).abs() < 0.01 && (mark.height() - side).abs() < 0.01,
                "the {zone:?} mark of a {size:?} pane is {}x{}, not the square {side}",
                mark.width(),
                mark.height()
            );
            assert!(
                pane.contains_rect(mark),
                "the {zone:?} mark of a {size:?} pane hangs outside it: {mark:?}"
            );
        }
        // Opposite marks are the same distance out, on the axis they belong to.
        let (left, right) = (
            hint_rect(pane, Zone::Split(Side::Left)),
            hint_rect(pane, Zone::Split(Side::Right)),
        );
        let (top, bottom) = (
            hint_rect(pane, Zone::Split(Side::Top)),
            hint_rect(pane, Zone::Split(Side::Bottom)),
        );
        assert_eq!(left.center().y, middle.center().y);
        assert_eq!(right.center().y, middle.center().y);
        assert_eq!(top.center().x, middle.center().x);
        assert_eq!(bottom.center().x, middle.center().x);
        let step = right.center().x - middle.center().x;
        assert!(
            (middle.center().x - left.center().x - step).abs() < 0.01
                && (bottom.center().y - middle.center().y - step).abs() < 0.01
                && (middle.center().y - top.center().y - step).abs() < 0.01,
            "the four are not evenly spaced around the middle of a {size:?} pane"
        );
        // And the air around the middle is a share of a mark, on every side of it: it is what
        // decides how much of the pane means "into this pane", so it is worth pinning rather
        // than leaving to whatever the spacing happens to work out as.
        let air = side * COMPASS_AIR;
        for (name, got) in [
            ("left", middle.left() - left.right()),
            ("right", right.left() - middle.right()),
            ("top", middle.top() - top.bottom()),
            ("bottom", bottom.top() - middle.bottom()),
        ] {
            assert!(
                (got - air).abs() < 0.01,
                "the air on the {name} of the middle of a {size:?} pane is {got}, not {air}"
            );
        }
    }
}

/// **Moving a tab into another pane does not mean threading a needle.**
///
/// What the air around the middle mark buys, written as the gesture rather than as geometry: the
/// pointer can be well outside the mark it is aiming at and still be asking to drop *into* the
/// pane. The boundary is midway across the air, which is the rule [`COMPASS_AIR`] is chosen by —
/// so this is where a smaller gap would show up as "it split when I wanted to move it".
#[test]
fn the_middle_reaches_out_across_half_the_air() {
    let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
    let middle = hint_rect(pane, Zone::Into);
    let air = middle.width() * COMPASS_AIR;
    assert!(air > 8.0, "the air is {air}, which is no more room than a hairline");

    let (cx, cy) = (pane.center().x, pane.center().y);
    // Outside the mark, inside the middle's reach.
    for at in [
        pos2(middle.right() + air * 0.4, cy),
        pos2(middle.left() - air * 0.4, cy),
        pos2(cx, middle.top() - air * 0.4),
        pos2(cx, middle.bottom() + air * 0.4),
    ] {
        assert_eq!(zone_at(pane, at), Zone::Into, "at {at:?}");
    }
    // And past the halfway line it is the split, so the reach is a boundary rather than a
    // preference: the four are still there to be hit.
    assert_eq!(
        zone_at(pane, pos2(middle.right() + air * 0.6, cy)),
        Zone::Split(Side::Right)
    );
    assert_eq!(
        zone_at(pane, pos2(cx, middle.top() - air * 0.6)),
        Zone::Split(Side::Top)
    );
}

/// **The marks do not overlap, and each is inside what dropping there would give.**
///
/// The first half is what makes the compass honest: two marks overlapping would be a point asking
/// for two things, and one of them would be lying about what a release does there. The second half
/// is the smaller-than-the-result rule — the mark is where you aim, the preview is what you get.
#[test]
fn the_marks_are_disjoint_and_smaller_than_what_they_give() {
    for size in [vec2(600.0, 400.0), vec2(2000.0, 1200.0), vec2(180.0, 90.0)] {
        let pane = Rect::from_min_size(pos2(40.0, 20.0), size);
        for (i, &zone) in Zone::ALL.iter().enumerate() {
            let mine = hint_rect(pane, zone);
            assert!(mine.width() > 0.0 && mine.height() > 0.0, "{zone:?} at {size:?}");
            assert!(
                preview_rect(pane, zone).contains_rect(mine),
                "{zone:?} at {size:?} is marked outside what it gives"
            );
            for &other in &Zone::ALL[i + 1..] {
                assert!(
                    !mine.intersects(hint_rect(pane, other)),
                    "{zone:?} and {other:?} overlap at {size:?}"
                );
            }
        }
        // And every zone is reachable at its own mark, which a compass clamped past the pane
        // would break.
        for &zone in &Zone::ALL {
            assert_eq!(zone_at(pane, hint_rect(pane, zone).center()), zone, "{size:?}");
        }
    }
}

#[test]
fn preview_of_a_side_split_is_half_the_pane() {
    // Width from `GAP`, as in `layout_leaves_one_gap_between_panes`: the preview shows what
    // the split would give, so it takes the same line out of the middle that the split does.
    let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0 + GAP, 400.0));
    let preview = preview_rect(pane, Zone::Split(Side::Right));
    assert_eq!(preview.width(), 300.0);
    assert_eq!(preview.right(), 600.0 + GAP);
    assert_eq!(preview_rect(pane, Zone::Into), pane);
}
