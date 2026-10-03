//! Panes and the dock: focusing one, splitting by dragging a tab, and resizing the seams.

use super::*;

#[test]
fn clicking_a_pane_focuses_it() {
    let mut h = Harness::with_panes(2);
    assert_eq!(h.app.panes.len(), 2);
    let second = h.app.panes[1].id;
    h.app.focused = h.app.panes[0].id;

    h.click_at(h.row_center(1, 0));
    assert_eq!(
        h.app.focused, second,
        "clicking in a pane has to move the keyboard there"
    );
}

#[test]
fn dragging_a_tab_onto_a_pane_edge_splits_it() {
    let mut h = Harness::new();
    h.app.panes[0]
        .tabs
        .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
    h.settle();

    let pane_id = h.app.panes[0].id;
    let strip_y = crate::ui::chrome::HEIGHT * 0.5;
    let from = (0..500)
        .step_by(2)
        .map(|dx| pos2(dx as f32, strip_y))
        .find(|at| h.hovers(Id::new(("tab", pane_id, 1usize)), *at))
        .expect("the second tab is not reachable");

    let pane = h.pane_rect(0);
    let to = pos2(pane.right() - 12.0, pane.center().y);
    let done = h.drag(from, to);

    assert!(
        done.contains(&"BeginTabDrag"),
        "the drag never started, got {done:?}"
    );
    assert!(
        done.contains(&"SplitTab"),
        "dropping a tab on a pane's right edge has to split it, got {done:?}"
    );
    assert_eq!(h.app.layout.count(), 2);
}

/// **While a tab is in the air, everywhere else it could land is on screen too.**
///
/// The reason the grey exists: the docking gesture used to be invisible until the pointer
/// happened to be in the right place, which made it something you had to be told about. What is
/// pinned here is the wiring and the two rules that keep the two colours meaning two things —
/// the hints are at the rects `dock` calls the sense areas, they are the grey and not the
/// accent, and the pane the pointer is actually over wears the accent alone.
#[test]
fn dragging_a_tab_shows_where_else_it_could_go() {
    use crate::dock::{hint_rect, Zone};

    let mut h = Harness::with_panes(2);
    // A second tab in the pane the drag starts from, so that pane's own edges are somewhere the
    // tab could go: splitting a pane's *only* tab away from itself would leave an empty pane
    // behind, and that zone is a no-op with nothing to hint at.
    h.app.panes[0]
        .tabs
        .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
    h.settle();

    let source_id = h.app.panes[0].id;
    let slot = h
        .app
        .tab_slots
        .iter()
        .find(|s| s.pane == source_id && s.tab == 1)
        .map(|s| s.rect)
        .expect("the second tab of the first pane is not on screen");
    let source = h.pane_rect(0);
    let over = h.pane_rect(1);

    let done = h.drag_and_hold(slot.center(), over.center());
    assert!(
        done.contains(&"BeginTabDrag"),
        "the drag never started, got {done:?}"
    );

    // The marks' own wash, asked for by name rather than recomputed: what this pins is that the
    // marks are that grey and not the accent — the colour that means "release now and it lands
    // here" — rather than what the figure behind the grey happens to be.
    let wash = crate::ui::hint_wash(&h.app.theme);
    let painted = h.rects();
    let hinted = |rect: Rect| {
        painted
            .iter()
            .filter(|(at, _, _)| at.min.distance(rect.min) < 0.5 && at.max.distance(rect.max) < 0.5)
            .map(|(_, _, fill)| *fill)
            .collect::<Vec<_>>()
    };

    for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
        let zone = Zone::Split(side);
        assert!(
            hinted(hint_rect(source, zone)).contains(&wash),
            "nothing grey where the tab could split its own pane, {zone:?}"
        );
    }
    // Not its middle, though: moving a tab into the strip it is already in does nothing, and a
    // hint over a gesture that does nothing is worse than no hint.
    assert!(
        !hinted(hint_rect(source, Zone::Into)).contains(&wash),
        "the pane the tab came from is offering to take it back"
    );
    // **And the pane being docked into keeps its hints too.** They are what a reader is aiming
    // with: clearing them the moment a target is reached takes the other four places away
    // exactly while the pointer is moving between them.
    for zone in Zone::ALL {
        assert!(
            hinted(hint_rect(over, zone)).contains(&wash),
            "the pane under the pointer lost its hints, {zone:?}"
        );
    }

    // **Each mark draws what dropping on it does**: a dashed rectangle over the part of the pane
    // the tab would take — the whole of it for the middle, a half for each of the four. Five
    // identical squares cannot say that by themselves, and this is the only thing that does.
    //
    // Exactly half, and out to the mark's own edges. A diagram inset inside the square, or short of
    // the middle by the width of a seam, reads as some other fraction and stops being a picture of
    // "the right-hand side of this pane".
    let ink = h.app.theme.accent.mark;
    let segments = h.segments();
    for zone in Zone::ALL {
        let mark = hint_rect(over, zone);
        let want = crate::ui::hint_part(mark, zone);
        let drawn = segments
            .iter()
            .filter(|([a, b], colour)| *colour == ink && mark.contains(*a) && mark.contains(*b))
            .fold(None, |union: Option<Rect>, ([a, b], _)| {
                let dash = Rect::from_two_pos(*a, *b);
                Some(union.map_or(dash, |all| all.union(dash)))
            })
            .unwrap_or_else(|| panic!("the {zone:?} mark has no diagram drawn in it"));
        assert!(
            drawn.min.distance(want.min) < 0.6 && drawn.max.distance(want.max) < 0.6,
            "the {zone:?} mark draws {drawn:?}, which is not the {zone:?} of its pane at {want:?}"
        );
        // And it is filled, not just outlined: the mark is a miniature of the preview, so the part
        // wears the accent inside the accent edge exactly as the full-size one does.
        assert!(
            hinted(want).contains(&crate::ui::hint_fill(&h.app.theme)),
            "the {zone:?} mark's part is not filled with the accent"
        );
    }
    // The accent goes over the top of them, so what says *this one* is still the only blue on
    // screen: the whole pane, because the pointer is in its middle. `azur`'s own figure for the
    // wash, the same way the grey above is `drop_hint`'s.
    let accent = h.app.theme.accent.default;
    let previewed = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 40);
    assert!(
        painted
            .iter()
            .any(|(at, _, fill)| *fill == previewed
                && at.min.distance(over.min) < 0.5
                && at.max.distance(over.max) < 0.5),
        "the pane under the pointer is not previewed in the accent"
    );
}

