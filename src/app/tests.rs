use super::*;

/// An app with one pane per path, and no scan allowed to land — the tests are
/// about structure, and a listing arriving would only add noise.
fn app(paths: &[&str]) -> (App, egui::Context) {
    let ctx = egui::Context::default();
    let open: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let app = App::opening(&ctx, Config::default(), open, Side::Right);
    (app, ctx)
}

#[test]
fn a_window_gesture_keeps_frames_coming_and_then_stops() {
    let mut s = Settling::default();
    let at_rest = Shape {
        pixels: (1024, 600),
        scale: 1000,
        native_scale: 1000,
        focused: true,
        minimized: false,
    };

    // The first frame is a change from nothing, so it paints -- which is correct and
    // also why the baseline has to be established before anything is asserted.
    assert!(s.observe(at_rest, 1.0));

    // Then nothing is moving, and no frames are asked for. This is the case that must
    // cost nothing, because it is every frame of every session.
    assert!(!s.observe(at_rest, 1.0 + Settling::QUIET));
    assert!(!s.observe(at_rest, 9.0));

    // Restored onto a monitor at 125%: both the pixel size and the scale change.
    let rescaled = Shape {
        pixels: (1280, 750),
        scale: 1250,
        ..at_rest
    };
    assert!(s.observe(rescaled, 10.0), "the change itself has to repaint");
    // And it keeps painting while the gesture settles, without needing more changes --
    // which is the point: the stretched frame is on screen during these.
    assert!(s.observe(rescaled, 10.2));
    assert!(s.observe(rescaled, 10.0 + Settling::QUIET - 0.01));
    // Then stops.
    assert!(!s.observe(rescaled, 10.0 + Settling::QUIET));
    assert!(!s.observe(rescaled, 20.0));

    // Losing focus counts too: it is what a restore animation and an occlusion both
    // come with, and it was the reported trigger.
    assert!(s.observe(
        Shape {
            focused: false,
            ..rescaled
        },
        21.0
    ));
}

#[test]
fn a_scale_change_too_small_to_see_is_not_a_change() {
    // The scale arrives as an `f32`. Comparing it exactly would let a value that
    // differs in its last bit request repaints for the rest of the session.
    let mut s = Settling::default();
    let a = Shape {
        pixels: (1024, 600),
        scale: (1.0_f32 * 1000.0).round() as u32,
        native_scale: 1000,
        focused: true,
        minimized: false,
    };
    let b = Shape {
        scale: (1.000_04_f32 * 1000.0).round() as u32,
        ..a
    };
    assert_eq!(a, b, "a difference this small is not a rescale");
    s.observe(a, 1.0); // the baseline
    assert!(!s.observe(b, 1.0 + Settling::QUIET));
}

fn titles(app: &App, pane: PaneId) -> Vec<String> {
    app.panes
        .iter()
        .find(|p| p.id == pane)
        .map(|p| p.tabs.iter().map(|t| t.title.clone()).collect())
        .unwrap_or_default()
}

fn tree(app: &App) -> Vec<PaneId> {
    let mut out = Vec::new();
    app.layout.panes(&mut out);
    out
}

#[test]
fn one_pane_per_open_path() {
    let (app, _ctx) = app(&["/a", "/b", "/c"]);
    assert_eq!(app.panes.len(), 3);
    assert_eq!(tree(&app).len(), 3);
    assert_eq!(app.layout.count(), 3);
}

