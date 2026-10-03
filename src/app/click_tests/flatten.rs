//! Flattening a folder: the whole tree in one listing, as a list or as a tree.

use super::*;

/// Flatten: the button on the bar, the shortcut, and the listing each of them produces.
///
/// Driven through the real button and the real keyboard rather than through `perform`,
/// because the two things most likely to be wrong are the ones only that can see: a
/// button nothing can reach, and a shortcut that never arrives.
#[test]
fn flattening_a_folder_shows_its_whole_tree_and_turning_it_off_puts_it_back() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    // `src`, not the crate root — the root has `target` in it, and walking a few hundred
    // thousand build artefacts would prove nothing this does not.
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: sources,
        },
    );
    h.settle();
    assert!(
        shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
        "a folder's own listing is one level deep"
    );

    // ---- Where the button is ------------------------------------------
    let rect_of = |h: &Harness, id: Id| {
        h.ctx
            .read_response(id)
            .map(|r| r.rect)
            .unwrap_or_else(|| panic!("{id:?} was not laid out"))
    };
    let button = rect_of(&h, Id::new(("flatten", pane)));
    let bar = h.app.panes[0].rect;
    // Past the end of the path — it is one of the right-hand group, not part of the trail —
    // and with a filter box's worth of room still to its right, which is what "just before
    // the filter" comes to in geometry. 90 is the width below which `breadcrumb` gives up
    // drawing the field at all.
    let empty = rect_of(&h, Id::new(("crumb-empty", pane)));
    assert!(
        button.left() >= empty.right(),
        "the flatten button is sitting in the path's room: {button:?} against {empty:?}"
    );
    assert!(
        bar.right() - button.right() >= 90.0,
        "nothing but {} points to the right of it, so the filter is not there",
        bar.right() - button.right()
    );

    // ---- Clicking it --------------------------------------------------
    let done = h.click_at(button.center());
    assert!(
        done.contains(&"ToggleFlat"),
        "the button did nothing, got {done:?}"
    );
    h.settle();
    assert!(h.app.panes[0].tab().flat, "the tab is not flattened");

    let flat = shown_names(&h, 0);
    // A row from under a subfolder, found by shape rather than named — the fixture here is the
    // crate's own `src`, and `flattening_as_a_tree_…` below records what naming a file in it
    // cost the first time round.
    let nested = flat
        .iter()
        .find(|name| name.contains('\\') && name.ends_with(".rs"))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the listing did not reach a second level: {} rows, first few {:?}",
                flat.len(),
                &flat[..flat.len().min(5)]
            )
        });
    let leaf = nested
        .rsplit('\\')
        .next()
        .expect("a last component")
        .to_owned();
    assert!(
        flat.len() > shown_names_at_rest(),
        "a flattened tree should have more rows in it than the folder did"
    );

    // A rename edits the *file's* name, not the whole of what the row shows. Otherwise the
    // field would open with `ui\filelist.rs` in it, and accepting that unchanged would ask
    // the shell to rename a file to a path.
    {
        let tab = h.app.panes[0].tab_mut();
        let at = tab
            .order
            .iter()
            .position(|&i| {
                tab.dir
                    .as_ref()
                    .is_some_and(|dir| dir.name(i as usize) == nested)
            })
            .expect("the row is in the listing");
        tab.select_only(at);
        tab.begin_rename();
        assert_eq!(
            tab.renaming.as_ref().map(|(_, text)| text.as_str()),
            Some(leaf.as_str()),
            "the rename field opened on the path rather than on the name"
        );
        tab.renaming = None;
    }

    // ---- And Ctrl+E, which is the same gesture from the keyboard ------
    h.take_journal();
    let held = Modifiers::COMMAND;
    h.modifiers = held;
    h.frame(vec![Event::Key {
        key: egui::Key::E,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: held,
    }]);
    h.modifiers = Modifiers::NONE;
    let done = h.take_journal();
    assert!(
        done.contains(&"ToggleFlat"),
        "Ctrl+E did nothing, got {done:?}"
    );
    h.settle();
    assert!(!h.app.panes[0].tab().flat);
    assert!(
        shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
        "turning it off left the tree on screen"
    );
}

