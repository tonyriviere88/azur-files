//! The filter box: what it narrows to, when it acts, and the shortcuts that reach past it.

use super::*;

/// **The filter waits for the typing to stop, and each keystroke restarts the wait.**
///
/// One pass costs up to 240 ms on a large listing — see [`crate::pane::FILTER_DELAY`] — so
/// what this is really about is that typing a word should cost one pass and not one per
/// letter. Driven through the real field with the real clock, because the whole behaviour is
/// a relationship between keystrokes and time.
#[test]
fn the_filter_waits_for_the_typing_to_stop() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        },
    );
    h.settle();
    let all = h.tab(0).order.len();
    assert!(all > 8, "the fixture folder is too small to filter");

    // The caret into the box, which is where the keystrokes have to land.
    let held = Modifiers::COMMAND;
    h.modifiers = held;
    h.frame(vec![Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: held,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "the caret is not in the filter box, so nothing below is being tested"
    );

    // Each step is most of the wait but not all of it, so two of them cross the deadline and
    // one does not. In terms of the constant rather than in milliseconds: what is being
    // tested is the relationship, and it should still hold when the wait is retuned.
    let step = crate::pane::FILTER_DELAY * 0.6;

    // ---- One letter: noted, not applied ---------------------------------
    h.frame(vec![Event::Text("c".to_owned())]);
    assert_eq!(h.tab(0).filter, "c", "the keystroke never reached the field");
    assert!(h.tab(0).filter_at.is_some(), "no deadline was set");
    assert_eq!(
        h.tab(0).order.len(),
        all,
        "the filter was applied on the keystroke, which is the thing this prevents"
    );

    // A frame most of the way through the wait changes nothing.
    h.time += step;
    h.frame(Vec::new());
    assert_eq!(h.tab(0).order.len(), all, "applied before the wait was up");

    // ---- A second letter restarts it ------------------------------------
    h.frame(vec![Event::Text("o".to_owned())]);
    assert_eq!(h.tab(0).filter, "co");
    // The same step again, which is past the deadline the *first* letter set and short of the
    // one the second set. This is where it would have fired without the restart.
    h.time += step;
    h.frame(Vec::new());
    assert_eq!(
        h.tab(0).order.len(),
        all,
        "the second keystroke did not restart the wait"
    );

    // ---- And then it fires, once, on the whole word ---------------------
    h.time += step;
    h.frame(Vec::new());
    assert!(h.tab(0).filter_at.is_none(), "the deadline is still standing");
    let dir = h.tab(0).dir.clone().expect("the listing");

    // Everything above is about the clock and holds wherever this repository lives. What is
    // left is about the *answer*, and that does depend on where it lives: the filter is asked
    // about the whole path (`fs::sort::prefix`), so a checkout under a folder whose own name
    // holds a `co` keeps every row, and the two assertions below would be reading a listing
    // that was never narrowed. Which is not a failure — it is a fixture that cannot show the
    // difference, so say so instead of asserting nothing.
    let base = dir.path.to_string_lossy().to_lowercase();
    if base.contains("co") {
        println!("`{base}` matches the filter this test types; skipping what it keeps");
        return;
    }

    let rows = h.tab(0).order.len();
    assert!(
        rows < all,
        "the filter never applied at all: still {rows} of {all} rows"
    );
    for &i in &h.tab(0).order {
        let path = dir.target(i as usize).to_string_lossy().to_lowercase();
        assert!(
            path.contains("co"),
            "`{path}` does not match the filter that was typed"
        );
    }
}

