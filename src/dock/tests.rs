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

/// A ratio from outside comes back inside the range a drag can reach.
#[test]
fn a_ratio_out_of_range_is_brought_back() {
    let tree = Node::decode("h0.999(0,1)", &[1, 2]).expect("a wild ratio is not a broken file");
    let Node::Split { ratio, .. } = tree else {
        panic!("that is a split")
    };
    assert_eq!(ratio, RATIO_MAX);
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

#[test]
fn a_corner_resolves_to_the_nearer_edge() {
    let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
    // Closer to the top than to the left.
    assert_eq!(zone_at(pane, pos2(40.0, 6.0)), Zone::Split(Side::Top));
    // And the other way round.
    assert_eq!(zone_at(pane, pos2(6.0, 40.0)), Zone::Split(Side::Left));
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
