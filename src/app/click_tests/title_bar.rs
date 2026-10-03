//! The title bar this program draws itself: the mark, the tabs, the window buttons, and the
//! switches at the foot of the window.

use super::*;

#[test]
fn the_application_mark_is_reachable() {
    let mut h = Harness::new();
    let found = h.find(
        Id::new("app-menu"),
        crate::ui::GUTTER + crate::ui::TOOL_SIZE * 0.5,
        0..crate::ui::chrome::HEIGHT as i32,
    );
    assert!(
        found.is_some(),
        "the application mark cannot be reached -- something is covering the \
         top-left corner of the title bar"
    );
}

#[test]
fn the_window_buttons_are_reachable() {
    let mut h = Harness::new();
    for (which, from_right) in [
        (WindowAction::Close, 23.0),
        (WindowAction::ToggleMaximize, 69.0),
        (WindowAction::Minimize, 115.0),
    ] {
        let id = Id::new(("caption", which as u8));
        let found = h.find(
            id,
            h.size.x - from_right,
            0..crate::ui::chrome::HEIGHT as i32,
        );
        assert!(
            found.is_some(),
            "{which:?} cannot be reached -- the top-right corner is covered"
        );
    }
}

#[test]
fn a_tab_and_its_close_button_are_clickable() {
    let mut h = Harness::new();
    h.app.panes[0]
        .tabs
        .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
    h.app.panes[0].active = 1;
    h.settle();

    let pane = h.app.panes[0].id;
    let strip_y = crate::ui::chrome::HEIGHT * 0.5;

    let at = (0..500)
        .step_by(2)
        .map(|dx| pos2(dx as f32, strip_y))
        .find(|at| h.hovers(Id::new(("tab", pane, 0usize)), *at))
        .expect("the first tab is not reachable");
    let done = h.click_at(at);
    assert!(
        done.contains(&"ActivateTab"),
        "a tab did not respond, got {done:?}"
    );
    assert_eq!(h.app.panes[0].active, 0);

    let close = (0..500)
        .step_by(2)
        .map(|dx| pos2(dx as f32, strip_y))
        .find(|at| h.hovers(Id::new(("tab-close", pane, 0usize)), *at))
        .expect("a tab's close button is not reachable");
    let done = h.click_at(close);
    assert!(
        done.contains(&"CloseTab"),
        "the close button did not fire, got {done:?}"
    );
    assert_eq!(h.app.panes[0].tabs.len(), 1);
}