#[test]
fn a_pane_stacked_below_another_gets_a_reachable_strip_of_its_own() {
    let mut h = Harness::new();
    h.app.actions.push(Action::SplitFocused {
        side: crate::pane::Side::Bottom,
    });
    h.settle();
    assert_eq!(h.app.panes.len(), 2);

    // The lower pane is the one whose top is furthest down.
    let lower = h
        .app
        .panes
        .iter()
        .max_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
        .map(|p| p.id)
        .expect("two panes");
    let slot = h
        .app
        .tab_slots
        .iter()
        .find(|s| s.pane == lower)
        .map(|s| s.rect)
        .expect("the lower pane has no tab on screen at all");

    assert!(
        slot.top() > crate::ui::chrome::HEIGHT,
        "a pane in the row below cannot have its tabs in the title bar, but {slot:?} is"
    );
    let pane_top = h
        .app
        .panes
        .iter()
        .find(|p| p.id == lower)
        .map(|p| p.rect.top())
        .unwrap();
    assert!(
        slot.bottom() <= pane_top,
        "the strip has to be above the pane it governs, not inside it"
    );

    // And it is a tab, not a picture of one.
    let index = h.app.tab_slots.iter().position(|s| s.pane == lower).unwrap();
    let id = Id::new(("tab", lower, h.app.tab_slots[index].tab));
    assert!(
        h.hovers(id, slot.center()),
        "the tab on the band is not reachable"
    );
}