/// **Two words typed in the box narrow by both.**
///
/// What the words *mean* belongs to `azur_egui_theme::filter` and is tested there. What is
/// tested here is the one character that has to survive the journey from the keyboard to
/// [`crate::fs::sort::build_order`] for any of it to work: the **space**. It is the only
/// punctuation in the box that now carries meaning, and it is exactly the kind of key
/// something else takes — a shortcut on `Space`, a guard that trims, an event a focused field
/// never sees. Any of those leaves a filter that quietly keeps nothing.
#[test]
fn two_words_in_the_filter_box_narrow_by_both() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        },
    );
    h.settle();

    // `Ctrl+F` for the caret, then the whole query — the keystroke-by-keystroke behaviour is
    // `the_filter_waits_for_the_typing_to_stop`'s subject, not this one's.
    let held = Modifiers::COMMAND;
    h.modifiers = held;
    h.frame(vec![Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: held,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(vec![Event::Text("view pre".to_owned())]);
    assert_eq!(
        h.tab(0).filter,
        "view pre",
        "the space did not reach the field"
    );

    h.time += crate::pane::FILTER_DELAY * 1.5;
    h.frame(Vec::new());
    assert!(h.tab(0).filter_at.is_none(), "the filter never applied");

    let dir = h.tab(0).dir.clone().expect("the listing");
    let order = &h.tab(0).order;
    let kept: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();

    // What "both words, in any order, over the whole path" comes to, worked out here rather
    // than written down as a list of names: whether this repository happens to live somewhere
    // with a `pre` in it then changes both sides together instead of only one, and the test
    // still says what it means to say.
    let want: Vec<&str> = (0..dir.len())
        .filter(|&i| {
            let path = dir.target(i).to_string_lossy().to_lowercase();
            path.contains("view") && path.contains("pre")
        })
        .map(|i| dir.name(i))
        .collect();
    // That the two words *narrow* — which is what makes the comparison below worth making.
    // Not a named file: the fixture is this crate's own `src`, and naming one in it makes the
    // test a hostage of the source layout.
    assert!(
        !want.is_empty() && want.len() < dir.len(),
        "the two words match {} of {} rows, so this asserts nothing",
        want.len(),
        dir.len()
    );
    assert_eq!(kept, want, "the two words did not both narrow the listing");
}

/// **A changed filter opens the listing at the top.**
///
/// Driven through the real `ScrollArea`, because the thing being claimed is about an offset
/// egui owns: setting [`Tab::scroll_y`] alone records where the listing *is*, and only
/// [`Tab::scroll_to`] moves it. A test that set the field and read it back would pass whatever
/// the program did.
///
/// The fixture is `src` **flattened**, and it has to be: the point only exists where there is
/// somewhere to be scrolled *to*, both before the filter and after it. A listing the filter
/// cuts down to less than a viewport-full comes back to the top on its own — `ScrollArea`
/// clamps to the content it has — and a test built on that one would hold with none of this
/// here.
#[test]
fn a_changed_filter_opens_the_listing_at_the_top() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        },
    );
    h.settle();
    h.app.perform(&h.ctx.clone(), Action::ToggleFlat(pane));
    h.settle();

    // Down the listing, for real. `scroll_to` is what the rubber-band's auto-scroll uses, so
    // this is a gesture the program already makes rather than a back door into egui.
    let rows = h.tab(0).order.len();
    h.app.panes[0].tab_mut().scroll_to = Some(300.0);
    h.frame(Vec::new());
    let was = h.tab(0).scroll_y;
    assert!(
        was > 100.0,
        "the flattened fixture ({rows} rows) does not scroll far enough to show anything: \
         stopped at {was}"
    );

    // A filter typed into the real field, and then the wait it takes to be applied.
    let held = Modifiers::COMMAND;
    h.modifiers = held;
    h.frame(vec![Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: held,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(vec![Event::Text(".rs$".to_owned())]);
    h.time += crate::pane::FILTER_DELAY * 1.5;
    h.frame(Vec::new());

    let tab = h.tab(0);
    assert!(tab.filter_at.is_none(), "the filter never applied");
    assert_eq!(
        tab.scroll_y, 0.0,
        "the listing kept its old offset through a change of filter"
    );
    // And the filtered listing is still long enough that the offset above was thrown away
    // rather than clamped away, which is what makes the assertion mean anything.
    let left = tab.order.len();
    assert!(
        left as f32 * crate::pane::ROW_HEIGHT > was + 300.0,
        "only {left} rows survived `.rs$`, which cannot hold an offset of {was}"
    );
}

/// **`F3` puts the caret in the filter box, exactly as `Ctrl+F` does.**
///
/// One key rather than a chord, and the key most Windows programs have meant "find" with for
/// longer than `Ctrl+F` has been the convention. Both are driven here rather than one: what
/// would break them is the same line, and a shortcut that quietly stopped working is invisible
/// until somebody presses it.
#[test]
fn ctrl_f_and_f3_both_reach_the_filter_box() {
    for key in [egui::Key::F, egui::Key::F3] {
        let mut h = Harness::new();
        // The chord for one and nothing at all for the other — `consume_shortcut` matches the
        // modifiers exactly, so `F3` would not fire if it were asked for with `COMMAND` held,
        // and `Ctrl+F` would not fire without it.
        let held = if key == egui::Key::F {
            Modifiers::COMMAND
        } else {
            Modifiers::NONE
        };
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        // The bar asks for focus on the frame it sees the shortcut; egui grants it at the end.
        h.frame(Vec::new());
        assert!(
            h.ctx.memory(|m| m.focused()).is_some(),
            "{key:?} did not put the caret anywhere"
        );

        // And it is the filter box that has it, not merely something: the proof is that
        // typing lands in `Tab::filter`.
        h.frame(vec![Event::Text("z".to_owned())]);
        assert_eq!(
            h.tab(0).filter,
            "z",
            "{key:?} focused something that is not the filter box"
        );
    }
}

/// `Ctrl+P` works with the caret in the filter box too, and the filter survives it.
///
/// The same argument as `Ctrl+E` below, and the same gesture in a different order: you type two
/// letters to find a file, then want to see what is inside it. A focused text field otherwise
/// owns the keyboard outright — which is right for every other shortcut and wrong for these two.
#[test]
fn ctrl_p_reaches_through_the_filter_box() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: sources,
        },
    );
    h.settle();

    // Ctrl+F, which is how the caret gets there without a click. Then a second frame: the bar
    // asks for focus on the frame it sees the shortcut, and egui grants it at the end.
    let held = Modifiers::COMMAND;
    let press = |h: &mut Harness, key: egui::Key| {
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
    };
    press(&mut h, egui::Key::F);
    h.frame(Vec::new());
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "Ctrl+F did not put the caret in the filter box, so this test proves nothing"
    );

    h.app.panes[0].tab_mut().filter = "sort".to_owned();
    h.app.panes[0].tab_mut().rebuild_order();
    h.frame(Vec::new());
    h.take_journal();

    press(&mut h, egui::Key::P);
    let done = h.take_journal();
    assert!(
        done.contains(&"TogglePreview"),
        "Ctrl+P did not get through the filter box, got {done:?}"
    );
    assert!(h.app.panes[0].tab().preview.open, "the panel did not open");
    assert_eq!(
        h.app.panes[0].tab().filter,
        "sort",
        "the filter went with the toggle"
    );
    // And the caret is still in the box, so the two really do compose rather than one
    // interrupting the other.
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "the shortcut took the caret out of the filter box"
    );

    // And again, to shut it: a shortcut you can only use one way round is a trap.
    press(&mut h, egui::Key::P);
    let done = h.take_journal();
    assert!(done.contains(&"TogglePreview"), "got {done:?}");
    assert!(!h.app.panes[0].tab().preview.open, "it did not shut again");
}

