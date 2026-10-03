//! The tiles view: where the arrows go, and what a mode change does to the scroll.

use super::*;

/// In the grid, `Down` goes to the tile below and `Right` to the one beside it.
///
/// **Which is a different number of rows for each key**, and getting it wrong is a listing where
/// the arrow keys walk the folder in an order that has nothing to do with what is on screen. So
/// the step is measured against the column count the view actually laid out — `Layout::columns` —
/// rather than against a number restated here, and the pane is deliberately left at the harness's
/// own width so that more than one column fits.
///
/// `Home` and `End` are checked as well, because they are the two that must *not* change: the
/// first tile and the last are still the first and last rows of the order however it is arranged.
#[test]
fn in_the_grid_the_arrows_move_by_a_line_of_tiles() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    // `src`, which has enough files to fill more than one line of tiles.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(&ctx, Action::Navigate { pane, path });
    h.settle();
    h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
    h.settle();

    let columns = h.tab(0).grid.columns;
    assert!(
        columns > 1,
        "the harness's pane fits only {columns} column, so this test cannot tell a line from a row"
    );
    assert!(h.tab(0).order.len() > columns * 2, "and not enough rows to move through");

    h.frame(tap(egui::Key::Home));
    assert_eq!(h.tab(0).cursor, Some(0), "Home is the first tile");
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(h.tab(0).cursor, Some(1), "Right is the next tile along");
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(
        h.tab(0).cursor,
        Some(1 + columns),
        "Down should be a whole line of tiles"
    );
    h.frame(tap(egui::Key::ArrowUp));
    assert_eq!(h.tab(0).cursor, Some(1));
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(h.tab(0).cursor, Some(0));
    // And off the left edge of the first line is still the first tile rather than an underflow.
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(h.tab(0).cursor, Some(0));
    h.frame(tap(egui::Key::End));
    assert_eq!(h.tab(0).cursor, Some(h.tab(0).order.len() - 1), "End is the last");

    // Back in the details view the same two keys are one row and the tree's, which is the other
    // half of the rule.
    h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
    h.settle();
    h.frame(tap(egui::Key::Home));
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(h.tab(0).cursor, Some(1), "a row is one step in the details view");
}