#[test]
fn split_or_not_every_tab_is_in_the_title_bar() {
    let mut h = Harness::new();
    h.app.panes[0]
        .tabs
        .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
    h.settle();
    assert_eq!(h.app.tab_slots.len(), 2, "one pane, two tabs");

    h.app.actions.push(Action::SplitFocused {
        side: crate::pane::Side::Right,
    });
    h.settle();
    assert_eq!(h.app.panes.len(), 2, "the split happened");

    // Every tab in the window, whichever pane governs it, is in the title bar — and
    // each pane has its own group there, so the tabs are grouped by owner rather than
    // merged into one strip.
    let bar = crate::ui::chrome::HEIGHT;
    for slot in &h.app.tab_slots {
        assert!(
            slot.rect.top() >= 0.0 && slot.rect.bottom() <= bar,
            "a tab for pane {} is at {:?}, outside the title bar",
            slot.pane,
            slot.rect
        );
    }
    for pane in &h.app.panes {
        let group: Vec<&crate::ui::chrome::Slot> = h
            .app
            .tab_slots
            .iter()
            .filter(|s| s.pane == pane.id)
            .collect();
        assert_eq!(
            group.len(),
            pane.tabs.len(),
            "pane {} has {} tabs and {} of them are on screen",
            pane.id,
            pane.tabs.len(),
            group.len()
        );
    }
    // Groups do not interleave: every slot of the left pane is left of every slot of
    // the right one, which is what makes the divider between them mean anything.
    let left = h.app.pane_order[0];
    let split = h
        .app
        .tab_slots
        .iter()
        .filter(|s| s.pane == left)
        .map(|s| s.rect.right())
        .fold(f32::MIN, f32::max);
    assert!(
        h.app
            .tab_slots
            .iter()
            .filter(|s| s.pane != left)
            .all(|s| s.rect.left() >= split),
        "the two panes' tabs are mixed together in the bar"
    );
}

/// `Reset window size` asks the platform for the default size, and un-maximises on the way.
///
/// The order is the part worth pinning. A window that has been maximised is the one most
/// likely to be sitting there when somebody reaches for this, and asking for a size while
/// still maximised is asking for something a window manager is entitled to ignore. Windows
/// happens not to — measured: from a maximised 2560×1392 the window comes back to 1024×600
/// with the un-maximise taken out, because `SetWindowPos` restores on the way — but that is
/// winit's platform behaviour rather than a promise, so the state is asked for explicitly.
#[test]
fn reset_window_size_asks_for_the_default_and_unmaximises_first() {
    use egui::ViewportCommand as Cmd;

    let mut h = Harness::new();
    h.settle();
    let [w, hh] = crate::config::WINDOW_SIZE;

    // From maximised: both commands, un-maximise first.
    h.app.maximized = true;
    h.commands.clear();
    h.app
        .perform(&h.ctx.clone(), Action::Window(WindowAction::ResetSize));
    // Checked before the next frame, not after. The harness feeds a viewport of a fixed size
    // for ever, so the frame that follows re-samples that and writes it back over this —
    // which is the right behaviour against a real window, where the frame that follows a
    // resize is the one that sees the new shape.
    assert!(!h.app.maximized, "the flag still says maximised");
    assert_eq!(h.app.window_size, Some([w, hh]));

    h.frame(Vec::new());
    let asked: Vec<&Cmd> = h
        .commands
        .iter()
        .filter(|c| matches!(c, Cmd::Maximized(_) | Cmd::InnerSize(_)))
        .collect();
    assert_eq!(
        asked,
        vec![&Cmd::Maximized(false), &Cmd::InnerSize(egui::vec2(w, hh))],
        "from maximised, the window was asked for {asked:?}"
    );

    // From a restored window there is nothing to un-maximise, so only the size is asked for.
    h.app.maximized = false;
    h.app.window_size = Some([1380.0, 840.0]);
    h.commands.clear();
    h.app
        .perform(&h.ctx.clone(), Action::Window(WindowAction::ResetSize));
    h.frame(Vec::new());
    let asked: Vec<&Cmd> = h
        .commands
        .iter()
        .filter(|c| matches!(c, Cmd::Maximized(_) | Cmd::InnerSize(_)))
        .collect();
    assert_eq!(
        asked,
        vec![&Cmd::InnerSize(egui::vec2(w, hh))],
        "a restored window should only be asked for a size: {asked:?}"
    );

    // And the default is one value, not two: the size the window opens at on a first run is
    // the size this puts it back to.
    assert_eq!(Config::default().window.unwrap_or(crate::config::WINDOW_SIZE), [w, hh]);
}

