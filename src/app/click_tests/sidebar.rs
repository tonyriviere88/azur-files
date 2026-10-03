//! The sidebar: places, drives, bookmarks, and the splitter beside them.

use super::*;

#[test]
fn a_sidebar_place_navigates() {
    let mut h = Harness::new();
    let at = h
        .find(Id::new(("place", "Home")), crate::ui::GUTTER + 80.0, 40..700)
        .expect("the Home row is not reachable by the pointer");
    let done = h.click_at(at);
    assert!(
        done.contains(&"Navigate"),
        "a sidebar place did not navigate, got {done:?}"
    );
}

#[test]
fn a_sidebar_group_header_folds_it() {
    let mut h = Harness::new();
    let before = h.app.sections.drives;
    let at = h
        .find(
            Id::new(("sidebar-group", "Drives")),
            crate::ui::GUTTER + 80.0,
            30..200,
        )
        .expect("the Drives heading is not reachable");
    h.click_at(at);
    assert_ne!(
        h.app.sections.drives, before,
        "a group heading has to fold its rows"
    );
}

#[test]
fn a_drive_row_navigates() {
    let mut h = Harness::new();
    let letter = h
        .app
        .volumes
        .all()
        .first()
        .map(|d| d.letter.clone())
        .expect("this machine has at least one volume");
    let at = h
        .find(Id::new(("drive", &letter)), crate::ui::GUTTER + 100.0, 40..300)
        .expect("no drive row is reachable by the pointer");
    let done = h.click_at(at);
    assert!(
        done.contains(&"Navigate"),
        "a drive row did not navigate, got {done:?}"
    );
}

#[test]
fn every_sidebar_place_is_reachable() {
    let mut h = Harness::new();
    let labels: Vec<String> = h.app.places.iter().map(|p| p.label.clone()).collect();
    for label in labels {
        let found = h.find(
            Id::new(("place", &label)),
            crate::ui::GUTTER + 80.0,
            30..760,
        );
        assert!(found.is_some(), "the `{label}` row is not reachable");
    }
}

#[test]
fn hovering_a_drive_puts_its_free_space_in_a_tooltip() {
    // The free-space numbers left the row and became a tooltip, so the tooltip is now
    // the only place they exist. Whether one actually appears is not something the
    // source shows: it needs a real hover, which is what this harness is for.
    let mut h = Harness::new();
    let letter = h
        .app
        .volumes
        .all()
        .first()
        .expect("a machine has a drive")
        .letter
        .clone();
    let at = h
        .find(Id::new(("drive", &letter)), crate::ui::GUTTER + 80.0, 30..300)
        .unwrap_or_else(|| panic!("the `{letter}` row is not reachable"));

    // A tooltip is an area in its own layer order, so this asks egui whether one is up
    // rather than hunting for the text.
    let shown = |h: &Harness| {
        h.ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .iter()
                .any(|layer| layer.order == egui::Order::Tooltip)
        })
    };
    assert!(!shown(&h), "a tooltip is up before anything was hovered");

    // Held still, not moved repeatedly: egui delays a tooltip until the pointer has
    // stopped, so a test that re-sends `PointerMoved` every frame resets the timer and
    // waits for ever. Move once, then let time pass.
    h.frame(vec![Event::PointerMoved(at)]);
    for _ in 0..20 {
        h.time += 0.25;
        h.frame(Vec::new());
        if shown(&h) {
            return;
        }
    }
    panic!("hovering the `{letter}` drive row shows no tooltip");
}

#[test]
fn a_folder_dropped_on_the_sidebar_is_bookmarked() {
    // The bookmarks group publishes itself as a drop zone, and a drop there pins
    // instead of copying. The zone has to *exist*: without it the drag reports "no"
    // to the pointer and the drop never happens at all.
    let mut h = Harness::new();
    let rect = h
        .app
        .bookmarks_rect
        .expect("the bookmarks group did not publish a drop zone");
    assert!(rect.height() > 0.0 && rect.width() > 0.0);

    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    h.app.perform(
        &h.ctx,
        Action::AddBookmark(here.join("src")),
    );
    assert!(
        h.app.bookmarks.contains(&here.join("src")),
        "pinning is what a drop on that zone performs"
    );
}

