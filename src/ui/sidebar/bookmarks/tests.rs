//! The two things about a bookmark list that are worth checking without a window: where a
//! dragged row ends up in the model, and where the pointer says it is going.
//!
//! Everything else here — the rows, the chevron, the field a group is named in — is a click
//! target, and click targets are checked by driving real pointer events through real frames.
//! See `app::click_tests::sidebar`.

use super::*;

fn path(name: &str) -> PathBuf {
    PathBuf::from(name)
}

/// The list as one line, so an assertion reads the way the panel looks: `a [Work: b c] d`.
fn shape(marks: &Bookmarks) -> String {
    let mut out = String::new();
    for entry in marks.entries() {
        if !out.is_empty() {
            out.push(' ');
        }
        match entry {
            Entry::Mark(path) => out.push_str(&path.display().to_string()),
            Entry::Group(group) => {
                out.push('[');
                out.push_str(&group.name);
                out.push(':');
                for mark in &group.marks {
                    out.push(' ');
                    out.push_str(&mark.display().to_string());
                }
                out.push(']');
            }
        }
    }
    out
}

/// `a b [Work: c d] e`
fn nested() -> Bookmarks {
    let mut marks: Bookmarks = ["a", "b"].iter().map(|n| path(n)).collect();
    let group = marks.add_group("Work", true);
    marks.add_in(group, path("c"));
    marks.add_in(group, path("d"));
    marks.add(path("e"));
    marks
}

#[test]
fn a_flat_list_reorders_to_where_it_is_dropped() {
    // Insertion-point arithmetic, which is where an off-by-one is invisible until a list
    // quietly reverses itself. `to` is a position in the list *as it stands*, so it has to be
    // adjusted for the row having been taken out of it first.
    let mut marks: Bookmarks = ["a", "b", "c", "d"].iter().map(|n| path(n)).collect();

    // The first one to the end.
    assert!(marks.move_to(Spot::top(0), Spot::top(4)));
    assert_eq!(shape(&marks), "b c d a");
    // The last one to the front.
    assert!(marks.move_to(Spot::top(3), Spot::top(0)));
    assert_eq!(shape(&marks), "a b c d");
    // One place down: 2 means "before whatever is at 2 now".
    assert!(marks.move_to(Spot::top(0), Spot::top(2)));
    assert_eq!(shape(&marks), "b a c d");
    // Nowhere, twice: onto itself, and just after itself.
    assert!(!marks.move_to(Spot::top(1), Spot::top(1)));
    assert!(!marks.move_to(Spot::top(1), Spot::top(2)));
    assert_eq!(shape(&marks), "b a c d");
    // Out of range, from a stale drag: nothing moves and nothing panics.
    assert!(!marks.move_to(Spot::top(9), Spot::top(0)));
    assert!(!marks.move_to(Spot::top(0), Spot::top(9)));
    assert_eq!(shape(&marks), "b a c d");
}

#[test]
fn a_bookmark_goes_into_a_group_and_comes_back_out() {
    let mut marks = nested();
    assert_eq!(shape(&marks), "a b [Work: c d] e");

    // `a` into the group, between the two already in it.
    assert!(marks.move_to(Spot::top(0), Spot::in_group(2, 1)));
    assert_eq!(shape(&marks), "b [Work: c a d] e");

    // And out again, to the end of the top level. The group is entry 1 now, which is the part
    // an index into a mixed list gets wrong if nothing adjusts it.
    assert!(marks.move_to(Spot::in_group(1, 1), Spot::top(3)));
    assert_eq!(shape(&marks), "b [Work: c d] e a");
}

#[test]
fn taking_a_mark_out_of_the_top_level_renumbers_the_group_it_is_going_into() {
    // The one adjustment that is easy to miss: a group is named by its position in the same
    // list the row is being taken out of, so removing a row *before* it moves it.
    let mut marks: Bookmarks = ["a"].iter().map(|n| path(n)).collect();
    let group = marks.add_group("Work", true);
    assert_eq!(group, 1);

    // Entry 0 into the group at entry 1 — which becomes entry 0 the moment `a` leaves.
    assert!(marks.move_to(Spot::top(0), Spot::in_group(1, 0)));
    assert_eq!(shape(&marks), "[Work: a]");
}

#[test]
fn reordering_inside_a_group_stays_inside_it() {
    let mut marks = nested();
    assert!(marks.move_to(Spot::in_group(2, 1), Spot::in_group(2, 0)));
    assert_eq!(shape(&marks), "a b [Work: d c] e");
    // And the no-ops, inside a group as well as at the top level.
    assert!(!marks.move_to(Spot::in_group(2, 0), Spot::in_group(2, 0)));
    assert!(!marks.move_to(Spot::in_group(2, 0), Spot::in_group(2, 1)));
    assert_eq!(shape(&marks), "a b [Work: d c] e");
}