/// **Two panes side by side can be resized, scrollbar or no scrollbar.**
///
/// [`dock::GRAB`] reaches four points into the pane on each side of the divider, because a
/// one-point grab target is not one — and in a horizontal split those four points are exactly
/// where the listing's vertical scrollbar is. Within a layer egui gives a click to the *last*
/// widget registered, so while the splitters went up before the panes the scrollbar took the
/// pointer and **the divider between two side-by-side panes did nothing at all**. A stacked pair
/// never showed it: there the grab reaches the pane's bottom edge, where a vertical scroll area
/// has nothing to claim.
///
/// **What this does and does not show.** It drives a real double-click at the divider over a
/// listing long enough to have a scrollbar, and the splitter answers by evening the split up —
/// so the divider is reachable and its `Sense` is right. It is *not* a guard on the ordering:
/// moving the block back above the panes was tried, and this still passed. Whatever competes for
/// those four points in the window does not exist in the harness, so the ordering itself is only
/// checked by using the window. Worth knowing before trusting this test to catch a regression in
/// it.
#[test]
fn side_by_side_panes_can_be_resized_over_the_scrollbar() {
    // Long enough to overflow the pane and put a scrollbar down its right edge.
    let deep = crate::sandbox::fresh("splitter-over-scrollbar");
    for i in 0..200 {
        std::fs::write(deep.join(format!("file-{i:03}.txt")), b"x").expect("a fixture file");
    }

    let mut h = Harness::with_panes(2);
    h.settle();
    let route = h
        .app
        .splitters
        .iter()
        .find(|s| s.horizontal)
        .map(|s| s.route.clone())
        .expect("two panes opened side by side have a horizontal splitter");

    // The pane on the left of it is the one whose scrollbar is in the way.
    let left = h
        .app
        .pane_rects
        .iter()
        .min_by(|a, b| a.1.left().total_cmp(&b.1.left()))
        .map(|(id, _)| *id)
        .expect("a pane");
    let index = h.app.panes.iter().position(|p| p.id == left).expect("its tab");
    let ctx = h.ctx.clone();
    h.app.perform(
        &ctx,
        Action::Navigate {
            pane: left,
            path: deep.clone(),
        },
    );
    h.settle();
    assert!(
        h.tab(index).order.len() > 100,
        "the left pane holds {} rows, which may not overflow it -- then there is no scrollbar \
         here and this test is not testing anything",
        h.tab(index).order.len()
    );

    // Somewhere other than even, so evening it up is a visible change.
    if let Some(ratio) = h.app.layout.ratio_at(&route) {
        *ratio = 0.3;
    }
    h.settle();
    let moved = h
        .app
        .splitters
        .iter()
        .find(|s| s.route == route)
        .map(|s| s.rect)
        .expect("the splitter after the ratio moved");

    let at = moved.center();
    assert!(
        h.hovers(egui::Id::new(("splitter", &route)), at),
        "the divider at {at:?} is not the widget under the pointer -- something registered \
         later is on top of it, which is how the scrollbar used to win"
    );
    h.double_click_at(at);
    assert_eq!(
        h.app.layout.ratio_at(&route).copied(),
        Some(0.5),
        "double-clicking the divider did not even the split up, so the gesture never reached it"
    );
}