/// The console's switch takes a click, and it is the same toggle the shortcut is.
///
/// **The reachability is the point of it.** `chrome::resize_borders` is registered *last* so that
/// it sits over everything, egui gives the pointer to the last widget that asked for it, and the
/// bottom band overlaps the status line — which the layout table in the README notes as costing
/// nothing precisely because nothing there was ever clickable before. So this sweeps the bar for
/// something under the pointer before it asserts anything about what a click does; a switch that
/// looks perfect and cannot be reached is a switch that does not work.
///
/// It stops one frame short of letting the console *draw*, and puts the flag back by hand.
/// Drawing the panel starts a real shell — with the developer's own `.bashrc` — and a test process
/// has no business doing that. What is being checked is the switch, and the switch has done its
/// job by the time the flag moves.
#[test]
fn the_console_switch_is_reachable_and_toggles_the_console() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let id = Id::new(("console-switch", pane));
    let rect = h.app.panes[0].rect;
    let y = rect.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
    let at = (0..120)
        .step_by(2)
        .map(|dx| pos2(rect.left() + dx as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("the console switch is not reachable along its own bar");

    assert!(!h.app.panes[0].console_open, "it starts shut");
    h.take_journal();
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    let done = h.take_journal();
    assert!(
        done.contains(&"ToggleConsole"),
        "the switch did not fire, got {done:?}"
    );
    assert!(h.app.panes[0].console_open, "the console did not open");
    h.app.panes[0].console_open = false;
}

/// The view switch takes a click, and what it switches to can be clicked as well.
///
/// **Two claims, and both of them are only checkable this way.**
///
/// The switch sits in the status line, which the window's bottom resize band overlaps — and
/// `chrome::resize_borders` is registered last, so it wins the pointer wherever the two meet. It is
/// also the *first* thing on that bar now, with the console's switch four points to its right, so
/// there are two 18-point targets to tell apart in a 22-point strip. So the bar is swept for
/// something under the pointer before anything is asserted about a click, and the sweep runs
/// **outwards from the left**, where a version of this that had drifted onto its neighbour would
/// show up as the wrong id being hovered rather than as nothing at all.
///
/// Then the grid itself. A tile is not a widget — the whole view is one interaction with the cell
/// derived from the pointer — so "the tiles are where the arithmetic says" is not something
/// `read_response` can be asked. It is asked by clicking and seeing what gets selected, and the
/// first line is *swept* rather than computed: the tiles are spread across the pane, so where the
/// first one's box begins is arithmetic this test would only be restating.
#[test]
fn the_view_switch_is_reachable_and_its_tiles_can_be_clicked() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let rect = h.pane_rect(0);
    let y = rect.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
    let id = Id::new(("view-switch", pane));
    let at = (0..60)
        .step_by(2)
        .map(|dx| pos2(rect.left() + dx as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("the view switch is not reachable along its own bar");
    // And it is in *front* of the console's, which is the position that was asked for. Checked
    // from the far side of the pointer's own reach: the console switch has to be somewhere to the
    // right of where this one answered.
    let console = (0..120)
        .step_by(2)
        .map(|dx| pos2(rect.left() + dx as f32, y))
        .find(|probe| h.hovers(Id::new(("console-switch", pane)), *probe))
        .expect("the console switch went missing when the view switch moved in beside it");
    assert!(
        console.x > at.x,
        "the view switch is at {} and the console's at {}, which is the wrong way round",
        at.x,
        console.x
    );

    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Details,
        "it starts in the details view"
    );
    let done = h.click_at(at);
    assert!(
        done.contains(&"SetView"),
        "the switch did not fire, got {done:?}"
    );
    assert_eq!(h.tab(0).view_mode, crate::pane::ViewMode::Icons);

    // A frame of the grid, then a sweep along its first line of tiles for a click that lands on
    // one. The selection is cleared between tries, since a click in the gap between two tiles is
    // a click on the folder and cancels it — which is the other half of what is being checked.
    h.settle();
    let body_top = h.pane_content_top(0) + crate::ui::breadcrumb::HEIGHT;
    let line = body_top + crate::ui::grid::CELL_H * 0.5;
    let mut landed = None;
    for dx in (0..260).step_by(6) {
        let at = pos2(rect.left() + dx as f32, line);
        h.click_at(at);
        if h.tab(0).selected_count == 1 {
            landed = Some(at);
            break;
        }
    }
    landed.expect("no click along the first line of tiles selected anything");
    assert_eq!(h.tab(0).selected_count, 1);
    assert_eq!(
        h.tab(0).cursor,
        Some(0),
        "the leftmost tile of the first line is the first row of the order"
    );

    // And the switch goes back, which is the whole of what a two-state switch has to do. Found
    // again rather than reused, so that a latched switch is proved reachable as well as an
    // unlatched one — a fill is drawn under it in that state and a hit rect is not a fill.
    let back = (0..60)
        .step_by(2)
        .map(|dx| pos2(rect.left() + dx as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("the switch is not reachable while it is latched");
    let done = h.click_at(back);
    assert!(done.contains(&"SetView"), "got {done:?}");
    assert_eq!(h.tab(0).view_mode, crate::pane::ViewMode::Details);
}

/// The status line's `N changed` is a button, and pressing it shows what has changed.
///
/// Three things, and the middle one is the reason this is a test rather than a reading of the
/// source. **It has to be reachable**: it sits in the same bar the window's bottom resize band
/// overlaps, and that band is registered last — the console switch at the other end of the line
/// needed the same sweep for the same reason. **It has to be quiet until the pointer is on it**,
/// which is what "subtle" means here and is a claim about a fill, not about the source. And the
/// press has to do *both* halves: a flatten on its own is the whole tree, and the lens on its own
/// is one folder's children.
///
/// The count is stubbed rather than read. Git is asked for real in this suite — the harness opens
/// this repository — so the button's existence would otherwise depend on whether the checkout
/// happens to be dirty, which is to say on who is running the test.
#[test]
fn the_changed_count_is_a_button_that_shows_what_has_changed() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();

    // `src`, not the repository root the harness opens: the press this test ends with starts a
    // deep walk, and the root has `target` under it.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(&ctx, Action::Navigate { pane, path });
    h.settle();
    // The real answer first, and then over the top of it — an answer still in flight would
    // otherwise land on a later frame and replace the stub mid-test.
    for _ in 0..200 {
        if h.tab(0).git_answered {
            break;
        }
        h.frame(Vec::new());
    }
    {
        let mut repo = crate::git::Repo::of([("app.rs", crate::git::State::Modified)]);
        repo.head = "master".to_owned();
        repo.changed = 1;
        repo.unstaged = 1;
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.git = Some(std::sync::Arc::new(repo));
        tab.git_answered = true;
    }
    h.frame(Vec::new());

    let id = Id::new(("status-changed", pane));
    let rect = h.app.panes[0].rect;
    let y = rect.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
    let at = (0..300)
        .step_by(2)
        .map(|dx| pos2(rect.left() + dx as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("`N changed` is not reachable along its own bar");

    // Nothing under it at rest, and something under it hovered. The pointer is parked off the
    // bar for the first half: `hovers` left it on the button.
    let hover_fill = crate::ui::control_fills(&h.app.theme, h.app.theme.bg.layer_alt).0;
    let button_shaped = move |rect: &Rect, fill: &egui::Color32| {
        *fill == hover_fill
            && rect.contains(at)
            && rect.height() <= crate::ui::filelist::STATUS_HEIGHT
    };
    h.frame(vec![Event::PointerMoved(pos2(rect.center().x, y - 200.0))]);
    assert!(
        !h.rects()
            .iter()
            .any(|(r, _, fill)| button_shaped(r, fill)),
        "the count is wearing a control's fill with the pointer nowhere near it"
    );
    h.frame(vec![Event::PointerMoved(at)]);
    assert!(
        h.rects().iter().any(|(r, _, fill)| button_shaped(r, fill)),
        "hovering the count did not light it up"
    );

    let done = h.click_at(at);
    assert!(
        done.contains(&"SetLens"),
        "the count did not fire, got {done:?}"
    );
    assert!(h.tab(0).flat, "the flatten was not turned on");
    assert_eq!(
        h.tab(0).lens,
        Some(crate::pane::Lens::Git),
        "the listing does not ask git"
    );

    // And pressed again it stays on, because it is not a toggle: the second press is about the
    // filter, and a button labelled with a fact about the repository should not undo itself.
    //
    // Through the action rather than through the pixels, and the reason is worth writing down: a
    // flatten **re-asks git**, because the marks are keyed by path relative to the folder and a
    // flattened listing's rows are not the same paths. So the count is legitimately absent for
    // the frame or two that takes, and a second sweep of the bar would be waiting on a real
    // `git status` of a real checkout to say something in particular.
    h.app.perform(
        &ctx,
        Action::SetLens {
            pane,
            lens: Some(crate::pane::Lens::Git),
        },
    );
    assert!(h.tab(0).flat, "a second press turned the flatten back off");
    assert_eq!(h.tab(0).lens, Some(crate::pane::Lens::Git));
}

#[test]
fn the_new_tab_button_makes_a_tab() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let before = h.app.panes[0].tabs.len();
    let at = (0..600)
        .step_by(2)
        .map(|dx| pos2(dx as f32, crate::ui::chrome::HEIGHT * 0.5))
        .find(|at| h.hovers(Id::new(("new-tab", pane)), *at))
        .expect("the + button is not reachable");
    let done = h.click_at(at);
    assert!(done.contains(&"NewTab"), "the + did not fire, got {done:?}");
    assert_eq!(h.app.panes[0].tabs.len(), before + 1);
}

/// One hover grey, everywhere, whatever kind of thing is under the pointer.
///
/// The window had two for a while and it was not obvious which: the rows, the sidebar and the
/// path bar's segments moved to this program's own grey, and Back, Forward, Up and Refresh went
/// on wearing Azur's `control-hover` because they reach it through `ui::control_fills` rather
/// than through `ui::row_fill`. Four buttons in the middle of the window, a rung and a half off
/// everything around them. The fix was to stop having two sources — `background-control-hover`
/// is set by `crate::theme` and everything reads it — and this is the guard, over four widgets
/// that get there by four different routes.
#[test]
fn every_hover_in_the_window_is_the_same_grey() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let grey = crate::ui::hover_fill(&h.app.theme);
    let bar = h.path_bar_y(0);
    let strip = crate::ui::chrome::HEIGHT * 0.5;

    // `Back` and `Forward` are disabled in a tab that has been nowhere, and a disabled button
    // paints no fill at all — so the two of the four that are always live stand for them.
    let widgets: [(Id, f32, &str); 4] = [
        (Id::new(("nav", pane, "Up (Alt+Up)")), bar, "Up"),
        (Id::new(("nav", pane, "Refresh (F5)")), bar, "Refresh"),
        (Id::new(("th", pane, 0usize)), h.header_y(0), "a column header"),
        (Id::new(("new-tab", pane)), strip, "the new-tab button"),
    ];
    for (id, y, what) in widgets {
        let at = (0..1024)
            .step_by(2)
            .map(|x| pos2(x as f32, y))
            .find(|at| h.hovers(id, *at))
            .unwrap_or_else(|| panic!("{what} is not reachable at y {y}"));
        let _ = at;
        let rect = h
            .ctx
            .read_response(id)
            .unwrap_or_else(|| panic!("{what} was not drawn"))
            .rect;
        assert_eq!(
            h.fill_at(rect).map(|(_, fill)| fill),
            Some(grey),
            "{what} hovers in a grey of its own"
        );
    }
}

#[test]
fn losing_the_window_puts_the_path_bar_and_the_context_menu_away() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;

    // The path bar's dropdown, opened the way anyone opens it.
    let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
    let index = crumbs.len() - 1;
    let y = h.path_bar_y(0);
    let at = (0..1200)
        .step_by(2)
        .map(|x| pos2(x as f32, y))
        .find(|at| h.hovers(Id::new(("crumb-chevron", pane, index)), *at))
        .expect("the chevron before the current folder is not reachable");
    h.wait();
    h.click_at(at);
    h.frame(Vec::new());
    assert_eq!(h.app.crumbs.showing(), Some((pane, index)), "nothing opened");

    // And a context menu. Built from this program's own entries, the way a right-button drag
    // builds one, so that opening it does not involve asking the shell anything.
    let own = vec![crate::shell::menu::Entry::own(
        crate::shell::menu::Own::Cancel,
    )];
    h.app.menu = Some(crate::ui::menu::Open::new(
        pane,
        pos2(200.0, 300.0),
        Vec::new(),
        PathBuf::new(),
        own,
        crate::shell::menu::Depth::Full,
        0,
    ));

    h.focused = false;
    h.frame(Vec::new());

    assert!(h.app.menu.is_none(), "the context menu is still up");
    assert_eq!(
        h.app.crumbs.showing(),
        None,
        "the path bar's dropdown is still up, and with it the tracking mode"
    );
}

#[test]
fn losing_the_window_puts_the_application_menu_away() {
    // The one at the top left, which egui tracks in its own memory rather than this program
    // — so it needs `Popup::close_all` and would not be caught by the test above.
    let mut h = Harness::new();
    let at = (0..64)
        .step_by(2)
        .map(|x| pos2(x as f32, crate::ui::chrome::HEIGHT * 0.5))
        .find(|at| h.hovers(Id::new("app-menu"), *at))
        .expect("the application mark is not reachable");
    h.click_at(at);
    h.frame(Vec::new());
    assert!(
        egui::Popup::is_any_open(&h.ctx),
        "the mark did not open its menu"
    );

    h.focused = false;
    h.frame(Vec::new());
    assert!(
        !egui::Popup::is_any_open(&h.ctx),
        "the application menu is still open"
    );
}

#[test]
fn ctrl_shift_t_puts_a_tab_back_and_does_not_also_open_a_new_one() {
    // Both of these hang off `Ctrl` and the same key, so the interesting half of the
    // assertion is the one about `NewTab` *not* being in the journal: without the `Shift`
    // test on the other arm, this gesture reopened the closed tab and opened a blank one
    // beside it, and only a test that drives the real keyboard path can see that.
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.panes[0].tabs.push(Tab::new(path.clone()));
    h.app.panes[0].active = 1;
    h.settle();

    h.app
        .perform(&h.ctx.clone(), Action::CloseTab { pane, tab: 1 });
    assert_eq!(h.app.panes[0].tabs.len(), 1);

    h.take_journal();
    let held = Modifiers::COMMAND | Modifiers::SHIFT;
    h.modifiers = held;
    h.frame(vec![Event::Key {
        key: egui::Key::T,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: held,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    let done = h.take_journal();

    assert!(
        done.contains(&"ReopenTab"),
        "Ctrl+Shift+T did nothing, got {done:?}"
    );
    assert!(
        !done.contains(&"NewTab"),
        "Ctrl+Shift+T opened a blank tab as well, got {done:?}"
    );
    assert_eq!(h.app.panes[0].tabs.len(), 2);
    assert_eq!(h.app.panes[0].tabs[1].path, path);
}

#[test]
fn the_bare_title_bar_moves_and_maximises_the_window() {
    let mut h = Harness::new();
    // Between the application mark and the first tab there is bar and nothing else,
    // which is where a window is grabbed.
    let bar_y = crate::ui::chrome::HEIGHT * 0.5;
    let bare = pos2(
        crate::ui::chrome::content_left(crate::ui::chrome::bar_rect(Rect::from_min_size(
            Pos2::ZERO,
            h.size,
        ))) + 60.0,
        bar_y,
    );

    let done = h.drag(bare, pos2(bare.x + 80.0, bare.y + 40.0));
    assert!(
        done.contains(&"Window"),
        "dragging the bar has to move the window, got {done:?}"
    );

    let done = h.double_click_at(bare);
    assert!(
        done.contains(&"Window"),
        "double-clicking the bar has to maximise, got {done:?}"
    );
}
