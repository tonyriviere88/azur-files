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
/// also the *middle* one of three 18-point targets four points apart in a 22-point strip, with the
/// console's before it and the measurement's after. So the bar is swept for something under the
/// pointer before anything is asserted about a click, and the sweep runs **outwards from the left**,
/// where a version of this that had drifted onto its neighbour would show up as the wrong id being
/// hovered rather than as nothing at all.
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
    // And it is *behind* the console's, which is the position that was asked for: the three switches
    // are in the order the three keys that work them are printed on the number row — `Ctrl+²`,
    // `Ctrl+1`, `Ctrl+2` — so the console's comes first. Checked from the far side of the pointer's
    // own reach: the console switch has to be somewhere to the left of where this one answered, and
    // both have to answer at all, which is the half that catches two rects laid on top of each other.
    let console = (0..120)
        .step_by(2)
        .map(|dx| pos2(rect.left() + dx as f32, y))
        .find(|probe| h.hovers(Id::new(("console-switch", pane)), *probe))
        .expect("the console switch went missing when the view switch moved in beside it");
    assert!(
        console.x < at.x,
        "the console switch is at {} and the view switch at {}, which is the wrong way round",
        console.x,
        at.x
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

/// **`Ctrl+1` and `Ctrl+2` work the two switches beside the console's, and no longer pick a tab.**
///
/// Driven through real frames for the reason the undo shortcuts are — see
/// [`super::transfer::the_undo_shortcuts_reach_undo_and_redo`]: what breaks in a shortcut is the
/// wiring, and wiring looks correct while doing nothing.
///
/// Three claims, and the third is the one that needs a test rather than a reading. `Ctrl+1`…`9` used
/// to activate a tab by number, and the two keys taken here were taken *from* that loop. A binding
/// that fires both would switch the view **and** jump to the first tab, which on a window with one
/// tab is indistinguishable from working correctly — so the tab is deliberately not the first one
/// when the key is pressed.
///
/// Both switches **toggle**, exactly as their buttons do, so each key is pressed twice: the key that
/// turned the tiles on has to be the key that turns them off, or the way back is a mouse.
#[test]
fn ctrl_1_and_ctrl_2_work_the_view_and_the_measurement_and_leave_the_tabs_alone() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctrl = Modifiers {
        command: true,
        ctrl: true,
        ..Modifiers::NONE
    };
    // A second tab, and it is the active one — see the doc above for why that matters.
    h.app.perform(&h.ctx.clone(), Action::NewTab { pane });
    h.settle();
    assert_eq!(
        h.app.panes[0].active, 1,
        "the fixture needs the second tab to be the active one"
    );

    let fired = |h: &mut Harness, key: egui::Key| -> Vec<&'static str> {
        h.app.journal = Some(Vec::new());
        h.modifiers = ctrl;
        h.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: ctrl,
        }]);
        h.modifiers = Modifiers::NONE;
        h.app.journal.clone().unwrap_or_default()
    };

    // ---- Ctrl+1: the view, both ways -----------------------------------
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Details,
        "a fresh tab on this folder is a listing of rows"
    );
    let done = fired(&mut h, egui::Key::Num1);
    assert!(done.contains(&"SetView"), "Ctrl+1 produced {done:?}");
    assert!(
        !done.contains(&"ActivateTab"),
        "Ctrl+1 switched the view and jumped to a tab: {done:?}"
    );
    assert_eq!(h.tab(0).view_mode, crate::pane::ViewMode::Icons);
    assert_eq!(h.app.panes[0].active, 1, "Ctrl+1 changed which tab is up");

    let done = fired(&mut h, egui::Key::Num1);
    assert!(done.contains(&"SetView"), "Ctrl+1 again produced {done:?}");
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Details,
        "Ctrl+1 is a switch, so the second press has to come back"
    );

    // ---- Ctrl+2: the measurement, both ways ----------------------------
    assert!(!h.tab(0).sizes.on, "nothing is being measured yet");
    let done = fired(&mut h, egui::Key::Num2);
    assert!(done.contains(&"ToggleSizes"), "Ctrl+2 produced {done:?}");
    assert!(
        !done.contains(&"ActivateTab"),
        "Ctrl+2 started the measurement and jumped to a tab: {done:?}"
    );
    assert!(h.tab(0).sizes.on, "Ctrl+2 did not start the measurement");
    assert_eq!(h.app.panes[0].active, 1, "Ctrl+2 changed which tab is up");

    let done = fired(&mut h, egui::Key::Num2);
    assert!(done.contains(&"ToggleSizes"), "Ctrl+2 again produced {done:?}");
    assert!(
        !h.tab(0).sizes.on,
        "Ctrl+2 is a switch, so the second press has to turn the column off"
    );

    // ---- And the rest of the row is nobody's ---------------------------
    //
    // `Ctrl+3` was tab 3 and is now unbound. Asserted because the alternative to dropping the
    // range was keeping `3`…`9`, and a half-range is the thing that was decided against — see
    // [`crate::app::App::keyboard`].
    let done = fired(&mut h, egui::Key::Num3);
    assert!(
        !done.contains(&"ActivateTab"),
        "Ctrl+3 still picks a tab by number: {done:?}"
    );
}