#[test]
fn bookmarks_reorder_to_where_they_are_dropped() {
    // That the *action* moves the list, which is a different question from the arithmetic it
    // does — `ui::sidebar::bookmarks::tests` has that one, over groups as well. Here for one
    // reason: an action nobody performs is a gesture that does nothing.
    let ctx = egui::Context::default();
    let mut app = App::opening(&ctx, Config::default(), Vec::new(), Side::Right);
    app.bookmarks = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
    let names = |app: &App| -> Vec<String> {
        app.bookmarks
            .paths()
            .map(|p| p.display().to_string())
            .collect()
    };

    use crate::ui::sidebar::Spot;
    // The first one to the end.
    app.perform(&ctx, Action::MoveBookmark { from: Spot::top(0), to: Spot::top(4) });
    assert_eq!(names(&app), ["b", "c", "d", "a"]);
    // The last one to the front.
    app.perform(&ctx, Action::MoveBookmark { from: Spot::top(3), to: Spot::top(0) });
    assert_eq!(names(&app), ["a", "b", "c", "d"]);
    // Nowhere: onto itself. A stale drag from a list that has since changed is the same kind
    // of nothing, and neither panics.
    app.perform(&ctx, Action::MoveBookmark { from: Spot::top(1), to: Spot::top(1) });
    app.perform(&ctx, Action::MoveBookmark { from: Spot::top(9), to: Spot::top(0) });
    assert_eq!(names(&app), ["a", "b", "c", "d"]);
}

/// The `+` beside the Bookmarks heading makes a group, and does not fold the section on the
/// way.
///
/// Both halves matter and only the second is a bug worth a test on its own: the button sits on
/// top of the heading's own click target, so a press that landed on the heading instead — or on
/// both — would create the group and immediately hide it.
#[test]
fn the_plus_on_the_bookmarks_heading_makes_a_group() {
    let mut h = Harness::new();
    let open = h.app.sections.bookmarks;
    assert!(open, "the section starts open");

    let at = h
        .find(
            Id::new("bookmark-group-add"),
            h.app.sidebar_width - 12.0,
            30..200,
        )
        .expect("the + on the Bookmarks heading is not reachable by the pointer");
    let done = h.click_at(at);
    assert!(
        done.contains(&"AddBookmarkGroup"),
        "the + did not make a group, got {done:?}"
    );
    assert_eq!(h.app.bookmarks.len_of(None), 1);
    assert!(
        h.app.sections.bookmarks,
        "the + folded the section it was making a group in"
    );
    // And the group opens with its name in a field, because being made and being named are one
    // gesture — see `Action::AddBookmarkGroup`.
    let naming = h.app.bookmark_edit.rename.as_ref().expect("a field is open");
    assert_eq!(naming.group, 0);
}

/// The `+` is a hover control, and it stands down for a drag.
///
/// Both halves are the point of it. A permanent button on the heading is permanent furniture in
/// a panel whose job is to be a list of names; and during a drop it would be a button under the
/// pointer, in the middle of a highlight saying something else is about to happen, that the
/// pointer cannot press. `read_response` is the question that matters — whether the widget
/// exists this frame at all.
#[test]
fn the_plus_is_a_hover_control_that_stands_down_for_a_drag() {
    let mut h = Harness::new();
    h.settle();
    let add = Id::new("bookmark-group-add");
    let section = h.app.bookmarks_rect.expect("the section is on screen");
    // Two frames per answer, because `read_response` falls back to the pass before this one: a
    // widget that has just stopped being drawn still answers for one more frame.
    let drawn = |h: &mut Harness| {
        h.frame(Vec::new());
        h.ctx.read_response(add).is_some()
    };

    // The pointer out in the listing: no button anywhere.
    h.frame(vec![Event::PointerMoved(pos2(600.0, 400.0))]);
    assert!(
        !drawn(&mut h),
        "the + is on the heading with the pointer nowhere near it"
    );

    // In the section — anywhere in it, not only on the heading.
    h.frame(vec![Event::PointerMoved(section.center())]);
    assert!(
        drawn(&mut h),
        "the + does not appear with the pointer in the section"
    );

    // And a drag from a listing takes it away again, with the pointer where it was. Put over the
    // window the way OLE puts one there — `drop_hover` is refreshed from that at the top of
    // every frame, so setting the field would not survive to the frame it was set for.
    let scale = h.ctx.pixels_per_point();
    let at = section.center();
    h.app
        .drops
        .hover(Some(((at.x * scale) as i32, (at.y * scale) as i32)));
    h.frame(Vec::new());
    assert!(
        h.app.drop_hover.is_some(),
        "the harness failed to put a drag over the window"
    );
    assert!(
        !drawn(&mut h),
        "the + is still up while a folder is being dragged into the section"
    );
    h.app.drops.hover(None);
    assert!(drawn(&mut h), "and comes back after");
}