/// **The arrows in a flattened tree of tiles, driven through the real keyboard.**
///
/// [`crate::ui::grid::tests::the_arrows_walk_the_pane_and_not_the_display_order`] is where the whole
/// table of where each key goes is pinned, against a layout at a chosen width. This is the wiring:
/// the keys arrive, the pane the keyboard is in is the one that moves, and `Ctrl` with `Home` or
/// `End` gets through to the listing rather than being eaten on the way.
///
/// **Every claim here is written to be free of the column count**, which is the harness's pane width
/// divided by a tile and is nobody's business but the layout's — so each folder in the fixture holds
/// exactly one file, and the answers are the same whether one tile fits across or six.
///
/// The fixture is where the point is. Flattened as a tree, the display order is `one`, `one\p.txt`,
/// `two`, `top.txt` — and the pane is the other way about: `top.txt` is drawn *first*, above every
/// folder row, because the listed folder's own files are. So `Ctrl+Home` landing on the first row of
/// the order is landing three quarters of the way down the pane, which is the bug this is for.
#[test]
fn the_arrows_in_a_tree_of_tiles_walk_what_is_on_the_pane() {
    use crate::pane::{FlatMode, ViewMode};

    let root = crate::sandbox::fresh("tree-tiles-arrows");
    std::fs::create_dir_all(root.join("one")).expect("the sandbox is writable");
    // Empty, and last: what `End` has to land on, and a folder rather than a file.
    std::fs::create_dir_all(root.join("two")).expect("the sandbox is writable");
    std::fs::write(root.join("top.txt"), b"x").expect("writable");
    std::fs::write(root.join(r"one\p.txt"), b"x").expect("writable");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    h.app.perform(&ctx, Action::Navigate { pane, path: root.clone() });
    h.settle();
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.app.perform(&ctx, Action::SetFlatMode(FlatMode::Tree));
    h.app.perform(&ctx, Action::SetView { pane, mode: ViewMode::Icons });
    h.settle();
    assert!(h.tab(0).is_tree(), "the listing is not a tree");
    assert_eq!(h.tab(0).view_mode, ViewMode::Icons, "and it is not tiles");
    assert_eq!(
        shown_names(&h, 0),
        vec!["one", r"one\p.txt", "two", "top.txt"],
        "the fixture is not the display order this test is about"
    );

    // Where the cursor is, by name — because a position means nothing here: the whole point is that
    // the pane's order and the display order are not each other.
    let under_the_cursor = |h: &Harness| -> String {
        let tab = h.tab(0);
        tab.cursor
            .and_then(|at| tab.entry_at(at))
            .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
            .unwrap_or_else(|| "nothing".to_owned())
    };
    // A key with `Ctrl` held, which has to be held on the input state as well as carried on the
    // event — `App::keyboard` reads the modifiers off the frame.
    let with_ctrl = |h: &mut Harness, key: egui::Key| {
        h.modifiers = Modifiers::COMMAND;
        h.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]);
        h.modifiers = Modifiers::NONE;
    };

    // ---- Ctrl+Home and Ctrl+End, the two ends of the pane ---------------
    with_ctrl(&mut h, egui::Key::Home);
    assert_eq!(
        under_the_cursor(&h),
        "top.txt",
        "Ctrl+Home is the first thing drawn, which here is a file three rows down the order"
    );
    with_ctrl(&mut h, egui::Key::End);
    assert_eq!(
        under_the_cursor(&h),
        "two",
        "Ctrl+End is the last thing drawn, which here is an empty folder's row"
    );

    // ---- And the four arrows, from the top down -------------------------
    with_ctrl(&mut h, egui::Key::Home);
    // Down off the listed folder's only file leaves its grid for the first folder row.
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(under_the_cursor(&h), "one", "down off the top grid");
    // **Down on a folder is what is inside it**, and not the next folder along.
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(under_the_cursor(&h), r"one\p.txt", "down on a folder");
    // Down on the last file of a branch is the next folder up the tree.
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(under_the_cursor(&h), "two", "down off the last file");
    // And nothing below it: the cursor stands still rather than jumping back into the order.
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(under_the_cursor(&h), "two", "down off the end of the pane");

    // Up is the same steps backwards.
    for expected in [r"one\p.txt", "one", "top.txt", "top.txt"] {
        h.frame(tap(egui::Key::ArrowUp));
        assert_eq!(under_the_cursor(&h), expected, "walking back up");
    }

    // ---- Left and Right: the tile beside it, and then the branch ---------
    //
    // On a folder's row the pair is the tree's *first*, and moves only when the branch has nothing to
    // answer with — which is what makes one press open a folder and the next go into it.
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(under_the_cursor(&h), "one", "right off the top grid");
    // **`one` is already open, so Right goes to what is inside it.**
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(under_the_cursor(&h), r"one\p.txt", "right on an open folder");
    // Off the end of that folder's files, which is the next row along.
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(under_the_cursor(&h), "two", "right off the last file");
    // **And on a folder with nothing in it, Right is the next folder** — except there is none after
    // `two`, so the cursor stands still rather than wrapping or falling into the order.
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(under_the_cursor(&h), "two", "right off the end of the pane");

    // **Left on an empty folder backs out on the first press**, because there is nothing to shut —
    // and what is before it on the pane is the last file of the folder above, which is open.
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(under_the_cursor(&h), r"one\p.txt", "left off an empty folder");
    assert_eq!(
        shown_names(&h, 0),
        vec!["one", r"one\p.txt", "two", "top.txt"],
        "Left on the empty folder shut something"
    );
    // Left on the first file of a folder is that folder.
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(under_the_cursor(&h), "one", "left off the first file");
    // On the open folder it shuts the branch and stays put, which is the tree answering.
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(under_the_cursor(&h), "one", "Left on an open folder moved the cursor");
    assert_eq!(
        shown_names(&h, 0),
        vec!["one", "two", "top.txt"],
        "Left on the folder's row did not shut it"
    );
    // **And now that it is shut, Left backs out of it** — to the place before it on the pane, which
    // here is the listed folder's own file.
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(under_the_cursor(&h), "top.txt", "left off a shut folder");
    // Right from there is the folder's row again, and the branch opens from it.
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(under_the_cursor(&h), "one");
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(
        shown_names(&h, 0),
        vec!["one", r"one\p.txt", "two", "top.txt"],
        "Right did not open it again"
    );
    assert_eq!(
        under_the_cursor(&h),
        "one",
        "the press that opened the branch also moved the cursor"
    );

    crate::sandbox::remove(&root);
}