/// A window comes back divided the way it was left.
///
/// The end-to-end claim, and the only place the two halves of it meet: `settings` writes
/// the tree over pane *numbers* and the panes in the order the tree numbers them, and
/// `opening` has to line those numbers back up with the panes it builds. Either half alone
/// can be right while the pair is wrong — a window whose panes come back in the other
/// order, showing the right folders in the wrong places — so this goes all the way through
/// the text of the file.
#[test]
fn the_panes_come_back_the_way_they_were_left() {
    let (mut app, ctx) = app(&["/left", "/right"]);
    let (left, right) = (app.panes[0].id, app.panes[1].id);
    // A second tab in the left pane, a third pane under the right one, and the focus
    // somewhere that is not the first pane — so that every part of what is written down
    // has something to say.
    app.perform(&ctx, Action::NewTab { pane: left });
    app.perform(
        &ctx,
        Action::Navigate {
            pane: left,
            path: PathBuf::from("/left/deeper"),
        },
    );
    // To the *left* of the right-hand pane, deliberately: the new pane goes in front of it
    // in the tree while being pushed to the back of `panes`, so layout order and the order
    // the panes happen to sit in the vector are no longer the same list. Written in the
    // wrong one of those two, everything below still passes.
    app.perform(
        &ctx,
        Action::OpenInSplit {
            pane: right,
            path: PathBuf::from("/under"),
            side: Side::Left,
        },
    );
    app.perform(&ctx, Action::Focus(right));
    *app.layout.ratio_at(&[]).unwrap() = 0.4;

    let order = tree(&app);
    assert_eq!(order.len(), 3, "three panes to write down");
    assert_ne!(
        order,
        app.panes.iter().map(|p| p.id).collect::<Vec<_>>(),
        "this test is only worth running while the two orders differ"
    );
    let titles: Vec<Vec<String>> = order.iter().map(|id| self::titles(&app, *id)).collect();
    let had_focus = order.iter().position(|id| *id == right).expect("in the tree");
    let shape = app.layout.encode();

    // Through the text of the file, not merely through the struct: the numbering is the
    // part that can go wrong, and it only exists in the text.
    let reopened = Config::parse(&app.settings().to_text());
    let back = App::opening(&ctx, reopened, Vec::new(), Side::Right);

    let recovered = tree(&back);
    assert_eq!(recovered.len(), order.len(), "a pane went missing");
    assert_eq!(back.layout.encode(), shape, "the same shape, with the same ratios");
    assert_eq!(
        recovered
            .iter()
            .map(|id| self::titles(&back, *id))
            .collect::<Vec<_>>(),
        titles,
        "the folders came back in the wrong panes"
    );
    assert_eq!(
        back.focused, recovered[had_focus],
        "the pane that had the keyboard has to be the one that gets it"
    );
    assert_eq!(
        back.panes
            .iter()
            .find(|p| p.id == recovered[0])
            .map(|p| p.active),
        Some(1),
        "and the tab that was in front stays in front"
    );
}

/// A settings file from before panes were remembered still opens every tab it names.
#[test]
fn remembered_tabs_with_no_layout_reopen_in_one_pane() {
    let ctx = egui::Context::default();
    let config = Config::parse("path=/a\npath=/b\npath=/c\n");
    let app = App::opening(&ctx, config, Vec::new(), Side::Right);
    assert_eq!(app.panes.len(), 1, "there is no layout to build");
    assert_eq!(app.panes[0].tabs.len(), 3, "but every tab is still opened");
}

/// And a layout that does not describe the panes beside it is not half-applied.
#[test]
fn a_layout_that_does_not_fit_its_panes_opens_plainly() {
    let ctx = egui::Context::default();
    // Two panes named, three in the tree.
    let config = Config::parse("layout=h0.5(0,v0.5(1,2))\npane=0\npath=/a\npane=0\npath=/b\n");
    let app = App::opening(&ctx, config, Vec::new(), Side::Right);
    assert_eq!(app.panes.len(), 1);
    assert_eq!(
        app.panes[0].tabs.len(),
        2,
        "the folders that were open have to open, whatever the tree said"
    );
}

#[test]
fn a_tab_dragged_to_another_pane_changes_hands() {
    let (mut app, ctx) = app(&["/left", "/right"]);
    let (left, right) = (app.panes[0].id, app.panes[1].id);
    app.perform(&ctx, Action::NewTab { pane: left });
    assert_eq!(titles(&app, left).len(), 2);

    app.perform(
        &ctx,
        Action::MoveTab {
            from: left,
            tab: 0,
            to: right,
            index: 0,
        },
    );
    assert_eq!(titles(&app, left).len(), 1);
    assert_eq!(titles(&app, right), ["left", "right"]);
    assert_eq!(app.focused, right, "the tab you moved is the one you wanted");
}

#[test]
fn moving_a_panes_last_tab_away_collapses_the_split() {
    let (mut app, ctx) = app(&["/left", "/right"]);
    let (left, right) = (app.panes[0].id, app.panes[1].id);

    app.perform(
        &ctx,
        Action::MoveTab {
            from: left,
            tab: 0,
            to: right,
            index: usize::MAX,
        },
    );
    assert_eq!(app.panes.len(), 1, "the emptied pane goes");
    assert_eq!(tree(&app), [right], "and so does its half of the tree");
    assert_eq!(titles(&app, right), ["right", "left"]);
}