/// A group is its own drop zone — the whole of it — and it wins over the section it sits in.
///
/// The zones the application publishes are what the OLE callbacks answer from — see
/// `transfer::a_folder_row_is_its_own_drop_target`, which is the same claim about a folder row
/// in a listing. Dragging a folder onto a group has to mean *into that group*, the highlight has
/// to say so rather than lighting up the whole section, and both have to be about the group and
/// not about the row the pointer happens to be on: a bookmark under the name is part of the
/// group, so a drop there means the same thing as a drop on the name.
#[test]
#[cfg(windows)]
fn a_group_is_its_own_drop_zone_rows_and_all() {
    use crate::shell::dnd::Onto;

    let mut h = Harness::new();
    let inside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(&h.ctx, Action::AddBookmarkGroup);
    h.app.perform(
        &h.ctx,
        Action::AddBookmarkIn {
            group: 0,
            path: inside.clone(),
        },
    );
    h.app.bookmark_edit.rename = None;
    h.settle();

    let (block, group) = *h
        .app
        .bookmark_rows
        .first()
        .expect("the group published nothing at all, so nothing can be dropped onto it");
    assert_eq!(group, 0);
    assert!(
        block.height() > crate::ui::sidebar::GAUGE_HEIGHT + 30.0,
        "the group's zone is {} tall, which is its name and not its rows",
        block.height()
    );

    h.app.publish_drop_targets(&h.ctx.clone());
    let scale = h.ctx.pixels_per_point();
    let physical = |at: Pos2| ((at.x * scale) as i32, (at.y * scale) as i32);
    // Two places in it: the group's own row, and the bookmark under it.
    for at in [
        pos2(block.center().x, block.top() + 2.0),
        pos2(block.center().x, block.bottom() - 2.0),
    ] {
        let resolved = h
            .app
            .drops
            .resolve(physical(at))
            .unwrap_or_else(|| panic!("no zone at {at:?}"));
        assert_eq!(
            resolved,
            Onto::BookmarkGroup(0),
            "{at:?} in the group resolved to {resolved:?}"
        );
        // And the highlight is the whole group, wherever in it the pointer is — a picture of
        // what is being joined rather than of the row it was aimed at.
        h.app.drop_hover = Some(physical(at));
        assert_eq!(h.app.bookmarks_preview(scale), Some(block));
    }

    // The heading above it is still the section's own zone, which pins at the end.
    let section = h.app.bookmarks_rect.expect("the section is on screen");
    assert_eq!(
        h.app
            .drops
            .resolve(physical(pos2(section.center().x, section.top() + 2.0))),
        Some(Onto::Bookmarks),
        "away from a group, a drop still means the list itself"
    );
}

/// Typing over that field names the group, and Escape leaves the name alone.
#[test]
fn a_group_is_named_by_typing_over_the_field() {
    let mut h = Harness::new();
    let at = h
        .find(
            Id::new("bookmark-group-add"),
            h.app.sidebar_width - 12.0,
            30..200,
        )
        .expect("the + is not reachable");
    h.click_at(at);

    // The field takes the keyboard on the frame it appears in, so this goes straight into it —
    // which is the whole point of it opening focused.
    h.frame(vec![Event::Text("Work".to_owned())]);
    h.frame(super::tap(egui::Key::Enter));
    h.frame(Vec::new());
    assert_eq!(
        h.app.bookmarks.group(0).expect("the group").name,
        "Work",
        "typing over the field did not name the group"
    );
    assert!(
        h.app.bookmark_edit.rename.is_none(),
        "the field is still open after Enter"
    );

    // And a name abandoned is the name it had: Escape puts the row back rather than blanking it.
    h.app.perform(&h.ctx, Action::BeginRenameBookmarkGroup(0));
    h.frame(Vec::new());
    h.frame(vec![Event::Text("Play".to_owned())]);
    h.frame(super::tap(egui::Key::Escape));
    h.frame(Vec::new());
    assert_eq!(h.app.bookmarks.group(0).expect("the group").name, "Work");
    assert!(h.app.bookmark_edit.rename.is_none());
}