#[test]
fn a_group_moves_but_never_into_another_group() {
    let mut marks = nested();
    let other = marks.add_group("Play", true);
    assert_eq!(shape(&marks), "a b [Work: c d] e [Play:]");

    // To the front, with what is in it.
    assert!(marks.move_to(Spot::top(2), Spot::top(0)));
    assert_eq!(shape(&marks), "[Work: c d] a b e [Play:]");

    // And not inside the other one, however the pointer got there: one level is the whole
    // design, and a nested group could only ever come from here.
    assert!(!marks.move_to(Spot::top(0), Spot::in_group(4, 0)));
    assert_eq!(shape(&marks), "[Work: c d] a b e [Play:]");
    assert_eq!(other, 4, "the list is unchanged, so the group is still there");
}

#[test]
fn a_folder_is_pinned_once_wherever_it_is_pinned() {
    // `contains` is what `Ctrl+D` and the shell's `Pin to Quick access` ask, and a folder in a
    // group is a folder that is bookmarked.
    let mut marks = nested();
    assert!(marks.contains(&path("c")));
    assert!(!marks.add(path("c")), "already in a group");
    assert!(!marks.add_in(2, path("a")), "already at the top level");
    assert_eq!(shape(&marks), "a b [Work: c d] e");

    // And nothing is pinned twice by the list being built from a file with a repeat in it.
    let repeated: Bookmarks = ["a", "b", "a"].iter().map(|n| path(n)).collect();
    assert_eq!(shape(&repeated), "a b");

    // "This PC" is not a folder anybody can pin.
    let mut marks = Bookmarks::default();
    assert!(!marks.add(PathBuf::new()));
    assert!(marks.is_empty());
}

#[test]
fn removing_a_bookmark_finds_it_in_a_group() {
    let mut marks = nested();
    assert!(marks.remove(&path("c")));
    assert_eq!(shape(&marks), "a b [Work: d] e");
    assert!(marks.remove(&path("b")));
    assert_eq!(shape(&marks), "a [Work: d] e");
    assert!(!marks.remove(&path("b")), "gone already");
}

#[test]
fn a_group_can_be_taken_apart_or_taken_away() {
    // Two different wishes, and the reason there are two menu entries: ungrouping keeps the
    // bookmarks where the group was, and removing takes them with it. There is no undo here.
    let mut marks = nested();
    assert!(marks.ungroup(2));
    assert_eq!(shape(&marks), "a b c d e");

    let mut marks = nested();
    assert!(marks.remove_group(2));
    assert_eq!(shape(&marks), "a b e");

    // Neither of them on a row that is not a group, whatever a stale menu asks for.
    assert!(!marks.ungroup(0));
    assert!(!marks.remove_group(0));
    assert!(!marks.remove_group(9));
    assert_eq!(shape(&marks), "a b e");
}

#[test]
fn a_group_is_renamed_but_never_to_nothing() {
    let mut marks = nested();
    assert!(marks.rename_group(2, "  Client  "));
    assert_eq!(marks.group(2).expect("a group").name, "Client");
    // A blank name is a row you cannot read and cannot aim at, so the old one stands.
    assert!(!marks.rename_group(2, "   "));
    assert_eq!(marks.group(2).expect("a group").name, "Client");
    // One line, bounded, and control characters out: this ends up as one line of the settings
    // file, where a newline in a name would come back as two settings.
    assert!(marks.rename_group(2, "two\nlines"));
    assert_eq!(marks.group(2).expect("a group").name, "twolines");
    assert!(marks.rename_group(2, &"x".repeat(200)));
    assert_eq!(marks.group(2).expect("a group").name.len(), NAME_LIMIT);
    // And not a row that is not a group.
    assert!(!marks.rename_group(0, "nope"));
}

#[test]
fn folding_a_group_is_remembered_per_group() {
    let mut marks = nested();
    assert!(marks.group(2).expect("a group").open);
    assert!(marks.toggle_group(2));
    assert!(!marks.group(2).expect("a group").open);
    assert!(!marks.toggle_group(0), "not a group");
}

// ---------------------------------------------------------------------------
// Where the pointer says a row is going
// ---------------------------------------------------------------------------

/// The rows of [`nested`] as they would be drawn: `a b [Work] c d e`, 22 points each, with the
/// two in the group indented — and the group's line spanning all three of its rows, which is
/// what [`section`] writes back once it has drawn them.
fn lines() -> Vec<Line> {
    let mut out = Vec::new();
    let mut y = 0.0;
    let mut push = |spot: Spot, header: Option<usize>, y: &mut f32| {
        let rect = Rect::from_min_max(pos2(0.0, *y), pos2(200.0, *y + ROW));
        out.push(Line {
            rect,
            span: rect,
            spot,
            header,
        });
        *y += ROW;
    };
    push(Spot::top(0), None, &mut y);
    push(Spot::top(1), None, &mut y);
    push(Spot::top(2), Some(2), &mut y);
    push(Spot::in_group(2, 0), None, &mut y);
    push(Spot::in_group(2, 1), None, &mut y);
    push(Spot::top(3), None, &mut y);
    // The group is one block: its row and the two under it.
    out[2].span = Rect::from_min_max(out[2].rect.min, out[4].rect.max);
    out
}