#[test]
fn reordering_inside_a_strip_accounts_for_the_gap_left_behind() {
    let (mut app, ctx) = app(&["/a"]);
    let pane = app.panes[0].id;
    // Three tabs: a, a, a -- retitled so the order is checkable.
    app.perform(&ctx, Action::NewTab { pane });
    app.perform(&ctx, Action::NewTab { pane });
    for (index, name) in ["one", "two", "three"].into_iter().enumerate() {
        app.panes[0].tabs[index].title = name.to_owned();
    }

    // Drop the first tab where the third one starts: it lands between two and
    // three, because removing it shifted everything after it down.
    app.perform(
        &ctx,
        Action::MoveTab {
            from: pane,
            tab: 0,
            to: pane,
            index: 2,
        },
    );
    assert_eq!(titles(&app, pane), ["two", "one", "three"]);
    assert_eq!(app.panes[0].active, 1, "the moved tab stays the active one");
}

#[test]
fn appending_to_a_strip_puts_the_tab_last() {
    let (mut app, ctx) = app(&["/a"]);
    let pane = app.panes[0].id;
    app.perform(&ctx, Action::NewTab { pane });
    app.panes[0].tabs[0].title = "one".to_owned();
    app.panes[0].tabs[1].title = "two".to_owned();

    app.perform(
        &ctx,
        Action::MoveTab {
            from: pane,
            tab: 0,
            to: pane,
            index: usize::MAX,
        },
    );
    assert_eq!(titles(&app, pane), ["two", "one"]);
}

#[test]
fn dropping_a_tab_on_a_pane_edge_splits_it() {
    let (mut app, ctx) = app(&["/a"]);
    let pane = app.panes[0].id;
    app.perform(&ctx, Action::NewTab { pane });

    app.perform(
        &ctx,
        Action::SplitTab {
            from: pane,
            tab: 1,
            target: pane,
            side: Side::Right,
        },
    );
    assert_eq!(app.panes.len(), 2);
    assert_eq!(app.layout.count(), 2);
    let new = tree(&app)[1];
    assert_eq!(new, app.focused, "focus follows the tab you pulled out");
    assert_eq!(titles(&app, pane).len(), 1);
    assert_eq!(titles(&app, new).len(), 1);
}

#[test]
fn closing_the_last_tab_of_a_split_pane_removes_the_pane() {
    let (mut app, ctx) = app(&["/left", "/right"]);
    let (left, right) = (app.panes[0].id, app.panes[1].id);

    app.perform(&ctx, Action::CloseTab { pane: left, tab: 0 });
    assert_eq!(app.panes.len(), 1);
    assert_eq!(tree(&app), [right]);
    assert_eq!(app.focused, right, "focus cannot stay on a pane that is gone");
}

#[test]
fn the_last_tab_of_the_last_pane_is_the_window() {
    let (mut app, ctx) = app(&["/only"]);
    let pane = app.panes[0].id;
    app.perform(&ctx, Action::CloseTab { pane, tab: 0 });
    // Nothing is torn down -- the close is a viewport command, and the state has
    // to stay coherent for however many frames it takes to arrive.
    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.panes[0].tabs.len(), 1);
}

#[test]
fn a_new_tab_points_where_the_old_one_did() {
    let (mut app, ctx) = app(&["/somewhere/deep"]);
    let pane = app.panes[0].id;
    app.perform(&ctx, Action::NewTab { pane });
    assert_eq!(app.panes[0].tabs[0].path, app.panes[0].tabs[1].path);
    assert_eq!(app.panes[0].active, 1);
}

#[test]
fn a_closed_tab_comes_back_with_nothing_behind_it() {
    let (mut app, ctx) = app(&["/one"]);
    let pane = app.panes[0].id;
    app.perform(
        &ctx,
        Action::NavigateNewTab {
            pane,
            path: PathBuf::from("/two"),
        },
    );
    // Somewhere for it to have come *from*, so that the assertion below is about a history
    // being dropped rather than about a tab that never had one.
    app.perform(
        &ctx,
        Action::Navigate {
            pane,
            path: PathBuf::from("/two/deep"),
        },
    );
    assert_eq!(app.panes[0].tabs[1].history.len(), 2);

    app.perform(&ctx, Action::CloseTab { pane, tab: 1 });
    assert_eq!(app.panes[0].tabs.len(), 1);

    app.perform(&ctx, Action::ReopenTab);
    let back = app.panes[0].tabs.last().expect("the tab that came back");
    assert_eq!(
        back.path,
        PathBuf::from("/two/deep"),
        "the folder it was showing"
    );
    assert_eq!(
        back.history,
        [PathBuf::from("/two/deep")],
        "the path and nothing else -- not the trail it got there by"
    );
    assert_eq!(back.at, 0);
    assert!(!back.can_go_back());
    assert!(app.closed.is_empty(), "and it is spent, not repeatable");
}