/// **The page keys walk the folders, and `Ctrl` with them never goes deeper.**
///
/// [`crate::ui::grid::tests::the_page_keys_walk_the_folders_and_ctrl_never_goes_deeper`] pins the
/// whole table; this is the wiring, and in particular that `Ctrl` reaches the listing on these two
/// keys — nothing else in the window binds them, and a modifier eaten on the way would leave both
/// pairs doing the same thing.
///
/// Its own fixture, deeper than the one above, because the difference between the two pairs needs a
/// level to step over: from `a`, the next folder at any depth is `sub` *inside* it and the next one
/// no deeper is `z`. Nothing here depends on the column count — every answer is a folder's row.
#[test]
fn the_page_keys_in_a_tree_of_tiles_walk_the_folders() {
    use crate::pane::{FlatMode, ViewMode};

    let root = crate::sandbox::fresh("tree-tiles-pages");
    std::fs::create_dir_all(root.join(r"a\sub")).expect("the sandbox is writable");
    std::fs::create_dir_all(root.join("z")).expect("the sandbox is writable");
    std::fs::write(root.join("top.txt"), b"x").expect("writable");
    std::fs::write(root.join(r"a\a1.txt"), b"x").expect("writable");
    std::fs::write(root.join(r"a\sub\deep.txt"), b"x").expect("writable");
    std::fs::write(root.join(r"z\z1.txt"), b"x").expect("writable");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    h.app.perform(&ctx, Action::Navigate { pane, path: root.clone() });
    h.settle();
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.app.perform(&ctx, Action::SetFlatMode(FlatMode::Tree));
    h.app.perform(&ctx, Action::SetView { pane, mode: ViewMode::Icons });
    h.settle();
    assert_eq!(
        shown_names(&h, 0),
        vec![
            "a",
            r"a\sub",
            r"a\sub\deep.txt",
            r"a\a1.txt",
            "z",
            r"z\z1.txt",
            "top.txt"
        ],
        "the fixture is not the tree this test is about"
    );

    let under_the_cursor = |h: &Harness| -> String {
        let tab = h.tab(0);
        tab.cursor
            .and_then(|at| tab.entry_at(at))
            .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
            .unwrap_or_else(|| "nothing".to_owned())
    };
    let with_ctrl = |h: &mut Harness, key: egui::Key| {
        h.modifiers = Modifiers::COMMAND;
        h.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]);
        h.modifiers = Modifiers::NONE;
    };

    // ---- Plain: every folder, whatever its depth ------------------------
    with_ctrl(&mut h, egui::Key::Home);
    assert_eq!(under_the_cursor(&h), "top.txt", "the first place on the pane");
    for expected in ["a", r"a\sub", "z", "z"] {
        h.frame(tap(egui::Key::PageDown));
        assert_eq!(under_the_cursor(&h), expected, "PageDown through the folders");
    }

    // ---- With Ctrl: only what is no deeper than here ---------------------
    //
    // Back across from `z`, which steps over the whole of `a`'s branch — `a\sub` is deeper, so it is
    // not a place this pair stops at.
    with_ctrl(&mut h, egui::Key::PageUp);
    assert_eq!(under_the_cursor(&h), "a", "Ctrl+PageUp went into the branch");

    // From a *file* the level is the folder it is in, not the depth its own tile is drawn at. So
    // from `a\a1.txt` the plain key finds `a\sub` and the Ctrl one steps over it.
    h.frame(tap(egui::Key::ArrowRight));
    assert_eq!(under_the_cursor(&h), r"a\a1.txt", "right on an open folder");
    with_ctrl(&mut h, egui::Key::PageDown);
    assert_eq!(under_the_cursor(&h), "z", "Ctrl+PageDown from a file went deeper");
    with_ctrl(&mut h, egui::Key::PageUp);
    assert_eq!(under_the_cursor(&h), "a");
    h.frame(tap(egui::Key::ArrowRight));
    h.frame(tap(egui::Key::PageDown));
    assert_eq!(
        under_the_cursor(&h),
        r"a\sub",
        "the plain key should go into the branch the Ctrl one steps over"
    );

    // ---- And Left out of a shut folder, with a branch above it -----------
    //
    // The case the fixture above is too flat for: shut `z`, and what is before it on the pane is the
    // last file of `a\sub`, which is open.
    h.frame(tap(egui::Key::PageDown));
    assert_eq!(under_the_cursor(&h), "z");
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(under_the_cursor(&h), "z", "Left on an open folder moved the cursor");
    assert!(
        !shown_names(&h, 0).contains(&r"z\z1.txt".to_owned()),
        "Left did not shut `z`"
    );
    h.frame(tap(egui::Key::ArrowLeft));
    assert_eq!(
        under_the_cursor(&h),
        r"a\sub\deep.txt",
        "left off a shut folder is the last child of the folder above it"
    );

    crate::sandbox::remove(&root);
}