/// The name does not move when the field opens over it.
///
/// The one thing about an in-place editor that is invisible in the source and obvious on screen:
/// a field is a box with its own padding and its own idea of where a line sits in it, so the
/// word being edited flinches a point or two the instant it becomes editable. Asked of the
/// *baseline*, because that is the question the eye asks — a galley's box says nothing about
/// whether two runs of text are level.
#[test]
fn a_group_name_stays_put_when_it_becomes_a_field() {
    let mut h = Harness::new();
    h.app.perform(&h.ctx, Action::AddBookmarkGroup);
    h.app.perform(
        &h.ctx,
        Action::CommitRenameBookmarkGroup {
            group: 0,
            name: "Work".to_owned(),
        },
    );
    h.settle();

    let where_is = |h: &Harness| -> Pos2 {
        h.baselines()
            .into_iter()
            .find(|(at, text)| text == "Work" && at.x < h.app.sidebar_width)
            .map(|(at, _)| at)
            .expect("the group's name is not on screen")
    };
    let label = where_is(&h);

    h.app.perform(&h.ctx, Action::BeginRenameBookmarkGroup(0));
    // Two frames: the field appears in the first and takes the keyboard in it, which is when
    // egui lays its galley out.
    h.frame(Vec::new());
    h.frame(Vec::new());
    let field = where_is(&h);

    assert!(
        (field.x - label.x).abs() < 0.6 && (field.y - label.y).abs() < 0.6,
        "the name moved from {label:?} to {field:?} when the field opened"
    );
}

/// A group's row folds it, and the bookmark inside it is a row of its own that navigates.
///
/// A group is not a place, so the click that would go somewhere on any other row here has to
/// fold this one instead — and the rows it hides have to be reachable when it is open, indent
/// and all.
#[test]
fn a_group_folds_and_the_bookmark_in_it_navigates() {
    let mut h = Harness::new();
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let inside = here.join("src");
    h.app.perform(&h.ctx, Action::AddBookmarkGroup);
    h.app.perform(
        &h.ctx,
        Action::AddBookmarkIn {
            group: 0,
            path: inside.clone(),
        },
    );
    h.app.bookmark_edit.rename = None;
    h.settle();

    // The row inside it, at the child indent — found by asking rather than by working out where
    // the indent put it.
    let child = h
        .find(
            Id::new(("bookmark", inside.as_path())),
            crate::ui::GUTTER + 90.0,
            30..300,
        )
        .expect("the bookmark inside the group is not reachable");
    let done = h.click_at(child);
    assert!(
        done.contains(&"Navigate"),
        "a bookmark in a group did not navigate, got {done:?}"
    );

    let group = h
        .find(Id::new(("bookmark-group", 0)), crate::ui::GUTTER + 60.0, 30..300)
        .expect("the group's own row is not reachable");
    let done = h.click_at(group);
    assert!(
        done.contains(&"ToggleBookmarkGroup"),
        "a group's row has to fold it rather than go anywhere, got {done:?}"
    );
    assert!(!h.app.bookmarks.group(0).expect("the group").open);
    // Folded, the row it held is gone from the panel.
    assert!(
        h.find(
            Id::new(("bookmark", inside.as_path())),
            crate::ui::GUTTER + 90.0,
            30..300
        )
        .is_none(),
        "a folded group still shows what is in it"
    );
}