#[test]
fn only_the_last_ten_closed_tabs_are_kept() {
    let (mut app, ctx) = app(&["/keep"]);
    let pane = app.panes[0].id;
    let path = |n: usize| PathBuf::from(format!("/gone/{n}"));

    // Twelve opened and closed, so the two oldest fall off the back.
    for n in 0..12 {
        app.perform(&ctx, Action::NavigateNewTab { pane, path: path(n) });
        app.perform(&ctx, Action::CloseTab { pane, tab: 1 });
    }
    assert_eq!(app.closed.len(), CLOSED_TABS);
    assert_eq!(app.closed.first(), Some(&path(2)), "0 and 1 are gone");

    // Two more presses than there is anything to answer them with.
    for _ in 0..CLOSED_TABS + 2 {
        app.perform(&ctx, Action::ReopenTab);
    }
    let back: Vec<PathBuf> = app.panes[0].tabs[1..].iter().map(|t| t.path.clone()).collect();
    let expected: Vec<PathBuf> = (2..12).rev().map(path).collect();
    assert_eq!(
        back, expected,
        "most recently closed comes back first, and nothing older than ten comes back at all"
    );
}

#[test]
fn reopening_with_nothing_closed_does_nothing() {
    let (mut app, ctx) = app(&["/only"]);
    app.perform(&ctx, Action::ReopenTab);
    assert_eq!(app.panes[0].tabs.len(), 1);
}

#[test]
fn the_close_that_is_the_window_is_not_remembered() {
    // That close is the window going, and the history goes with it. Recording it would put
    // the folder back into a window that is on its way out.
    let (mut app, ctx) = app(&["/only"]);
    let pane = app.panes[0].id;
    app.perform(&ctx, Action::CloseTab { pane, tab: 0 });
    assert!(app.closed.is_empty());
}

#[test]
fn a_tab_pulled_out_into_a_pane_of_its_own_was_not_closed() {
    let (mut app, ctx) = app(&["/a"]);
    let pane = app.panes[0].id;
    app.perform(
        &ctx,
        Action::NavigateNewTab {
            pane,
            path: PathBuf::from("/b"),
        },
    );
    app.perform(
        &ctx,
        Action::SplitTab {
            from: pane,
            tab: 1,
            target: pane,
            side: Side::Right,
        },
    );
    assert_eq!(app.panes.len(), 2);
    assert!(
        app.closed.is_empty(),
        "a tab that moved somewhere else was never closed"
    );
}

#[test]
fn bookmarks_toggle_and_never_pin_this_pc() {
    let (mut app, ctx) = app(&["/a"]);
    let path = PathBuf::from("/a");
    app.perform(&ctx, Action::ToggleBookmark(path.clone()));
    assert!(app.is_bookmarked(&path));
    app.perform(&ctx, Action::ToggleBookmark(path.clone()));
    assert!(!app.is_bookmarked(&path));

    app.perform(&ctx, Action::ToggleBookmark(PathBuf::new()));
    assert!(app.bookmarks.is_empty(), "This PC is not a folder to pin");
}

/// Windows' `Pin to Quick access` pins *here*, to the sidebar in front of the user, and never
/// reaches the shell.
///
/// By verb and not by label, because the entry reads `Épingler à l'accès rapide` on this
/// machine and `Pin to Quick access` on an English one. Everything else in the menu still
/// belongs to the shell, which is what the last case holds down: an empty answer here is what
/// sends a command on to [`crate::shell::Modal`], so a rule that matched too much would take
/// entries away from Windows rather than adding one to this program.
#[test]
fn pin_to_quick_access_bookmarks_here_instead() {
    use crate::shell::menu::Command;

    // Real directories, because the rule only pins folders and asks the disk which is which.
    let dir = crate::sandbox::dir("pin-verb");
    let sub = dir.join("inner");
    std::fs::create_dir_all(&sub).expect("a folder to pin");
    let file = dir.join("one.txt");
    std::fs::write(&file, b"x").expect("a file that is not one");

    let shell = |verb: &str| Command::Shell {
        verb: Some(verb.to_owned()),
        id: 0,
        path: Vec::new(),
        label: "whatever Windows calls it".to_owned(),
    };
    let menu = |items: Vec<PathBuf>| {
        crate::ui::menu::Open::new(
            1,
            pos2(0.0, 0.0),
            items,
            dir.clone(),
            Vec::new(),
            crate::shell::menu::Depth::Full,
            0,
        )
    };

    // A selected folder.
    let on_selection = menu(vec![sub.clone()]);
    match App::pin_is_a_bookmark(&on_selection, &shell("pintohome")).as_slice() {
        [Action::AddBookmark(path)] => assert_eq!(*path, sub),
        other => panic!("`pintohome` on a folder gave {:?}", names(other)),
    }
    match App::pin_is_a_bookmark(&on_selection, &shell("unpinfromhome")).as_slice() {
        [Action::RemoveBookmark(path)] => assert_eq!(*path, sub),
        other => panic!("`unpinfromhome` on a folder gave {:?}", names(other)),
    }

    // Nothing selected is the folder the menu was raised in.
    match App::pin_is_a_bookmark(&menu(Vec::new()), &shell("pintohome")).as_slice() {
        [Action::AddBookmark(path)] => assert_eq!(*path, dir),
        other => panic!("`pintohome` on a background menu gave {:?}", names(other)),
    }

    // Several folders at once, which is what the shell would have pinned.
    let both = menu(vec![sub.clone(), dir.clone()]);
    assert_eq!(App::pin_is_a_bookmark(&both, &shell("pintohome")).len(), 2);

    // A file is not a bookmark, so the shell keeps it.
    let on_file = menu(vec![file.clone()]);
    assert!(
        App::pin_is_a_bookmark(&on_file, &shell("pintohome")).is_empty(),
        "a file was turned into a sidebar entry"
    );

    // And every other verb in the menu is still Windows'.
    for verb in ["open", "copy", "delete", "properties", "pintohomefile", "PinToStartScreen"] {
        assert!(
            App::pin_is_a_bookmark(&on_selection, &shell(verb)).is_empty(),
            "`{verb}` was taken off the shell"
        );
    }
    assert!(
        App::pin_is_a_bookmark(&on_selection, &Command::Own(crate::shell::menu::Own::CopyHere))
            .is_empty()
    );

    crate::sandbox::remove(&dir);
}