/// **The rule as a fresh profile has it, and the menu that turns it off and on again** — ticked,
/// dragged, and obeyed.
///
/// Five claims, and every one of them needs a driven frame.
///
/// **A folder of pictures opens as tiles out of the box**, because the rule is on by default — see
/// [`crate::pane::AutoTiles`]. That is the first assertion in the test and it is the one a change to
/// the default would break.
///
/// **The menu is reachable.** It hangs off an 18-point switch in a 22-point bar that the window's
/// bottom resize band overlaps, and that band is registered last — so the switch is swept for before
/// anything is asserted, exactly as `the_view_switch_is_reachable_and_its_tiles_can_be_clicked` does
/// for the left button. Swept while the switch is *latched*, too, since the folder came up as tiles.
///
/// **The slider inside it works.** A widget in a popup that cannot be dragged is the other failure
/// that reads perfectly correctly in the source: the rail is swept for too, because where a slider's
/// input row falls under its own header is arithmetic this test would only be restating.
///
/// **The tick changes nothing on screen**, which is the whole of [`crate::pane::AutoTiles`]'s
/// contract and the one thing about it somebody could reasonably expect to go the other way. The
/// folder in front of the menu is four fifths pictures and stays as it is either way it is ticked.
///
/// **And the next folder obeys whatever the menu was left saying.** Away and back rather than a
/// refresh, because those are two different things and only one of them is an opening — the round trip
/// also puts the judgement on the *cached* path, which is the one `Back`, `Forward` and a revisited
/// folder take.
#[test]
fn the_view_switchs_menu_decides_when_a_folder_opens_as_tiles() {
    // Zero-byte files with the right names: the rule reads the type off the extension — see
    // `fs::fmt::shows_a_picture` — so nothing here has to be a real picture.
    //
    // **The rows that must not count carry an extension nobody could have registered a thumbnail
    // provider for**, and that is not decoration. The count asks this machine about every type the
    // table will not claim, so a `.txt` here would make every figure below depend on what happens to
    // be installed where the suite is running. `.zzznotatype` is the same answer on every machine.
    let root = crate::sandbox::fresh("auto-tiles");
    let gallery = root.join("gallery");
    let sources = root.join("sources");
    for (dir, pictures) in [(&gallery, 8), (&sources, 2)] {
        std::fs::create_dir_all(dir).expect("the sandbox is writable");
        for i in 0..pictures {
            std::fs::write(dir.join(format!("shot-{i}.png")), b"").expect("writable");
        }
        for i in 0..(10 - pictures) {
            std::fs::write(dir.join(format!("notes-{i}.zzznotatype")), b"").expect("writable");
        }
    }

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    // Its own, rather than the window's: `h.tab(0)` borrows the app, and the cache is only a cache —
    // a fresh one gives the same answers about the same machine.
    let mut providers = crate::shell::providers::Providers::new();

    // A fresh profile, so the rule is on and the threshold is `TILES_THRESHOLD`.
    assert!(h.app.auto_tiles.on, "a fresh profile has the rule off");
    assert_eq!(h.app.auto_tiles.threshold, crate::pane::TILES_THRESHOLD);

    // The gallery, opened by a window nobody has configured: four fifths pictures, so **tiles**.
    h.app.perform(&ctx, Action::Navigate { pane, path: gallery.clone() });
    h.settle();
    assert_eq!(
        h.tab(0).picture_rows(&mut providers),
        (8, 10),
        "the fixture is not what this test needs"
    );
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Icons,
        "a folder of pictures did not open as tiles on a fresh profile"
    );

    // ---- The menu ------------------------------------------------------
    let rect = h.pane_rect(0);
    let bar = rect.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
    let id = Id::new(("view-switch", pane));
    let switch = (0..60)
        .step_by(2)
        .map(|dx| pos2(rect.left() + dx as f32, bar))
        .find(|at| h.hovers(id, *at))
        .expect("the view switch is not reachable along its own bar");
    h.click_with(switch, PointerButton::Secondary, Modifiers::NONE);

    let text_at = |h: &Harness, label: &str| {
        h.texts()
            .into_iter()
            .find(|(_, text)| text == label)
            .map(|(at, _)| at)
    };
    // Where a label has come to rest, once the menu has stopped moving. **The settling is not
    // politeness.** This menu changes height as its own settings are ticked — the probe adds a line
    // reporting what it cost, and unticking takes it away again — and it opens *above* the bar it
    // hangs off, so every row in it moves when it does. A position read from the frame before a
    // change is a position something else has moved into: as a click, one that quietly does nothing;
    // as a slider's rail, a drag over the wrong strip of the menu.
    let settled_at = |h: &mut Harness, label: &str| -> Pos2 {
        let mut settled = None;
        for _ in 0..8 {
            h.frame(Vec::new());
            let now = text_at(h, label);
            if now.is_some() && now == settled {
                break;
            }
            settled = now;
        }
        settled.unwrap_or_else(|| {
            panic!(
                "no `{label}` on the menu: {:?}",
                h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
            )
        })
    };
    let click_entry = |h: &mut Harness, label: &str| -> Vec<&'static str> {
        let at = settled_at(h, label);
        // A couple of points into the label, which is inside the entry whatever its padding is.
        h.click_at(pos2(at.x + 2.0, at.y + 6.0))
    };

    // **Off first**, since the rule is already on: the tick has to work in the direction somebody who
    // does not want it will press it, and that is the direction a `!` in the wrong place would break.
    const TICK: &str = "Automatically switch to thumbnail view";
    let done = click_entry(&mut h, TICK);
    assert!(
        done.contains(&"SetAutoTiles"),
        "clicking `{TICK}` did nothing, got {done:?}"
    );
    assert!(!h.app.auto_tiles.on, "the entry did not turn the rule off");
    // A setting, so it is part of what the window writes down. Asked of `settings()` rather than of
    // `config_dirty`, which the frame after the one that sets it has already cleared.
    assert!(!h.app.settings().auto_tiles.on, "and it was not written down");

    // **Nothing behind the menu moved.** The folder it was raised over is the one this rule picked up
    // a moment ago, and it is still tiles — because the rule is about *opening* a folder, so turning it
    // off cannot un-open one.
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Icons,
        "unticking the rule re-arranged the folder that was already on screen"
    );

    // And back on, which is both halves of a tick.
    let done = click_entry(&mut h, TICK);
    assert!(done.contains(&"SetAutoTiles"), "got {done:?}");
    assert!(h.app.auto_tiles.on, "the entry does not tick back on");
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Icons,
        "ticking it back on re-arranged the folder that was already on screen"
    );

    // **And it says what it makes of the folder it was raised over**, which is the instrument the
    // whole menu is there to be: eight of its ten rows are pictures and the other two are of a type
    // no machine can claim, so eighty percent on every machine.
    let reading = h
        .texts()
        .into_iter()
        .map(|(_, text)| text)
        .find(|text| text.starts_with("Here:"))
        .unwrap_or_else(|| {
            panic!(
                "the menu does not say what the rule makes of this folder: {:?}",
                h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
            )
        });
    assert_eq!(reading, "Here: 80% — 8 of 10 rows");

    // ---- The slider, swept for under its own header ---------------------
    //
    // Sticky, so the menu is still up after the tick — which is what makes this one gesture rather
    // than two, and is the reason the menu is sticky at all.
    const RAIL: &str = "Pictures in the folder";
    // Both read after the menu has settled where the ticks left it — see `settled_at`.
    let value = settled_at(&mut h, "60%");
    // The rail runs the width of the menu's *content*, which is where the slider's own label starts —
    // further left than the entries above it, whose labels are indented by the tick's gutter.
    let rail_left = settled_at(&mut h, RAIL).x;
    // Both labels fit the menu they are in. Worth an assertion because the menu is sized from what
    // its *entries* asked for — `MenuItem` is the only thing that feeds the width probe — so the
    // slider is along for the ride and a label a few points too long would come out as
    // `Pictures in the fol…` beside a number, which reads as a control that has been cut off.
    let cropped = h.cropped();
    for label in [TICK, RAIL] {
        assert!(
            !cropped.iter().any(|text| text == label),
            "`{label}` does not fit the menu it is in"
        );
    }

    let mut dragged = None;
    for dy in (6..34).step_by(2) {
        let y = value.y + dy as f32;
        // From the left end of the rail to past its right one, so the value can only come out at
        // the top of the range — a drag that happened to land where the value already was would
        // report nothing changed and prove nothing.
        // A few points in from the rail's left end rather than exactly on it: the menu's own frame
        // margin is not the slider, which is a press that lands on nothing — found the hard way
        // against the real window.
        let done = h.drag(pos2(rail_left + 8.0, y), pos2(value.x + 60.0, y));
        if done.contains(&"SetTilesThreshold") {
            dragged = Some(y);
            break;
        }
    }
    dragged.unwrap_or_else(|| {
        panic!(
            "no drag under the threshold's own header reached the slider: rail from {rail_left}, \
             value at {value:?}, menu {:?}",
            h.texts()
                .into_iter()
                .filter(|(_, text)| text.len() > 3)
                .rev()
                .take(6)
                .collect::<Vec<_>>()
        )
    });
    assert_eq!(
        h.app.auto_tiles.threshold, 100.0,
        "the rail was dragged to its right end and the threshold did not follow"
    );
    assert!(h.app.auto_tiles.on, "the sweep hit the entry above the slider");
    assert_eq!(h.app.settings().auto_tiles.threshold, 100.0, "and it was not written down");

    // At a hundred percent the gallery is *not* a folder of pictures — eight in ten — so the round
    // trip below has to leave it in the details view. Which is the slider being read at all.
    h.app.perform(&ctx, Action::Navigate { pane, path: sources.clone() });
    h.settle();
    h.app.perform(&ctx, Action::Navigate { pane, path: gallery.clone() });
    h.settle();
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Details,
        "four fifths pictures passed a threshold of all of them"
    );

    // ---- And at a threshold it does pass ------------------------------
    h.app.perform(&ctx, Action::SetTilesThreshold(60.0));
    h.app.perform(&ctx, Action::Navigate { pane, path: sources });
    h.settle();
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Details,
        "two pictures in ten opened as tiles at a threshold of sixty"
    );
    h.app.perform(&ctx, Action::Navigate { pane, path: gallery });
    h.settle();
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Icons,
        "a folder that is four fifths pictures did not open as tiles"
    );
    // And the switch is still what takes it back, with the rule leaving it alone from then on.
    h.app.perform(&ctx, Action::SetView { pane, mode: crate::pane::ViewMode::Details });
    h.app.panes[0].tab_mut().refresh();
    h.settle();
    assert_eq!(
        h.tab(0).view_mode,
        crate::pane::ViewMode::Details,
        "a refresh put the tiles back over somebody who had switched to rows"
    );

    crate::sandbox::remove(&root);
}