/// The whole of the group in [`lines`] — its name and the two bookmarks in it.
fn block() -> Rect {
    lines()[2].span
}

/// The middle of the `n`th row, and a point in its top and bottom halves.
fn rows(n: usize) -> (egui::Pos2, egui::Pos2) {
    let top = n as f32 * ROW;
    (pos2(60.0, top + 4.0), pos2(60.0, top + ROW - 4.0))
}

#[test]
fn a_group_row_is_a_drop_target_rather_than_a_gap() {
    // Which is the only way to fill a group that is folded shut, and the reason the row lights
    // up instead of showing a caret beside itself.
    let lines = lines();
    for at in [rows(2).0, rows(2).1] {
        match landing(&lines, at, false, 4) {
            Some(Landing::Into { group, rect }) => {
                assert_eq!(group, 2);
                // And what lights up is the *group*, not the row the pointer is on: a highlight
                // round the name alone leaves the bookmarks it is about to join outside it.
                assert_eq!(rect, block());
            }
            other => panic!("a group's row has to accept a drop, got {other:?}"),
        }
    }
}

#[test]
fn a_caret_goes_before_or_after_the_row_the_pointer_is_in() {
    let lines = lines();
    let spot = |at: egui::Pos2| match landing(&lines, at, false, 4) {
        Some(Landing::Between { spot, .. }) => spot,
        other => panic!("expected a caret, got {other:?}"),
    };
    assert_eq!(spot(rows(0).0), Spot::top(0), "above the first row");
    assert_eq!(spot(rows(0).1), Spot::top(1), "below it");
    assert_eq!(spot(rows(1).0), Spot::top(1));
    // Inside the group, at the level the rows in it are on.
    assert_eq!(spot(rows(3).0), Spot::in_group(2, 0));
    assert_eq!(spot(rows(3).1), Spot::in_group(2, 1));
    assert_eq!(spot(rows(4).1), Spot::in_group(2, 2), "the end of the group");
    // And the caret is set in with the rows it is between.
    match landing(&lines, rows(3).0, false, 4) {
        Some(Landing::Between { edge, .. }) => assert_eq!(edge.left(), INDENT + CHILD),
        other => panic!("expected a caret, got {other:?}"),
    }
    // The row after the group is where a bookmark is aimed to land *after* the whole group, and
    // the line for it is on the far side of everything in it — which is that row's own top edge.
    match landing(&lines, rows(5).0, false, 4) {
        Some(Landing::Between { spot, edge }) => {
            assert_eq!(spot, Spot::top(3));
            assert_eq!(edge.center().y, block().bottom());
            assert_eq!(edge.left(), INDENT, "and it is not indented with the group");
        }
        other => panic!("expected a caret, got {other:?}"),
    }
}

#[test]
fn below_the_last_row_is_the_end_of_the_top_level() {
    // Without it, a list that ends in an open group has no reachable "put it at the bottom":
    // every row down there belongs to the group.
    let lines = lines();
    let mut inside_last = lines.clone();
    inside_last.pop();
    for rows in [lines, inside_last] {
        let below = pos2(60.0, 400.0);
        match landing(&rows, below, false, 4) {
            Some(Landing::Between { spot, .. }) => assert_eq!(spot, Spot::top(4)),
            other => panic!("a drop below every row goes to the end, got {other:?}"),
        }
    }
}

#[test]
fn a_group_being_dragged_sees_another_group_as_one_block() {
    // The rows inside a group are not places a group can go, so they are excluded from the
    // search — and what is left of the group is the whole of it. Which half of *the block* the
    // pointer is in is what decides, and the caret is drawn on the block's own edges: a line
    // under the group's name would be a line in the middle of its rows.
    let lines = lines();
    let block = block();
    let between = |at| match landing(&lines, at, true, 4) {
        Some(Landing::Between { spot, edge }) => (spot, edge.center().y),
        other => panic!("expected a caret at the top level, got {other:?}"),
    };

    // Anywhere in the top half of the block — its name, or the first row under it — is before it.
    for at in [rows(2).0, rows(2).1, rows(3).0] {
        let (spot, y) = between(at);
        assert_eq!(spot, Spot::top(2), "at {at:?}");
        assert_eq!(y, block.top(), "the caret is not on top of the block");
    }
    // And anywhere in the bottom half is after it, with the line below everything in it.
    for at in [rows(3).1, rows(4).0, rows(4).1] {
        let (spot, y) = between(at);
        assert_eq!(spot, Spot::top(3), "at {at:?}");
        assert_eq!(
            y,
            block.bottom(),
            "the caret for a drop after a group is drawn inside it"
        );
    }
    // Never inside it, wherever it is aimed.
    for at in [rows(2).0, rows(2).1, rows(3).0, rows(4).1] {
        assert!(
            between(at).0.group.is_none(),
            "a group cannot go inside a group, aimed at {at:?}"
        );
    }
}

#[test]
fn nothing_to_land_on_is_not_a_landing() {
    assert_eq!(landing(&[], pos2(60.0, 10.0), false, 0), None);
}