/// Windows' Open, Couper, Copier and Coller act *here*, and everything else in the menu is
/// still the shell's.
///
/// By verb throughout, for the reason on [`App::ours_rather_than_the_shell_s`]: the labels are
/// in whatever language this Windows is in. What the shell offers under each of those verbs is
/// held down separately, by
/// `shell::menu::tests::the_verbs_this_program_takes_over_are_still_the_shell_s` — this test is
/// the other half, that the verbs turn into the right actions once they arrive.
///
/// The last two cases are the ones worth having: an empty answer is what sends a command on to
/// [`crate::shell::Modal`], so a rule that matched too much would take entries *away* from
/// Windows, which is the opposite of the point.
#[test]
fn the_shell_s_open_cut_copy_and_paste_act_in_this_explorer() {
    use crate::shell::menu::Command;

    // Real files and folders: the rules ask the disk which is which.
    let dir = crate::sandbox::fresh("redirected-verbs");
    let sub = dir.join("inner");
    let other = dir.join("second");
    std::fs::create_dir_all(&sub).expect("a folder");
    std::fs::create_dir_all(&other).expect("another folder");
    let file = dir.join("one.txt");
    std::fs::write(&file, b"x").expect("a file");

    let shell = |verb: &str| Command::Shell {
        verb: Some(verb.to_owned()),
        id: 0,
        path: Vec::new(),
        label: "whatever Windows calls it".to_owned(),
    };
    let menu = |items: Vec<PathBuf>| {
        crate::ui::menu::Open::new(
            7,
            pos2(0.0, 0.0),
            items,
            dir.clone(),
            Vec::new(),
            crate::shell::menu::Depth::Full,
            0,
        )
    };
    let ours = |items: Vec<PathBuf>, verb: &str| {
        App::ours_rather_than_the_shell_s(&menu(items), &shell(verb))
    };

    // Open on one folder navigates the pane the menu came from -- 7, not the focused one.
    match ours(vec![sub.clone()], "open").as_slice() {
        [Action::Navigate { pane, path }] => {
            assert_eq!(*pane, 7, "Open navigated a pane the menu was not raised in");
            assert_eq!(*path, sub);
        }
        other => panic!("Open on a folder gave {:?}", names(other)),
    }

    // Several folders get a tab each instead, leaving the pane where it is.
    match ours(vec![sub.clone(), other.clone()], "open").as_slice() {
        [
            Action::NavigateNewTab { path: first, .. },
            Action::NavigateNewTab { path: second, .. },
        ] => {
            assert_eq!(*first, sub);
            assert_eq!(*second, other);
        }
        got => panic!("Open on two folders gave {:?}", names(got)),
    }

    // A shortcut to a folder is a place too, and leads to the folder rather than to the `.lnk`.
    #[cfg(windows)]
    {
        let link = dir.join("to-inner.lnk");
        assert!(
            crate::shell::links::write_shortcut(&link, &sub, ""),
            "could not write the shortcut this case is about"
        );
        match ours(vec![link.clone()], "open").as_slice() {
            [Action::Navigate { path, .. }] => assert_eq!(
                *path, sub,
                "Open on a folder shortcut did not follow it -- so it would have opened \
                 Explorer"
            ),
            got => panic!("Open on a folder shortcut gave {:?}", names(got)),
        }
    }

    // A file is the shell's: its `open` is the registered default verb, and this program has
    // nowhere to show a file anyway.
    for (what, items) in [
        ("one file", vec![file.clone()]),
        ("two files", vec![file.clone(), file.clone()]),
        ("the background", Vec::new()),
    ] {
        assert!(
            ours(items, "open").is_empty(),
            "Open on {what} was taken off the shell"
        );
    }

    // A folder and a file together: the folder here, the file the way `Enter` opens one. The
    // gesture is one `InvokeCommand` for the whole selection, so half of it cannot be left to
    // Windows -- and leaving all of it would open the folder in Explorer.
    match ours(vec![sub.clone(), file.clone()], "open").as_slice() {
        [Action::Navigate { path, .. }, Action::Open(opened)] => {
            assert_eq!(*path, sub);
            assert_eq!(*opened, file);
        }
        got => panic!("Open on a folder and a file gave {:?}", names(got)),
    }

    // Cut and Copy carry the selection the menu was raised over.
    match ours(vec![sub.clone(), file.clone()], "cut").as_slice() {
        [Action::CutItems(items)] => assert_eq!(*items, vec![sub.clone(), file.clone()]),
        got => panic!("Cut gave {:?}", names(got)),
    }
    match ours(vec![file.clone()], "copy").as_slice() {
        [Action::CopyItems(items)] => assert_eq!(*items, vec![file.clone()]),
        got => panic!("Copy gave {:?}", names(got)),
    }

    // Paste means into the selected folder, not into the folder the pane is showing.
    match ours(vec![sub.clone()], "paste").as_slice() {
        [Action::PasteIntoFolder(into)] => assert_eq!(*into, sub),
        got => panic!("Paste on a folder gave {:?}", names(got)),
    }
    // The shell offers it on any selection with a folder somewhere in it. The first folder is
    // the one; the file beside it is not pasted into.
    match ours(vec![file.clone(), sub.clone()], "paste").as_slice() {
        [Action::PasteIntoFolder(into)] => assert_eq!(*into, sub),
        got => panic!("Paste on a file and a folder gave {:?}", names(got)),
    }
    assert!(
        ours(vec![file.clone()], "paste").is_empty(),
        "Paste was answered for a selection with no folder in it to paste into"
    );

    // Nothing selected is the folder's background menu, which carries none of the three --
    // Explorer synthesises its own Paste there and this program has Ctrl+V. A shell that grew
    // one would fall through to it rather than being answered against no selection at all.
    for verb in ["cut", "copy", "paste"] {
        assert!(
            ours(Vec::new(), verb).is_empty(),
            "`{verb}` on a background menu was answered with no selection to act on"
        );
    }

    // Pinning still comes through here, unchanged.
    match ours(vec![sub.clone()], "pintohome").as_slice() {
        [Action::AddBookmark(path)] => assert_eq!(*path, sub),
        got => panic!("`pintohome` gave {:?}", names(got)),
    }

    // And the whole rest of the menu is Windows'. `openas`, `opennew` and `opencontaining`
    // are here because they *start with* the verb that is hooked, which is the mistake a
    // `starts_with` would make.
    for verb in [
        "delete", "rename", "properties", "link", "copyaspath", "edit", "print", "runas",
        "openas", "opennew", "opencontaining", "pintohomefile", "PinToStartScreen",
        "NewFolder", "ShareX", "{6A1F6B13-3B82-48A1-9E06-7BB0A6D0BFFD}",
    ] {
        assert!(
            ours(vec![sub.clone()], verb).is_empty(),
            "`{verb}` was taken off the shell"
        );
    }
    // An empty verb matches nothing rather than falling into a hook.
    assert!(ours(vec![sub.clone()], "").is_empty());
    // And an entry the shell gave no canonical verb for at all is the shell's too: there is
    // nothing to recognise it by, and guessing from the label is what none of this does. The
    // label here is the one a French Windows puts on `copy`, which is the mistake being ruled
    // out.
    assert!(App::ours_rather_than_the_shell_s(
        &menu(vec![sub.clone()]),
        &Command::Shell {
            verb: None,
            id: 25,
            path: Vec::new(),
            label: "Copier".to_owned(),
        },
    )
    .is_empty(), "an entry with no verb was matched on its label");

    crate::sandbox::remove(&dir);
}