/// Dragging a bookmark onto a group's row puts it in the group.
///
/// The gesture, driven for real, and the one part of this feature that reading the source
/// cannot establish: a drag is three states over three frames, and a drop resolved on the frame
/// the button comes up is the difference between a list that rearranges and one that does
/// nothing at all.
#[test]
fn a_bookmark_dragged_onto_a_group_goes_into_it() {
    let mut h = Harness::new();
    let inside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(&h.ctx, Action::AddBookmark(inside.clone()));
    h.app.perform(&h.ctx, Action::AddBookmarkGroup);
    h.app.perform(
        &h.ctx,
        Action::CommitRenameBookmarkGroup {
            group: 1,
            name: "Work".to_owned(),
        },
    );
    h.settle();

    let x = crate::ui::GUTTER + 70.0;
    let mark = h
        .find(Id::new(("bookmark", inside.as_path())), x, 30..300)
        .expect("the bookmark row is not reachable");
    let group = h
        .find(Id::new(("bookmark-group", 1)), x, 30..300)
        .expect("the group's row is not reachable");

    let done = h.drag(mark, group);
    assert!(
        done.contains(&"MoveBookmark"),
        "dragging a bookmark onto a group did nothing, got {done:?}"
    );
    assert_eq!(
        h.app.bookmarks.len_of(None),
        1,
        "the bookmark is still at the top level"
    );
    assert_eq!(
        h.app.bookmarks.group(0).expect("the group").marks,
        [inside],
        "the drop did not land in the group"
    );
}

/// Press, travel, and stop there with the button still down.
///
/// The harness's own `drag` finishes the gesture; what these two want is the frame in the middle
/// of one, which is the only frame the mark for it is painted in.
fn hold(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        h.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
    }
    h.frame(vec![Event::PointerMoved(to)]);
}

/// Two groups, the second holding a bookmark, and where each of them was drawn.
fn two_groups(h: &mut Harness) -> ((Rect, usize), (Rect, usize)) {
    let inside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for _ in 0..2 {
        h.app.perform(&h.ctx, Action::AddBookmarkGroup);
    }
    h.app.perform(
        &h.ctx,
        Action::AddBookmarkIn {
            group: 1,
            path: inside,
        },
    );
    h.app.bookmark_edit.rename = None;
    h.settle();
    let rows = h.app.bookmark_rows.clone();
    assert_eq!(rows.len(), 2, "two groups, two blocks: {rows:?}");
    (rows[0], rows[1])
}

/// Dragging a bookmark onto a group lights up **the whole group**, not the row it was aimed at.
///
/// A group is one thing, and the highlight is a picture of what the bookmark is about to join. A
/// box round the name alone left the bookmarks in it outside the box.
#[test]
fn the_highlight_for_a_drop_into_a_group_covers_its_rows() {
    let mut h = Harness::new();
    let (_, filled) = two_groups(&mut h);
    let (block, group) = filled;
    assert_eq!(group, 1);
    assert!(
        block.height() > 30.0,
        "the group's block is {} tall, which is its name and not its rows",
        block.height()
    );

    // The empty group's own row is what gets picked up — a group is draggable too, but this is
    // about a *bookmark* going in, so drag the one inside the other group out of it and back.
    let inside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mark = h
        .find(
            Id::new(("bookmark", inside.as_path())),
            crate::ui::GUTTER + 90.0,
            30..320,
        )
        .expect("the bookmark inside the group is not reachable");
    hold(&mut h, mark, pos2(block.center().x, block.top() + 2.0));

    // The wash and the outline both, over exactly the block. `fill_at` is no use here: a drop
    // target is a fill *and* a stroke at the same rect, and the stroke is the later of the two.
    let accent = h.app.theme.accent.default;
    let wash = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 40);
    let same = |rect: &Rect| rect.min.distance(block.min) < 0.5 && rect.max.distance(block.max) < 0.5;
    assert!(
        h.rects()
            .iter()
            .any(|(rect, _, fill)| same(rect) && *fill == wash),
        "the group is not washed over at {block:?}; fills were {:?}",
        h.rects()
            .into_iter()
            .filter(|(rect, _, _)| rect.left() < 200.0)
            .collect::<Vec<_>>()
    );
    assert!(
        h.outlines()
            .iter()
            .any(|(rect, color)| same(rect) && *color == accent),
        "and it has no accent outline round it: {:?}",
        h.outlines()
    );
}