/// **The other flatten mode: the same rows as the tree they came from.**
///
/// Three things this holds, and each of them is a thing only a driven frame can see:
///
/// - the order is **pre-order** and the drawing **indents** by it — a tree whose rows are all
///   at the same x is a list with chevrons on it;
/// - the **twisty is where the pointer can reach it**, which is the class of bug that looks
///   perfectly correct in the source: a rect a few points off, or covered by the row's own
///   hit-test, responds to nothing and reads as a dead control;
/// - and a shut folder **takes its subtree out of the listing**, rather than merely off screen.
///
/// The mode is set through `perform` and the collapse through a real click, which is the split
/// the suite makes everywhere: the menu entry is checked where the menu is, and the gesture
/// nothing else can verify is driven for real.
#[test]
fn flattening_as_a_tree_indents_the_rows_and_a_twisty_shuts_a_branch() {
    use crate::pane::FlatMode;

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    // A tree of its own, in the sandbox, rather than this crate's `src`.
    //
    // It used to read `src`, and that made the test a hostage of the source layout: adding one
    // file at the top of `src` moved every row down by one and pushed `ui\filelist.rs` off the
    // bottom of the window, so a test about *indentation* failed because of a file it never
    // named. Eight rows, shaped like what it asserts, and nothing outside the sandbox.
    let sources = crate::sandbox::fresh("tree");
    for folder in ["fs", "ui"] {
        std::fs::create_dir_all(sources.join(folder)).expect("a fixture folder");
    }
    for file in [
        "fs/dir.rs",
        "fs/sort.rs",
        "ui/filelist.rs",
        "ui/menu.rs",
        "main.rs",
        "theme.rs",
    ] {
        std::fs::write(sources.join(file), b"// a row\n").expect("a fixture file");
    }
    let ctx = h.ctx.clone();
    h.app.perform(
        &ctx,
        Action::Navigate {
            pane,
            path: sources,
        },
    );
    h.settle();
    h.app.perform(&ctx, Action::SetFlatMode(FlatMode::Tree));
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.settle();
    assert!(h.app.panes[0].tab().is_tree(), "the tab is not a tree");

    // ---- Pre-order: a folder, then what is under it --------------------
    let rows = shown_names(&h, 0);
    let depth = |name: &str| name.matches('\\').count();
    let ui_at = rows
        .iter()
        .position(|name| name == "ui")
        .unwrap_or_else(|| panic!("`src\\ui` is not a row: {:?}", &rows[..rows.len().min(8)]));
    assert!(
        rows[ui_at + 1..]
            .iter()
            .take_while(|name| depth(name) > 0)
            .any(|name| name == r"ui\filelist.rs"),
        "the rows under `ui` are not the rows after it: {:?}",
        &rows[ui_at..rows.len().min(ui_at + 8)]
    );
    // And every row of a tree is directly under the folder it is in, which is the assertion
    // that catches an order that is *sorted* by path rather than walked: `a\b\c` would still
    // come after `a\b` there, but a folder's rows would not be one block.
    for (at, name) in rows.iter().enumerate().skip(1) {
        let previous = depth(&rows[at - 1]);
        assert!(
            depth(name) <= previous + 1,
            "row {at} `{name}` is {} levels under `{}`",
            depth(name) - previous,
            rows[at - 1]
        );
    }

    // ---- The indent is on screen, not merely in the order --------------
    let painted = h.texts();
    let x_of = |name: &str| {
        painted
            .iter()
            .find(|(_, text)| text == name)
            .map(|(at, _)| at.x)
            .unwrap_or_else(|| panic!("`{name}` was not drawn"))
    };
    assert!(
        x_of("filelist.rs") > x_of("ui") + 8.0,
        "`filelist.rs` is drawn at {} and its folder at {}: the tree is not indented",
        x_of("filelist.rs"),
        x_of("ui")
    );
    // The row shows its own name and not its path — the indent is what says where it is, so
    // the dimmed `ui` after the name that the *list* mode draws would be saying it twice.
    assert!(
        !painted
            .iter()
            .any(|(_, text)| text.starts_with("filelist.rs") && text.contains("ui")),
        "the tree is still drawing the folder after the name"
    );

    // ---- The twisty, clicked where it is actually drawn ----------------
    //
    // The rows' own hit-test rect is where the geometry comes from — the same rect the row
    // was drawn in — so nothing here restates a number the layout could have moved.
    let block = h
        .ctx
        .read_response(Id::new(("rows-hit", pane)))
        .map(|r| r.rect)
        .expect("the listing was not laid out");
    let row_rect = |at: usize| {
        Rect::from_min_size(
            pos2(block.left(), block.top() + at as f32 * crate::pane::ROW_HEIGHT),
            vec2(block.width(), crate::pane::ROW_HEIGHT),
        )
    };
    let twisty = crate::ui::filelist::twisty_rect(row_rect(ui_at), 0).center();

    let before = shown_names(&h, 0).len();
    let done = h.click_at(twisty);
    assert!(
        done.contains(&"ToggleCollapsed"),
        "the twisty on `ui` did nothing, got {done:?}"
    );
    let after = shown_names(&h, 0);
    assert!(
        after.len() < before,
        "the branch is still open: {before} rows before, {} after",
        after.len()
    );
    assert!(
        after.iter().any(|name| name == "ui"),
        "shutting the folder took the folder itself away"
    );
    assert!(
        !after.iter().any(|name| name == r"ui\filelist.rs"),
        "what was under `ui` is still in the listing"
    );
    // And a shut folder is not a selected one: opening a branch is a way of *looking*, and a
    // click that also moved the selection would make it unusable as one.
    assert_eq!(
        h.app.panes[0].tab().selected_count,
        0,
        "the twisty selected the row as well as shutting it"
    );

    // ---- And a click *beside* it is still an ordinary click ------------
    //
    // The twisty test comes first and returns, so a rect that was too wide — or a `return`
    // that fired on any click at all — would leave a tree in which nothing can be selected.
    // Nothing else in the suite would notice: every other selection test is over a listing
    // that has no twisties in it.
    let shut_rows = shown_names(&h, 0).len();
    let done = h.click_at(pos2(
        block.center().x,
        row_rect(ui_at).center().y,
    ));
    assert!(
        done.contains(&"Focus"),
        "a click on the row itself did nothing, got {done:?}"
    );
    assert_eq!(
        h.app.panes[0].tab().selected_count,
        1,
        "a click on the row did not select it"
    );
    assert_eq!(
        shown_names(&h, 0).len(),
        shut_rows,
        "a click on the row opened the branch as well"
    );

    // ---- Open again, from the keyboard --------------------------------
    //
    // `Right` on the cursor's row, which is what the key means in every tree on the platform
    // — and the only way through a tree for somebody not using the pointer.
    h.app.panes[0].tab_mut().select_only(ui_at);
    h.frame(Vec::new());
    h.frame(vec![Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(Vec::new());
    assert_eq!(
        shown_names(&h, 0).len(),
        before,
        "Right did not open the branch again"
    );
}

/// **The flatten button's menu is reachable, and picking a mode from it takes.**
///
/// A right click on the control that opens a thing is where the settings of that thing belong —
/// and a menu nothing can reach is one of the two failures only a driven frame can see. So the
/// gesture is real all the way through: right-click the button, find the entry on screen by its
/// label, click it.
///
/// Picking a mode deliberately does *not* turn the flatten on, exactly as picking a preview
/// position does not open the panel — the entry above it is what does that. Asserted, because
/// it is a decision rather than an omission.
#[test]
fn the_flatten_buttons_menu_picks_the_mode_without_turning_the_view_on() {
    use crate::pane::FlatMode;

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.settle();
    let button = h
        .ctx
        .read_response(Id::new(("flatten", pane)))
        .map(|r| r.rect)
        .expect("the flatten button was not laid out");

    h.click_with(button.center(), PointerButton::Secondary, Modifiers::NONE);
    // The menu's own entries, found the way a user finds them: by reading the words.
    let entry = |h: &Harness, label: &str| {
        h.texts()
            .into_iter()
            .find(|(_, text)| text == label)
            .map(|(at, _)| at)
    };
    let tree = entry(&h, "Tree").unwrap_or_else(|| {
        panic!(
            "the menu did not open, or has no `Tree` in it: {:?}",
            h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
        )
    });
    assert!(
        entry(&h, "Flatten this folder").is_some(),
        "the menu has no toggle in it, so the mode is all it can do"
    );

    // A couple of points into the label, which is inside the entry whatever its padding is.
    let done = h.click_at(pos2(tree.x + 2.0, tree.y + 6.0));
    assert!(
        done.contains(&"SetFlatMode"),
        "clicking `Tree` did nothing, got {done:?}"
    );
    assert_eq!(h.app.flat_mode, FlatMode::Tree);
    assert!(
        !h.app.panes[0].tab().flat,
        "picking a mode turned the flatten on as well"
    );
    // And it is a setting, so it is part of what the window writes down. Asked of `settings()`
    // rather than of `config_dirty`: the frame after the one that sets the flag is the frame
    // that saves and clears it, and by here several have gone by. What matters is that the mode
    // is in the answer — a value that is chosen and never written is the failure the settings
    // round trip exists for.
    assert_eq!(h.app.settings().flat_mode, FlatMode::Tree);

    // The third setting in the same menu, and the one whose default is *on*: so the click under
    // test turns it off, which is also the only way to tell a tick that means something from a
    // tick that is painted on. Everything else about it is the mode's story — it is the window's,
    // it is written down, and it does not turn the flatten on by itself.
    assert!(h.app.regroup, "regrouping starts on");
    h.click_with(button.center(), PointerButton::Secondary, Modifiers::NONE);
    let regroup = entry(&h, "Regroup single folders").unwrap_or_else(|| {
        panic!(
            "the menu has no regroup entry: {:?}",
            h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
        )
    });
    let done = h.click_at(pos2(regroup.x + 2.0, regroup.y + 6.0));
    assert!(
        done.contains(&"SetRegroup"),
        "clicking `Regroup single folders` did nothing, got {done:?}"
    );
    assert!(!h.app.regroup, "the entry did not turn it off");
    assert!(!h.app.settings().regroup, "and it was not written down");
    assert!(
        !h.app.panes[0].tab().flat,
        "a setting in this menu turned the flatten on"
    );
}

/// **The funnel in the filter box is a button, it opens on a *left* click, and what it offers
/// takes.**
///
/// Three failures only a driven frame can see, and the first one is the reason this test exists.
/// The funnel sits *inside* the field, in the room the design system's `prefix_room` keeps for a
/// leading affix — and the field's own input covers that corner, so a click there reaches whichever
/// widget was registered last. Get that order wrong and the funnel is a picture of a button: the
/// caret lands in the box and the menu never opens, which is exactly what the glyph did before it
/// was a control.
///
/// Then the entries are found the way a reader finds them, by the words on screen, and
/// `Show images only` is asked to do all three of its parts — the lens, the flatten and the tiles.
/// A gallery of one folder's own children, or one drawn as rows, is half the answer.
///
/// Its own sandbox folder rather than a folder of the repository, because the claim is about which
/// rows are left: `src` has no pictures in it, so every arrangement of this would pass.
#[test]
fn the_funnel_in_the_filter_box_opens_on_a_left_click_and_its_listings_take() {
    use crate::pane::{Lens, ViewMode};

    let root = crate::sandbox::fresh("funnel-lenses");
    std::fs::create_dir_all(root.join("shots")).expect("a folder");
    for (name, bytes) in [
        ("a.png", &b"not really a png"[..]),
        ("notes.txt", b"words"),
        (r"shots\b.jpg", b"nor this"),
        (r"shots\build.log", b"lines"),
    ] {
        std::fs::write(root.join(name), bytes).expect("a file");
    }

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    h.app.perform(
        &ctx,
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();

    let funnel = h
        .ctx
        .read_response(Id::new(("filter-lens", pane)))
        .map(|r| r.rect)
        .expect("the funnel was not laid out");

    // ---- It opens on the left button ----------------------------------
    h.click_at(funnel.center());
    let entry = |h: &Harness, label: &str| {
        h.texts()
            .into_iter()
            .find(|(_, text)| text == label)
            .map(|(at, _)| at)
    };
    let images = entry(&h, Lens::Images.label()).unwrap_or_else(|| {
        panic!(
            "a left click on the funnel opened no menu: {:?}",
            h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
        )
    });
    assert!(
        entry(&h, Lens::Git.label()).is_some(),
        "the menu is missing the other listing"
    );
    // And it hangs *below* the button, which is what makes it this control's menu rather than a
    // popup that happens to be on screen. Only the y: a menu near the right-hand edge of the
    // window is shifted along to fit, so where it starts is egui's business and not this test's.
    assert!(
        images.y > funnel.bottom(),
        "the menu is over the bar it was opened from: {images:?} against {funnel:?}"
    );

    // ---- And every part of what it offers happens ----------------------
    //
    // A couple of points into the label, which is inside the entry whatever its padding is.
    let done = h.click_at(pos2(images.x + 2.0, images.y + 6.0));
    assert!(
        done.contains(&"SetLens"),
        "clicking `{}` did nothing, got {done:?}",
        Lens::Images.label()
    );
    assert_eq!(h.tab(0).lens, Some(Lens::Images));
    assert!(h.tab(0).flat, "a gallery of one folder is half the answer");
    assert_eq!(
        h.tab(0).view_mode,
        ViewMode::Icons,
        "pictures were not put in the view that shows pictures"
    );

    h.settle();
    let mut shown = shown_names(&h, 0);
    shown.sort();
    assert_eq!(
        shown,
        ["a.png", r"shots\b.jpg"],
        "the listing is not the pictures under this folder"
    );

    // ---- Ticked, and the tick is how it is turned off ------------------
    h.click_at(funnel.center());
    let images = entry(&h, Lens::Images.label()).expect("the menu did not open again");
    h.click_at(pos2(images.x + 2.0, images.y + 6.0));
    assert_eq!(h.tab(0).lens, None, "the entry on show did not turn off");
    h.settle();
    let mut shown = shown_names(&h, 0);
    shown.sort();
    assert_eq!(
        shown,
        [
            "a.png",
            "notes.txt",
            "shots",
            r"shots\b.jpg",
            r"shots\build.log"
        ],
        "turning the lens off did not put the whole tree back"
    );
}

/// Switching between the two flatten modes **does not read the folder again.**
///
/// Which is the whole reason they are two orders over one listing: a tree big enough to be
/// worth flattening took seconds to walk, and a mode switch that walked it again would make
/// the menu something you avoid using. The listing is an `Arc`, so the assertion is that it is
/// the very same one — not merely one that looks alike.
#[test]
fn switching_flatten_modes_reorders_the_listing_it_already_has() {
    use crate::pane::FlatMode;

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let ctx = h.ctx.clone();
    h.app.perform(
        &ctx,
        Action::Navigate {
            pane,
            path: sources,
        },
    );
    h.settle();
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.settle();
    assert_eq!(
        h.app.panes[0].tab().flat_mode,
        FlatMode::List,
        "the default is the list"
    );

    let listing = h.app.panes[0].tab().dir.clone().expect("the walk's answer");
    let as_list = shown_names(&h, 0);

    h.app.perform(&ctx, Action::SetFlatMode(FlatMode::Tree));
    h.frame(Vec::new());
    let after = h.app.panes[0].tab().dir.clone().expect("still a listing");
    assert!(
        std::sync::Arc::ptr_eq(&listing, &after),
        "the mode switch dropped the listing, so the tree was walked a second time"
    );

    // The same rows, differently arranged — which is the other half of "one listing".
    let as_tree = shown_names(&h, 0);
    assert_ne!(as_list, as_tree, "the order did not change");
    let (mut sorted_list, mut sorted_tree) = (as_list.clone(), as_tree.clone());
    sorted_list.sort();
    sorted_tree.sort();
    assert_eq!(
        sorted_list, sorted_tree,
        "the two modes are not showing the same rows"
    );
}

/// How many rows `src` itself has. Read once, so the comparison above is against the
/// folder rather than against a number written down here.
fn shown_names_at_rest() -> usize {
    let dir = crate::fs::scan::scan(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"));
    dir.len()
}

/// A flatten is a question about *this* folder, so it does not come along to the next one.
///
/// Which is also what makes opening a row the way out of the view: without this, clicking a
/// folder three levels down in a flattened listing would start a second tree walk on
/// arrival, and there would be no gesture that ended one.
#[test]
fn a_flattened_view_does_not_follow_you_into_the_next_folder() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let ctx = h.ctx.clone();
    h.app.perform(
        &ctx,
        Action::Navigate {
            pane,
            path: sources.clone(),
        },
    );
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.settle();
    assert!(h.app.panes[0].tab().flat);

    h.app.perform(
        &ctx,
        Action::Navigate {
            pane,
            path: sources.join("ui"),
        },
    );
    h.settle();
    assert!(
        !h.app.panes[0].tab().flat,
        "the flatten followed the navigation"
    );
    assert!(
        shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
        "and the listing that arrived was still a flattened one"
    );

    // A *refresh* is the same question again, so that one keeps it.
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.settle();
    h.app.perform(&ctx, Action::Refresh(pane));
    h.settle();
    assert!(
        h.app.panes[0].tab().flat,
        "F5 turned the flatten off, which is not what a re-read means"
    );
    // Only the flag, and deliberately: the folder here is `src\ui`, which has no subfolders of
    // its own, so its flattened listing and its shallow one are the same rows. There is
    // nothing about the *listing* this fixture could tell you — which is exactly why the flag
    // is not enough on its own. That half is
    // `deep_reads_survive_a_folder_changing_underneath_them`, over a folder that has a tree.
}

/// **A folder changing on disk re-reads a flattened tab as a flattened tab.**
///
/// This is the flatten "resetting on its own". [`App::folder_changed`] is what
/// [`crate::watch`] calls when a folder changes underneath the window, and it asked for the
/// folder's own children whatever view the tab was in — so saving a file into a flattened
/// folder replaced the tree with one level of it, with [`Tab::flat`] still set and the button
/// still lit. Nothing announced itself: the listing was simply shallow again.
///
/// What made it hard to pin down is that the trigger is invisible and rare. The watch is not
/// recursive, so only a change to the flattened folder's *own* files could do it — a file
/// saved three folders down did nothing at all — and a burst settles for 150 ms before
/// anything is asked. So it fired on some saves and not others, minutes apart, with no gesture
/// of the user's anywhere near it.
///
/// Driven through `folder_changed` rather than by writing a file and waiting on the watcher:
/// the thread is real and so is the settle window, and what is being tested is what the
/// program does with the notification, not that Windows sends one.
#[test]
fn deep_reads_survive_a_folder_changing_underneath_them() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let ctx = h.ctx.clone();
    h.app.perform(
        &ctx,
        Action::Navigate {
            pane,
            path: sources.clone(),
        },
    );
    h.app.perform(&ctx, Action::ToggleFlat(pane));
    h.settle();
    let flat = shown_names(&h, 0);
    assert!(
        flat.iter().any(|name| name.contains('\\')),
        "the fixture is not flattened, so this test proves nothing: {:?}",
        &flat[..flat.len().min(5)]
    );

    // What the watcher does when something writes into `src`.
    h.app.folder_changed(&sources);
    h.settle();

    let tab = h.app.panes[0].tab();
    assert!(tab.flat, "the flag went, which was never the half that went");
    let after = shown_names(&h, 0);
    assert!(
        after.iter().any(|name| name.contains('\\')),
        "the re-read put the folder's own children on screen under a lit button: {:?}",
        &after[..after.len().min(5)]
    );
    assert_eq!(after, flat, "the tree came back different from how it went");

    // And `F5` over the same folder, which is the other way a listing is re-read. It was
    // already right — `Tab::refresh` drops the listing and lets `App::start_scans` re-ask,
    // and that one branches — but the two paths are a pair, and the assertion that can see
    // the difference belongs where both of them can be put through it.
    h.app.perform(&ctx, Action::Refresh(pane));
    h.settle();
    assert_eq!(
        shown_names(&h, 0),
        flat,
        "F5 did not come back with the tree"
    );
}