/// This program's Paste on empty space pastes into the folder the menu was raised in.
///
/// The counterpart of the redirected `paste` verb, which is the one on a *selected* folder. Both
/// end at [`Action::PasteIntoFolder`] so that the two entries and Ctrl+V are one paste — see
/// [`crate::shell::menu::Own::Paste`] for why this one has to be ours at all.
#[test]
fn our_paste_on_empty_space_pastes_into_the_folder_being_shown() {
    let dir = crate::sandbox::dir("own-paste");
    let (app, _ctx) = app(&["/a"]);
    // A background menu: nothing selected, raised in `dir`.
    let background = crate::ui::menu::Open::new(
        1,
        pos2(0.0, 0.0),
        Vec::new(),
        dir.clone(),
        Vec::new(),
        crate::shell::menu::Depth::Full,
        0,
    );
    match app.own_menu_action(&background, crate::shell::menu::Own::Paste) {
        Some(Action::PasteIntoFolder(into)) => assert_eq!(
            into, dir,
            "Paste went somewhere other than the folder the menu was raised in"
        ),
        other => panic!(
            "Paste gave {:?}",
            other.as_ref().map(Action::name)
        ),
    }
    // And Cancel is still the one own entry that means "do nothing", so a menu dismissed
    // through it does not paste.
    assert!(
        app.own_menu_action(&background, crate::shell::menu::Own::Cancel)
            .is_none()
    );
}