/// The caret for a drop *after* a group is drawn below everything in it.
///
/// Under the group's name is inside the group — a line there reads as landing between two of its
/// bookmarks, which is not what the drop would do.
#[test]
fn the_caret_after_a_group_is_drawn_below_its_rows() {
    let mut h = Harness::new();
    let (first, second) = two_groups(&mut h);

    // The first group, dragged down into the bottom half of the second — which for a group being
    // dragged means after the whole of it, because a group holds no groups.
    let from = h
        .find(
            Id::new(("bookmark-group", first.1)),
            crate::ui::GUTTER + 60.0,
            30..320,
        )
        .expect("the first group's row is not reachable");
    let block = second.0;
    hold(&mut h, from, pos2(block.center().x, block.bottom() - 2.0));

    let carets: Vec<Rect> = h
        .rects()
        .into_iter()
        .map(|(rect, _, _)| rect)
        .filter(|rect| rect.left() < 200.0 && (rect.height() - 2.0).abs() < 0.5)
        .collect();
    assert!(
        carets
            .iter()
            .any(|rect| (rect.center().y - block.bottom()).abs() < 0.6),
        "no caret along the bottom of the group at {}: got {carets:?}",
        block.bottom()
    );
}

/// Double-clicking the sidebar splitter puts it back where it started.
///
/// The gesture the column edges in the listing already answer to, and the way out of a
/// sidebar dragged somewhere silly. Driven through the real splitter rather than by calling
/// something, because what is easy to get wrong here is the grip's rect and its `Sense`: a
/// `drag`-only splitter reports no clicks at all, and the reset would be dead code.
#[test]
fn double_clicking_the_sidebar_splitter_restores_its_width() {
    let mut h = Harness::new();
    h.settle();

    let default = crate::config::SIDEBAR_WIDTH;
    assert_eq!(
        h.app.sidebar_width, default,
        "the harness did not start at the default width"
    );

    // Somewhere silly, the way a drag would leave it.
    h.app.sidebar_width = 380.0;
    h.frame(Vec::new());
    let dragged = h.app.sidebar_width;
    assert_eq!(dragged, 380.0);

    // Found by asking the splitter itself, rather than by re-deriving where the layout put
    // it: a test that computes the grip's x is a test of this test's arithmetic.
    let grip = egui::Id::new("sidebar-grip");
    let at = (0..40)
        .map(|step| pos2(dragged - 8.0 + step as f32 * 0.5, 300.0))
        .find(|&at| h.hovers(grip, at))
        .expect("the sidebar splitter is not reachable by the pointer at all");
    h.double_click_at(at);
    assert_eq!(
        h.app.sidebar_width, default,
        "double-clicking the splitter at {at:?} left the sidebar at {}",
        h.app.sidebar_width
    );
    // Not `config_dirty`: the frame after the one that sets it writes the settings and
    // clears it again, so by the time this can look it is already false — which is the flag
    // doing its job rather than a fault. That it *was* set is covered by the settings file
    // holding `sidebar_width=200` after a real window is dragged wider and double-clicked
    // back, which is a thing to check with a real window and not from here.
}

/// The Bookmarks group lights up for a drag, and only over itself.
///
/// Dropping a folder there pins it, which is a real destination and needs to look like one.
#[test]
fn the_bookmarks_group_shows_a_drop_target() {
    let mut h = Harness::new();
    h.settle();
    let rect = h
        .app
        .bookmarks_rect
        .expect("the Bookmarks group is on screen");
    let scale = h.ctx.pixels_per_point();
    let put = |h: &mut Harness, at: egui::Pos2| {
        h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
    };

    put(&mut h, rect.center());
    assert_eq!(h.app.bookmarks_preview(scale), Some(rect));

    // Below the group, in Places: pinning is not what a drop there would mean.
    put(&mut h, egui::pos2(rect.center().x, rect.bottom() + 40.0));
    assert_eq!(h.app.bookmarks_preview(scale), None);

    h.app.drop_hover = None;
    assert_eq!(h.app.bookmarks_preview(scale), None);
}