/// **Each of the three switches names its key, and the key is a shade back from the sentence.**
///
/// A switch at the bottom corner of a pane is found by pointing at it, so its tooltip is the only
/// place the keystroke that saves the trip next time can be said — and `Ctrl+²`, `Ctrl+1` and
/// `Ctrl+2` are the three leftmost keys of the number row in the order the buttons sit in.
///
/// # Why this is a test and not a reading of three string literals
///
/// The two colours are not written at any call site. A tooltip is one string; the design system
/// *recognises* the `Label (Keys)` shape and dims the bracket — see
/// `azur_egui_theme::components::shortcut_in`, which explains at length why that is recognised rather
/// than passed in. The consequence is that a tooltip can be worded in a way the guard does not take,
/// and nothing anywhere says so: the words are right, the shortcut is right, and it comes out one
/// colour. That is exactly what `Ctrl+²` did — `²` is a Unicode `No` rather than an ASCII digit, so
/// the one shortcut on this bar that could not be typed on a QWERTY board was also the one shortcut
/// in the window wearing `text-primary`. Reading the literals proves nothing about it. This asks the
/// frame.
///
/// Asserted on the **two runs of the painted galley**, because the halves are deliberately one galley
/// — they have to sit on one baseline and break as one paragraph — so `Harness::texts` reports the
/// whole sentence and says nothing about which part is dim. [`Harness::coloured`] is the accessor that
/// splits a galley where its colour changes.
///
/// The pointer is parked off the window between switches: a tooltip is held up while *something* is
/// hovered, and moving from one 18-point target to the next four points away is a move egui does not
/// delay — so without a gap in between, the tooltip under test could be the previous switch's.
#[test]
fn each_switch_on_the_status_line_names_its_key_in_a_quieter_colour() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let bar = h.app.panes[0].rect;
    let y = bar.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
    // Read out as two colours rather than held as a borrow of the theme: everything below this
    // needs `h` mutably, since a hover is a frame.
    let (ink, aside) = (h.app.theme.text.primary, h.app.theme.text.secondary);

    // In the order they are on the bar, which is the order of the keys. The sentence and the aside
    // separately, because that is the split being asserted — and together they are the tooltip the
    // switch was given, which is what makes this readable against the source.
    let cases = [
        ("console-switch", "Show the console", " (Ctrl+²)"),
        (
            "view-switch",
            "Show large icons, with a thumbnail on anything that has one",
            " (Ctrl+1)",
        ),
        (
            "sizes",
            "Measure each folder, and bar the share of what is on show",
            " (Ctrl+2)",
        ),
    ];
    for (which, label, keys) in cases {
        let id = Id::new((which, pane));
        // Swept rather than read off the response, for the reason the two tests above are: the
        // window's bottom resize band overlaps this bar and is registered last, so a point that is
        // inside the switch's rect is not necessarily a point the switch can be hovered at.
        let at = (0..120)
            .step_by(2)
            .map(|dx| pos2(bar.left() + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .unwrap_or_else(|| panic!("`{which}` is not reachable along the status line"));
        // egui holds a tooltip back for `interaction.tooltip_delay` and until the pointer has come
        // to rest, so this moves once and then waits — `Harness::hovers` has already moved it here,
        // and moving again on every frame would restart the delay for ever.
        for _ in 0..40 {
            h.frame(Vec::new());
        }

        let frame = h
            .tooltip_rect()
            .unwrap_or_else(|| panic!("nothing came up over `{which}`, hovered at {at:?}"));
        let runs: Vec<(String, egui::Color32)> = h
            .coloured()
            .into_iter()
            .filter(|(at, _, _)| frame.contains(*at))
            .map(|(_, run, colour)| (run, colour))
            .collect();
        let colour_of = |want: &str| -> egui::Color32 {
            runs.iter()
                .find(|(run, _)| run == want)
                .map(|(_, colour)| *colour)
                .unwrap_or_else(|| {
                    panic!("`{which}` did not paint {want:?}; it painted {runs:?}")
                })
        };
        assert_eq!(
            colour_of(label),
            ink,
            "`{which}` is not saying what it does in `text-primary`"
        );
        assert_eq!(
            colour_of(keys),
            aside,
            "`{which}` is not saying its key in `text-secondary` — the guard in `shortcut_in` \
             did not take {keys:?} for a chord, so the whole tooltip is one colour"
        );

        // Off the window, so the next switch's tooltip is its own. Two frames: one to take the
        // pointer away and one for the tooltip to notice it has gone.
        h.frame(vec![Event::PointerMoved(pos2(-100.0, -100.0))]);
        h.frame(Vec::new());
    }
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