/// `Ctrl+E` works with the caret in the filter box, and the filter survives the toggle.
///
/// The two compose, and that is the gesture: type two letters, look at what came up, and
/// want the rest of the tree. A focused text field otherwise owns the keyboard outright —
/// which is right for every other shortcut and wrong for this one.
#[test]
fn ctrl_e_reaches_through_the_filter_box() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: sources,
        },
    );
    h.settle();

    // Ctrl+F, which is how the caret gets there without a click. Then a second frame: the
    // bar asks for focus on the frame it sees the shortcut, and egui grants it at the end.
    let held = Modifiers::COMMAND;
    h.modifiers = held;
    h.frame(vec![Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: held,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "Ctrl+F did not put the caret in the filter box, so this test proves nothing"
    );

    // A filter typed in, so the other half of the claim can be checked: it survives.
    h.app.panes[0].tab_mut().filter = "sort".to_owned();
    h.app.panes[0].tab_mut().rebuild_order();
    h.frame(Vec::new());
    h.take_journal();

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
        "Ctrl+E did not get through the filter box, got {done:?}"
    );
    h.settle();

    let tab = h.app.panes[0].tab();
    assert!(tab.flat);
    assert_eq!(tab.filter, "sort", "the filter went with the toggle");
    let names = shown_names(&h, 0);
    assert!(
        !names.is_empty() && names.iter().all(|name| name.contains("sort")),
        "the filter is not being applied to the flattened listing: {names:?}"
    );
    assert!(
        names.iter().any(|name| name.contains('\\')),
        "nothing from a subfolder came up, so the tree was not walked: {names:?}"
    );
}