/// **A scrolled grid switched from one flatten mode to the other still gets its pictures.**
///
/// The bug this is for, and it is a *paint-on-demand* bug rather than a caching one. Scroll a grid
/// of tiles far enough and every cell of the thumbnail atlas is held by a file you have gone past.
/// Switch the flatten mode and the tiles are all new, all their cells are held by those files, and
/// the cells only become reusable once a frame has gone by without them being drawn. Nothing on
/// screen is moving, so nothing asks for that frame — and the window sits on a grid of painted
/// glyphs until you scroll and force some frames by hand. Which is exactly what it did.
///
/// So the shape of the test is the shape of the report: fill the atlas, then switch, then run frames
/// **without touching anything** and require the pictures to arrive. `Thumbs::poll` moving to the end
/// of the frame is what makes that possible, and the repaint booked on a refused request is what
/// makes it happen when the answer needs a frame that nobody else would ask for.
///
/// **What is counted is textured quads**, which in this view is exactly "tiles with a picture": a
/// tile that has one draws an image out of the atlas, and a tile that has not draws a painted glyph
/// out of the font atlas. So the first show establishes the number, and the number has to come back.
///
/// The folder is a few hundred `.txt` files in the sandbox rather than anything real. What matters
/// is the *count* — enough tiles to fill the atlas twice over between the two views — and `.txt` is
/// the cheapest thing the shell reliably draws: no thumbnail handler, so it answers from the icon
/// every time and the test does not depend on what is in the files.
///
/// **What this does and does not prove**, because that is worth being straight about. It holds the
/// end-to-end invariant: switch a scrolled grid's mode, touch nothing, and the pictures come back.
/// It does *not* discriminate against the version of the bug that was found, on this fixture and
/// this window — 420 files over a 2560-wide pane never fills the atlas, so there is no starvation
/// for the ordering to rescue, and the test passes against both. What discriminates is
/// `shell::thumbs`' own `a_visible_cell_is_never_taken_from_the_tile_drawing_it`, which fails
/// outright on the two-frame rule this replaced. This one is here for the *next* change: it is the
/// only test that runs the whole path with frames drawn only when the window asks for them.
///
/// `#[ignore]`d because it drives several hundred real shell calls and takes a few seconds, which is
/// not what `cargo test` is for.
#[test]
#[ignore = "drives a few hundred real shell calls; run explicitly"]
fn a_scrolled_grid_that_changes_mode_still_fills_in() {
    // Twenty folders of twenty files: enough rows that a scrolled tree and the top of a list have
    // nothing in common, which is the whole condition.
    let root = crate::sandbox::dir("tiles-refill");
    for folder in 0..20 {
        let sub = root.join(format!("f{folder:02}"));
        std::fs::create_dir_all(&sub).expect("the sandbox is writable");
        for file in 0..20 {
            let at = sub.join(format!("{folder:02}-{file:02}.txt"));
            if !at.exists() {
                std::fs::write(&at, b"x").expect("the sandbox is writable");
            }
        }
    }

    let mut h = Harness::with_panes(1);
    // Big enough to want a lot of tiles at once, which is the condition the report has.
    h.size = vec2(2560.0, 1392.0);
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();

    h.app.perform(&ctx, Action::Navigate { pane, path: root });
    h.settle();
    h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.settle();
    assert!(h.tab(0).order.len() > 400, "not enough rows to fill the atlas");

    // ---- The first show, which is the case that always worked ----------
    let first = h.pictures_once_settled();
    assert!(
        first > 50,
        "only {first} tiles drew a picture on the first show — this window is too small to \
         tell the bug from the arithmetic"
    );

    // ---- Fill the atlas with files the list will not be showing --------
    h.app.perform(&ctx, Action::SetFlatMode(crate::pane::FlatMode::Tree));
    h.settle();
    for step in 1..=8 {
        h.app.panes[0].tab_mut().scroll_to = Some(step as f32 * 1200.0);
        let _ = h.pictures_once_settled();
    }

    // ---- The switch back, and then **nothing but frames** --------------
    //
    // No pointer, no keys, no scrolling. This is the whole report: the window is left alone, and it
    // has to fill itself in.
    h.app.perform(&ctx, Action::SetFlatMode(crate::pane::FlatMode::List));
    let again = h.pictures_once_settled();
    assert_eq!(
        again, first,
        "{} of {first} tiles never got a picture back after the mode changed, without the view \
         being touched",
        first.saturating_sub(again)
    );
}