/// `Copy path(s)` copies what the menu was raised over, one path per line, with the slash the path
/// field is set to write.
///
/// Three claims. The *selection the menu was raised over* rather than the pane's, because a right
/// click can be about a row the keyboard is not on. The folder being shown when nothing is selected,
/// which is `Ctrl+Shift+C`'s answer and [`App::pin_is_a_bookmark`]'s. And the slash, which is the
/// reason the entry is this program's at all — see [`crate::shell::menu::Own::CopyPaths`].
///
/// Driven through [`Action::CopyPaths`] — the action `Ctrl+Shift+C` pushes — so what this holds down
/// is held down for both of them at once.
#[test]
fn copy_paths_writes_the_paths_with_the_slash_the_path_field_is_set_to() {
    use crate::shell::menu::Own;

    let (mut app, ctx) = app(&["/a"]);
    let folder = PathBuf::from(r"C:\src\ui");
    let menu = |items: Vec<PathBuf>| {
        crate::ui::menu::Open::new(
            1,
            pos2(0.0, 0.0),
            items,
            folder.clone(),
            Vec::new(),
            crate::shell::menu::Depth::Full,
            0,
        )
    };
    // What the last action put on the clipboard, and drained so the next assertion cannot pass on
    // the answer to the previous one. `copy_text` is an output command rather than a syscall, so
    // nothing here goes near the desktop's own clipboard.
    let copied = |ctx: &egui::Context| -> Option<String> {
        ctx.output_mut(|o| {
            let text = o.commands.iter().rev().find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            });
            o.commands.clear();
            text
        })
    };
    let copy = |app: &mut App, items: Vec<PathBuf>| -> Option<String> {
        let action = app
            .own_menu_action(&menu(items), Own::CopyPaths)
            .expect("the entry has to mean something");
        assert_eq!(action.name(), "CopyPaths");
        app.perform(&ctx, action);
        copied(&ctx)
    };

    let two = vec![folder.join("menu.rs"), folder.join("mod.rs")];
    assert_eq!(
        copy(&mut app, two.clone()).as_deref(),
        Some("C:\\src\\ui\\menu.rs\r\nC:\\src\\ui\\mod.rs"),
        "two paths, one per line, with the separator a fresh profile writes"
    );

    // Nothing selected is the folder the menu was raised in.
    assert_eq!(copy(&mut app, Vec::new()).as_deref(), Some(r"C:\src\ui"));

    // And with the path field set to `/`, which is what that setting is for. The line break between
    // the paths is not a separator and is left alone.
    app.forward_slashes = true;
    assert_eq!(
        copy(&mut app, two).as_deref(),
        Some("C:/src/ui/menu.rs\r\nC:/src/ui/mod.rs")
    );
    assert_eq!(copy(&mut app, Vec::new()).as_deref(), Some("C:/src/ui"));
}

