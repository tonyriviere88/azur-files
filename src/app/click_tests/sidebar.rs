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
    // Insertion-point arithmetic, which is where an off-by-one is invisible until a
    // list quietly reverses itself.
    let ctx = egui::Context::default();
    let mut app = App::opening(&ctx, Config::default(), Vec::new(), Side::Right);
    app.bookmarks = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
    let names = |app: &App| -> Vec<String> {
        app.bookmarks
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    };

    // The first one to the end.
    app.perform(&ctx, Action::MoveBookmark { from: 0, to: 4 });
    assert_eq!(names(&app), ["b", "c", "d", "a"]);
    // The last one to the front.
    app.perform(&ctx, Action::MoveBookmark { from: 3, to: 0 });
    assert_eq!(names(&app), ["a", "b", "c", "d"]);
    // One place down: `to` is an insertion point in the list as it was, so 2 means
    // "before what is currently at 2".
    app.perform(&ctx, Action::MoveBookmark { from: 0, to: 2 });
    assert_eq!(names(&app), ["b", "a", "c", "d"]);
    // Nowhere, twice: onto itself, and just after itself.
    app.perform(&ctx, Action::MoveBookmark { from: 1, to: 1 });
    app.perform(&ctx, Action::MoveBookmark { from: 1, to: 2 });
    assert_eq!(names(&app), ["b", "a", "c", "d"]);
    // Out of range, from a stale drag: nothing moves and nothing panics.
    app.perform(&ctx, Action::MoveBookmark { from: 9, to: 0 });
    app.perform(&ctx, Action::MoveBookmark { from: 0, to: 9 });
    assert_eq!(names(&app), ["b", "a", "c", "d"]);
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
