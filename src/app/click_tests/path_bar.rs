//! The bar above a listing: history, Up, Refresh, and the breadcrumb's own buttons.

use super::*;

#[test]
fn the_history_buttons_respond() {
    let mut h = Harness::new();
    let y = h.path_bar_y(0);
    let pane = h.app.panes[0].id;
    let id = Id::new(("nav", pane, "Up (Alt+Up)"));
    let left = h.pane_rect(0).left();
    let at = (0..120)
        .step_by(2)
        .map(|dx| pos2(left + dx as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("the Up button is not reachable by the pointer");
    let done = h.click_at(at);
    assert!(done.contains(&"Up"), "Up did not respond, got {done:?}");
}

/// **A right click on Back opens the whole history, and an entry in it takes you there.**
///
/// One menu for both directions: the place ahead above, where you are ticked in the middle, the
/// place behind below, each by its folder name alone. The arrows are `text-secondary` at rest and
/// `accent.mark` on the entry under the pointer — asserted on the painted ink, because the colour
/// is chosen after `MenuItem` has drawn and nothing else would see it go wrong.
#[test]
fn the_back_buttons_menu_lists_the_history_both_ways() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.panes[0].tab_mut().navigate(root.join("src"));
    h.app.panes[0].tab_mut().navigate(root.join("src").join("ui"));
    h.app.panes[0].tab_mut().go_back();
    h.settle();

    let button = h
        .ctx
        .read_response(Id::new(("nav", pane, "Back (Alt+Left)")))
        .map(|r| r.rect)
        .expect("the Back button was not laid out");
    h.click_with(button.center(), PointerButton::Secondary, Modifiers::NONE);
    // Past the menu's fade-in, or every ink below is read part way to its colour.
    h.wait();

    // The *last* of each name painted, because the menu is the top layer and the breadcrumb and
    // the listing behind it spell the same names.
    let entry = |h: &Harness, label: &str| {
        h.texts()
            .into_iter()
            .rev()
            .find(|(_, text)| text == label)
            .map(|(at, _)| at)
            .unwrap_or_else(|| {
                panic!(
                    "the menu has no `{label}` in it: {:?}",
                    h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
                )
            })
    };
    let ahead = entry(&h, "ui");
    let here = entry(&h, "src");
    let behind = entry(&h, "azur-files");
    assert!(
        ahead.y < here.y && here.y < behind.y,
        "the history is not in the order it was walked: {ahead:?} {here:?} {behind:?}"
    );

    // The arrow sits in the slot left of the label, on the label's line. It is stroked rather
    // than filled, so its colour is read off the shaft — the one straight segment it draws.
    let ink = |h: &Harness, at: Pos2| {
        let slot = Rect::from_min_max(pos2(at.x - 30.0, at.y - 4.0), pos2(at.x, at.y + 20.0));
        h.segments()
            .into_iter()
            .filter(|(points, _)| points.iter().all(|p| slot.contains(*p)))
            .map(|(_, color)| color)
            .collect::<Vec<_>>()
    };
    let (rest, hover) = (h.app.theme.text.secondary, h.app.theme.accent.mark);
    assert_eq!(ink(&h, behind), [rest], "the arrow at rest is not text-secondary");
    assert!(ink(&h, here).is_empty(), "where you are has an arrow as well as its tick");
    h.frame(vec![Event::PointerMoved(pos2(behind.x + 2.0, behind.y + 6.0))]);
    h.wait();
    assert_eq!(ink(&h, behind), [hover], "the hovered entry's arrow is not accent.mark");
    assert_eq!(ink(&h, ahead), [rest], "an entry the pointer is not on went blue");

    let done = h.click_at(pos2(behind.x + 2.0, behind.y + 6.0));
    assert!(done.contains(&"GoToHistory"), "the entry did nothing, got {done:?}");
    assert_eq!(h.tab(0).path, root);
    assert_eq!(h.tab(0).history.len(), 3, "a jump back dropped the places ahead");
}

/// Refresh is in the group at the left, and the star that was beside the filter is gone.
///
/// Both halves matter. Refresh moved *into* the never-dropped group, so it is found by
/// scanning from the pane's left edge and its id is the group's — a test still looking for
/// `("refresh", pane)` out on the right would pass on a bar that had lost the button
/// entirely. And a button removed from a toolbar has to leave its *function* reachable, or
/// "we tidied the bar" means "we deleted the feature": `Ctrl+D` is checked here for that
/// reason, not for the shortcut's own sake.
#[test]
fn refresh_is_beside_up_and_the_bookmark_star_is_gone() {
    let mut h = Harness::new();
    let y = h.path_bar_y(0);
    let pane = h.app.panes[0].id;
    let left = h.pane_rect(0).left();

    let id = Id::new(("nav", pane, "Refresh (F5)"));
    let at = (0..140)
        .step_by(2)
        .map(|dx| pos2(left + dx as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("Refresh is not reachable from the left-hand group");
    let done = h.click_at(at);
    assert!(done.contains(&"Refresh"), "Refresh did not fire, got {done:?}");

    // Immediately after Up: the buttons touch, so this is a claim about the two rects and not
    // about wherever the sweep above happened to land.
    let rect_of = |h: &Harness, tip: &str| {
        h.ctx
            .read_response(Id::new(("nav", pane, tip)))
            .map(|r| r.rect)
            .unwrap_or_else(|| panic!("the {tip} button was not laid out"))
    };
    let up = rect_of(&h, "Up (Alt+Up)");
    let refresh = rect_of(&h, "Refresh (F5)");
    assert_eq!(
        refresh.left(),
        up.right(),
        "Refresh starts at {} and Up ends at {}",
        refresh.left(),
        up.right()
    );

    h.frame(Vec::new());
    assert!(
        h.ctx
            .read_response(Id::new(("bookmark-toggle", pane)))
            .is_none(),
        "the bookmark star is still on the bar"
    );

    // And the thing it used to do is still done. The modifiers go on the harness as well as
    // on the event, because the code under test reads `InputState::modifiers`.
    h.take_journal();
    h.modifiers = Modifiers::COMMAND;
    h.frame(vec![Event::Key {
        key: egui::Key::D,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    let done = h.take_journal();
    assert!(
        done.contains(&"ToggleBookmark"),
        "Ctrl+D no longer bookmarks, so removing the star removed the feature: {done:?}"
    );
}

#[test]
#[ignore = "diagnostic; run explicitly"]
fn where_are_the_crumbs() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
    println!(
        "path={:?}\ntrail={:?}\nactive={} of {}",
        h.tab(0).path,
        h.tab(0).trail,
        crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path),
        crumbs.len()
    );
    let y = h.path_bar_y(0);
    h.frame(vec![Event::PointerMoved(pos2(h.pane_rect(0).left() + 40.0, y))]);
    for (index, (label, _)) in crumbs.iter().enumerate() {
        let rect = h.ctx.read_response(Id::new(("crumb", pane, index))).map(|r| r.rect);
        println!("  {index} {label:<32} {rect:?}");
    }
    println!(
        "  overflow {:?}\n  pane {:?}  bar_y {y}",
        h.ctx
            .read_response(Id::new(("crumb-overflow", pane)))
            .map(|r| r.rect),
        h.pane_rect(0)
    );
}

#[test]
fn a_breadcrumb_segment_navigates() {
    let mut h = Harness::new();
    let y = h.path_bar_y(0);
    let pane = h.app.panes[0].id;

    // The parent of the folder being shown, found rather than assumed: which segments
    // are on the bar depends on how much of the path fits, and the leading ones collapse
    // into the `…`. This one is next to the current folder, so it is there whenever
    // anything is. (It used to look for segment 0, This PC, on the grounds that it is
    // always present — which stopped being true the day the path bar got 27px narrower.)
    let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
    let parent = crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path) - 1;
    let id = Id::new(("crumb", pane, parent));
    let left = h.pane_rect(0).left();
    let at = (0..900)
        .step_by(2)
        .map(|dx| pos2(left + dx as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("the parent breadcrumb segment is not reachable");

    let done = h.click_at(at);
    assert!(
        done.contains(&"Navigate"),
        "a breadcrumb segment has to navigate, got {done:?}"
    );
    assert_eq!(
        h.tab(0).path,
        crumbs[parent].1,
        "and it has to go to the folder it names"
    );
}

#[test]
fn after_going_up_the_folder_left_is_still_on_the_breadcrumb() {
    // The whole point of `Tab::trail`: walk up, and the folder just left is a segment
    // you can click rather than a name to go hunting for in a chevron menu.
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let was = h.tab(0).path.clone();

    h.app.panes[0].tab_mut().go_up();
    h.settle();
    assert_eq!(h.tab(0).trail, was, "the trail has to outlive the move");

    // The segment for the folder we came out of is the last one on the bar, past the
    // one now in bold.
    let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
    let deepest = crumbs.len() - 1;
    assert_eq!(crumbs[deepest].1, was);
    assert!(
        crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path) < deepest,
        "the bold segment should be an ancestor of the end of the trail"
    );

    // And it is reachable by the pointer and navigates -- the two things a segment
    // drawn in the right place can still fail to do.
    let y = h.path_bar_y(0);
    let left = h.pane_rect(0).left();
    let at = (0..900)
        .step_by(2)
        .map(|dx| pos2(left + dx as f32, y))
        .find(|at| h.hovers(Id::new(("crumb", pane, deepest)), *at))
        .expect("the segment past the current folder cannot be reached");
    let done = h.click_at(at);
    assert!(
        done.contains(&"Navigate"),
        "clicking back down the trail has to navigate, got {done:?}"
    );
    assert_eq!(h.tab(0).path, was, "and it goes back where it came from");
}

/// The path bar is the selected tab's colour, and the tab is the same colour it is.
///
/// Both from one frame, because the point of the change is that they *match*: a test that
/// checked the bar alone would pass just as well if the tab drifted, and the two are painted
/// by different modules.
#[test]
fn the_path_bar_and_the_selected_tab_are_one_surface() {
    let mut h = Harness::new();
    h.settle();

    let seam = crate::ui::seam(&h.app.theme);
    // The pane's own rect, not an inset of it: the bar reaches the seams on both sides.
    let pane = h.app.pane_rects[0].1;
    let bar = Rect::from_min_size(
        pane.min,
        egui::vec2(pane.width(), crate::ui::breadcrumb::HEIGHT),
    );

    let (corner, fill) = h
        .fill_at(bar)
        .expect("nothing was painted at the path bar's rect");
    assert_eq!(fill, seam, "the path bar is not the selected tab's colour");
    assert_eq!(corner, egui::CornerRadius::ZERO);

    // The tab above it, found among the slots this frame resolved rather than derived.
    let tab = h
        .app
        .tab_slots
        .iter()
        .find(|slot| slot.pane == h.app.focused && slot.tab == 0)
        .map(|slot| slot.rect)
        .expect("the focused pane's tab was not laid out");
    // Its fill reaches a point past its own rect, over the strip's bottom hairline. That
    // overhang *is* the weld — with the tab stopping at its rect, a line of `stroke-subtle`
    // ran between two surfaces of the same colour — so the test looks for the welded rect
    // and would fail if the tab went back to painting only itself.
    let welded = Rect::from_min_max(tab.min, pos2(tab.max.x, tab.max.y + 1.0));
    let filled = h
        .rects()
        .into_iter()
        .rev()
        .find(|(rect, _, _)| {
            rect.min.distance(welded.min) < 0.5 && rect.max.distance(welded.max) < 0.5
        })
        .expect("the focused pane's tab does not reach over the strip's bottom edge");
    assert_eq!(
        filled.2, seam,
        "the focused pane's selected tab is not the colour its path bar is"
    );
}