/// Showing hidden files is one answer for the window, and it is written down.
///
/// It was one answer per *tab* and remembered nowhere, which made `Ctrl+H` a keystroke you pressed
/// again every launch. Both halves of the fix are here, and the first is what makes the second
/// possible: two panes free to disagree have no single state for a settings file to hold.
///
/// The new-tab case is [`crate::pane::Tab::showing`]'s reason for existing — the note there is why a
/// tab cannot be handed this one a frame late the way it can `flat_mode`.
#[test]
fn showing_hidden_files_is_the_window_s_preference_and_survives_a_relaunch() {
    let (mut app, ctx) = app(&["/a", "/b"]);
    assert!(!app.show_hidden, "a fresh profile leaves them out of the way");
    assert!(app.panes.iter().flat_map(|p| &p.tabs).all(|t| !t.show_hidden));

    app.perform(&ctx, Action::ToggleHidden);
    assert!(app.show_hidden);
    assert!(
        app.panes.iter().flat_map(|p| &p.tabs).all(|t| t.show_hidden),
        "a pane was left disagreeing with the preference that is about to be written"
    );
    assert!(app.config_dirty, "the toggle was never going to reach the file");
    assert!(app.settings().show_hidden);

    // A tab opened afterwards agrees with it, rather than being the one listing in the window with
    // half its rows missing.
    app.perform(
        &ctx,
        Action::NavigateNewTab {
            pane: 1,
            path: PathBuf::from("/c"),
        },
    );
    assert!(
        app.panes[0].tabs.last().expect("the new tab").show_hidden,
        "a tab made after the toggle came up hiding them"
    );

    // And the window those settings describe comes back showing them — every tab of it, since that
    // is what the one line in the file means.
    let back = App::opening(&ctx, app.settings(), Vec::new(), Side::Right);
    assert!(back.show_hidden);
    assert!(
        back.panes.iter().flat_map(|p| &p.tabs).all(|t| t.show_hidden),
        "the restored tabs are ordered without the setting the window was launched with"
    );

    app.perform(&ctx, Action::ToggleHidden);
    assert!(!app.show_hidden);
    assert!(!app.settings().show_hidden);
    assert!(app.panes.iter().flat_map(|p| &p.tabs).all(|t| !t.show_hidden));
}

/// The join: the entry the shell really puts in the menu, through the real dispatch.
///
/// Everything either side of this is covered on its own —
/// `shell::menu::tests::the_verbs_this_program_takes_over_are_still_the_shell_s` holds the shell
/// to the four verb names, and
/// [`the_shell_s_open_cut_copy_and_paste_act_in_this_explorer`] holds the dispatch to the right
/// actions — but both halves pass while the two are wired to different strings. So this one
/// hands over a `Command` that was read out of an `HMENU` rather than one written here, and
/// nothing about it is synthesised except which pane it came from.
#[test]
#[cfg(windows)]
fn the_real_open_entry_from_the_real_menu_navigates_here() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let dir = crate::sandbox::fresh("real-open-entry");
    let sub = dir.join("inner");
    std::fs::create_dir_all(&sub).expect("a folder to right-click");

    let entries = crate::shell::menu::build(&dir, std::slice::from_ref(&sub));
    assert!(
        !entries.is_empty(),
        "the shell gave no menu at all for a folder, so this test proves nothing"
    );
    let open = entries
        .iter()
        .find_map(|entry| match &entry.kind {
            crate::shell::menu::Kind::Command(
                command @ crate::shell::menu::Command::Shell { verb: Some(verb), .. },
            ) if verb == "open" => Some(command.clone()),
            _ => None,
        })
        .expect("the shell's menu for a folder has an Open in it");

    let menu = crate::ui::menu::Open::new(
        3,
        pos2(0.0, 0.0),
        vec![sub.clone()],
        dir.clone(),
        entries,
        crate::shell::menu::Depth::Full,
        0,
    );
    match App::ours_rather_than_the_shell_s(&menu, &open).as_slice() {
        [Action::Navigate { pane, path }] => {
            assert_eq!(*pane, 3);
            assert_eq!(
                *path, sub,
                "the shell's own Open on a folder has to navigate this pane; anything else \
                 opens a second file manager over the top of this one"
            );
        }
        got => panic!(
            "the shell's real Open entry gave {:?} instead of navigating here",
            names(got)
        ),
    }

    crate::sandbox::remove(&dir);
}

/// Action names, for a panic message that says which ones came back.
#[cfg(test)]
fn names(actions: &[Action]) -> Vec<&'static str> {
    actions.iter().map(Action::name).collect()
}

#[test]
fn split_focused_opens_the_folder_already_showing() {
    let (mut app, ctx) = app(&["/here"]);
    app.perform(&ctx, Action::SplitFocused { side: Side::Bottom });
    assert_eq!(app.panes.len(), 2);
    assert_eq!(app.panes[1].tab().path, PathBuf::from("/here"));
}