/// The panels are square, flush, and separated by one line of the selected tab's colour.
///
/// Asserted against the shapes the frame actually painted, not against the source. Three
/// separate things had to agree for the old look — a corner radius, a `stroke-subtle` ring
/// and a four-point gap — and each of them was written down somewhere else, which is how a
/// window ends up with panels that read as loose cards without anyone having decided that.
#[test]
fn the_panels_are_square_and_a_single_line_apart() {
    let mut h = Harness::with_panes(2);
    h.settle();

    let seam = crate::ui::seam(&h.app.theme);
    let layer = h.app.theme.bg.layer;

    // ---- The gaps, from the layout ------------------------------------
    let body = Rect::from_min_max(
        pos2(0.0, crate::ui::chrome::bar_rect(Rect::from_min_size(Pos2::ZERO, h.size)).bottom()),
        Pos2::ZERO + h.size,
    );
    let (sidebar, panes_area) = App::split_body(body, h.app.sidebar_width);
    assert_eq!(
        panes_area.left() - sidebar.right(),
        crate::ui::SEAM,
        "the sidebar and the panes are {} apart",
        panes_area.left() - sidebar.right()
    );

    let mut rects: Vec<Rect> = h.app.pane_rects.iter().map(|(_, r)| *r).collect();
    rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
    assert_eq!(rects.len(), 2, "this test wants two panes side by side");
    assert_eq!(
        rects[1].left() - rects[0].right(),
        crate::ui::SEAM,
        "two panes are {} apart",
        rects[1].left() - rects[0].right()
    );

    // ---- The fills, from the frame ------------------------------------
    //
    // The seam is not a shape of its own: it is what the fill behind the whole block leaves
    // showing, so what is checked is that the block is there, in that colour, under panels
    // that are square and do not cover it.
    let (corner, fill) = h
        .fill_at(sidebar)
        .expect("nothing was painted at the sidebar's rect");
    assert_eq!(fill, layer, "the sidebar is not `background-layer`");
    assert_eq!(corner, egui::CornerRadius::ZERO, "the sidebar has rounded corners");

    for rect in &rects {
        let (corner, fill) = h
            .fill_at(*rect)
            .unwrap_or_else(|| panic!("nothing was painted at the pane's rect {rect:?}"));
        assert_eq!(fill, layer, "a pane is not `background-layer`");
        assert_eq!(corner, egui::CornerRadius::ZERO, "a pane has rounded corners");
    }

    let block = sidebar.union(panes_area);
    let (corner, fill) = h
        .fill_at(block)
        .expect("nothing is painted behind the panels for the seams to show");
    assert_eq!(fill, seam, "the seams are not the selected tab's colour");
    assert_eq!(corner, egui::CornerRadius::ZERO);

    // ---- And nothing but the window's border outside them --------------
    //
    // The block fills the body: no canvas ring, and the panels reach three of the window's
    // four edges. Written as edges rather than as "no gutter" because that is the thing that
    // can be seen — a stripe of `background-canvas` down the side of the window.
    assert_eq!(block, body, "there is canvas showing around the panels");
    assert_eq!(sidebar.left(), 0.0, "the sidebar stops short of the window");
    assert_eq!(
        rects[1].right(),
        h.size.x,
        "the last pane stops short of the window's right edge"
    );
    for rect in &rects {
        assert_eq!(
            rect.bottom(),
            h.size.y,
            "a pane stops short of the window's bottom edge"
        );
    }
}

/// The window's resize band does not swallow the listing's scrollbar.
///
/// This is the bill for the panels reaching the window's edges. The east band is registered
/// last and so wins any click in the right-hand four points, which used to be canvas and is
/// now the outer edge of a `10`-point scrollbar. Six points is enough — but "enough" is a
/// claim about a click target, and the only way to know is to press the pointer down there
/// and see whether the listing moves.
#[test]
fn the_scrollbar_survives_the_window_resize_band() {
    let mut h = Harness::new();
    // Short enough that the folder overflows and there is a bar to grab at all.
    h.size = egui::vec2(1024.0, 300.0);
    h.settle();
    assert_eq!(h.app.panes[0].tab().scroll_y, 0.0, "it starts at the top");

    // As far out as the pointer can go and still be the scrollbar's: one point inside the
    // band, which is the pixel this test exists to defend.
    let x = h.size.x - crate::ui::GUTTER - 1.0;
    let y = h.size.y * 0.5;
    h.frame(vec![Event::PointerMoved(pos2(x, y))]);
    h.frame(vec![Event::PointerButton {
        pos: pos2(x, y),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=4 {
        h.frame(vec![Event::PointerMoved(pos2(x, y + step as f32 * 10.0))]);
    }
    let scrolled = h.app.panes[0].tab().scroll_y;
    h.frame(vec![Event::PointerButton {
        pos: pos2(x, y + 40.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(
        scrolled > 0.0,
        "dragging the scrollbar {} points from the window's edge scrolled nothing",
        h.size.x - x
    );
}
