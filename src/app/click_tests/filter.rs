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
    // And searches the tree: the filter that applied is what flattens the folder, once.
    h.settle();
    assert!(h.tab(0).flat, "a filter that applied did not flatten the folder");
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

    // Against the tree it walked, not the folder's own children it started from.
    let rows = h.tab(0).order.len();
    let all = dir.len();
    assert!(
        rows < all,
        "the filter never applied at all: still {rows} of {all} rows"
    );
    // Files only: a flattened tree keeps the folders that lead to a match, so that the match can be
    // reached, and `app` is kept for `app\click_tests\…co…` without matching itself.
    for &i in h.tab(0).order.iter().filter(|&&i| !dir.entries[i as usize].is_dir()) {
        let path = dir.target(i as usize).to_string_lossy().to_lowercase();
        assert!(
            path.contains("co"),
            "`{path}` does not match the filter that was typed"
        );
    }
}

/// **A filter searches the whole tree**: once the typing stops, the folder is flattened — one walk
/// for the word, not one per letter — and the filter narrows what the walk found.
///
/// And it is the filter's own question only where the button would be: on This PC there is no tree
/// to walk, and typing there flattens nothing.
#[test]
fn typing_a_filter_flattens_the_folder_once_the_typing_stops() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: src.clone(),
        },
    );
    h.settle();
    assert!(!h.tab(0).flat, "the folder opened flattened, so this proves nothing");

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

    // Typed but not settled: nothing walked yet.
    h.frame(vec![Event::Text("tests".to_owned())]);
    assert!(!h.tab(0).flat, "flattened on the keystroke rather than when the typing stopped");

    h.take_journal();
    h.time += crate::pane::FILTER_DELAY * 1.5;
    // The frame that settles the filter, and the one after, which is where the app acts on it —
    // never during drawing. See `App::follow_filters`.
    h.frame(Vec::new());
    h.frame(Vec::new());
    let done = h.take_journal();
    assert_eq!(
        done.iter().filter(|name| **name == "FilterFlat").count(),
        1,
        "the settled filter should flatten exactly once, got {done:?}"
    );
    assert!(h.tab(0).flat);
    h.settle();

    // Files from below the folder, which only the tree has — `tests.rs` lives in subfolders of `src`.
    let tab = h.tab(0);
    let dir = tab.dir.as_ref().expect("the walk landed");
    assert!(
        tab.order
            .iter()
            .any(|&i| dir.name(i as usize).contains(std::path::MAIN_SEPARATOR)),
        "nothing from a subfolder survived the filter, so the tree was not searched"
    );
    // Still flat after another frame: one walk, not a toggle back and forth.
    h.frame(Vec::new());
    assert!(h.tab(0).flat);

    // ---- Cleared, through the real field: the folder as it was -----------
    let erase: Vec<Event> = (0.."tests".len())
        .map(|_| Event::Key {
            key: egui::Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        })
        .collect();
    h.frame(erase);
    assert_eq!(h.tab(0).filter, "", "the field was not emptied");
    h.take_journal();
    h.time += crate::pane::FILTER_DELAY * 1.5;
    h.frame(Vec::new());
    h.frame(Vec::new());
    let done = h.take_journal();
    assert_eq!(
        done.iter().filter(|name| **name == "FilterFlat").count(),
        1,
        "clearing the filter should unflatten exactly once, got {done:?}"
    );
    assert!(!h.tab(0).flat, "the folder is still flattened after its filter was cleared");
    h.settle();
    let tab = h.tab(0);
    let dir = tab.dir.as_ref().expect("the folder's own listing");
    assert!(
        (0..dir.len()).all(|i| !dir.name(i).contains(std::path::MAIN_SEPARATOR)),
        "the listing is still the tree, not the folder's own children"
    );

    // ---- This PC: no tree, so nothing to flatten -------------------------
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: PathBuf::new(),
        },
    );
    h.settle();
    h.app.panes[0].tab_mut().filter = "c".to_owned();
    h.app.panes[0].tab_mut().filter_changed(h.time);
    h.take_journal();
    h.time += crate::pane::FILTER_DELAY * 1.5;
    h.frame(Vec::new());
    h.frame(Vec::new());
    let done = h.take_journal();
    assert!(!done.contains(&"FilterFlat"), "This PC was flattened: {done:?}");
    assert!(!h.tab(0).flat);
}

/// **`Escape` in the filter box empties it and keeps the caret there**, ready for the next query; a
/// second `Escape`, on the empty box, leaves it as it always did — so the key is never a trap.
#[test]
fn escape_clears_the_filter_and_keeps_the_caret() {
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
    let field = h.ctx.memory(|m| m.focused());
    assert!(field.is_some(), "Ctrl+F did not put the caret in the filter box");
    h.frame(vec![Event::Text("abc".to_owned())]);
    assert_eq!(h.tab(0).filter, "abc");

    let escape = || Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    h.frame(vec![escape()]);
    h.frame(Vec::new());
    assert_eq!(h.tab(0).filter, "", "Escape did not empty the box");
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        field,
        "Escape took the caret out of the box along with the text"
    );
    // The clear is a real one: once the wait is up the listing is the whole folder again.
    h.time += crate::pane::FILTER_DELAY * 1.5;
    h.frame(Vec::new());
    assert!(h.tab(0).filter_at.is_none(), "the cleared filter was never applied");

    // And on an empty box `Escape` is the way out, as before.
    h.frame(vec![escape()]);
    h.frame(Vec::new());
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        None,
        "Escape on an empty box kept the keyboard in it"
    );
}

/// The filter box with the caret in it, over this crate's own `src` — the fixture every test below
/// shares. Returns the pane.
fn filter_in_src(h: &mut Harness) -> crate::pane::PaneId {
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        },
    );
    h.settle();
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
        "Ctrl+F did not put the caret in the filter box"
    );
    pane
}

/// Past the typing's wait, and the frame after it, which is where the app acts on a settled filter.
fn past_the_wait(h: &mut Harness) {
    h.time += crate::pane::FILTER_DELAY * 1.5;
    h.frame(Vec::new());
    h.frame(Vec::new());
}

/// **While the tree is walked, the folder's own matches stay on screen** — blanking them for the
/// second the walk takes reads as "nothing matched".
#[test]
fn the_folder_stays_on_screen_while_the_tree_is_walked() {
    let mut h = Harness::new();
    filter_in_src(&mut h);
    h.frame(vec![Event::Text("mod".to_owned())]);
    past_the_wait(&mut h);

    // The frame the walk was asked for in: flattened, waiting, and a listing on show regardless.
    let tab = h.tab(0);
    assert!(tab.flat, "the filter did not flatten the folder");
    if tab.awaiting.is_some() {
        assert!(
            tab.dir.is_some(),
            "the listing was dropped while the walk was out, so the pane went blank"
        );
    }
    h.settle();
    let tab = h.tab(0);
    assert!(tab.awaiting.is_none(), "the walk never landed");
    let dir = tab.dir.as_ref().expect("the tree");
    assert!(
        (0..dir.len()).any(|i| dir.name(i).contains('\\')),
        "what landed is not the tree"
    );
}

/// **Clearing the filter puts the folder back where it was**: the same row selected as before the
/// typing started, in the folder's own listing.
#[test]
fn clearing_the_filter_puts_the_selection_back() {
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
    let chosen = {
        let tab = h.app.panes[0].tab_mut();
        tab.select_only(3);
        let entry = tab.order[3] as usize;
        tab.dir.as_ref().expect("the listing").name(entry).to_owned()
    };

    filter_in_src(&mut h);
    // `filter_in_src` navigated again, which is the same folder and keeps nothing; select again.
    {
        let tab = h.app.panes[0].tab_mut();
        let at = tab
            .order
            .iter()
            .position(|&i| tab.dir.as_ref().unwrap().name(i as usize) == chosen)
            .expect("the row is still there");
        tab.select_only(at);
    }
    h.frame(vec![Event::Text("mod".to_owned())]);
    past_the_wait(&mut h);
    h.settle();
    assert!(h.tab(0).flat);

    h.frame(vec![key(egui::Key::Escape)]);
    past_the_wait(&mut h);
    h.settle();
    let tab = h.tab(0);
    assert!(!tab.flat, "clearing the filter left the folder flattened");
    let dir = tab.dir.as_ref().expect("the folder's own listing");
    let selected: Vec<&str> = (0..dir.len())
        .filter(|&i| tab.selected[i])
        .map(|i| dir.name(i))
        .collect();
    assert_eq!(selected, [chosen.as_str()], "the selection did not come back");
    assert!(tab.cursor.is_some(), "and the cursor with it");
}

/// **`↓` goes from the box to the results, and `Enter` opens one.**
///
/// `Enter` is only pressed over a folder: a file would be handed to the shell and opened for real.
#[test]
fn down_and_enter_take_the_filter_to_its_results() {
    let mut h = Harness::new();
    let pane = filter_in_src(&mut h);
    // `ui$`: the `ui` folder itself, which is a folder — whatever else ends in `ui`.
    h.frame(vec![Event::Text("ui$".to_owned())]);
    past_the_wait(&mut h);
    h.settle();

    h.take_journal();
    h.frame(vec![key(egui::Key::ArrowDown)]);
    h.frame(Vec::new());
    assert!(h.take_journal().contains(&"FilterDown"));
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        None,
        "↓ left the keyboard in the box"
    );
    assert_eq!(h.tab(0).cursor, Some(0), "↓ did not put the cursor on the first match");

    // Back into the box, and Enter.
    filter_in_src_again(&mut h);
    if !h.tab(0).is_dir_at(0) {
        println!("the first match is a file; not pressing Enter on it");
        return;
    }
    let want = h.tab(0).target_at(0).expect("a target");
    h.take_journal();
    h.frame(vec![key(egui::Key::Enter)]);
    h.frame(Vec::new());
    let done = h.take_journal();
    assert!(done.contains(&"FilterEnter"), "{done:?}");
    assert!(done.contains(&"Navigate"), "Enter did not open the match: {done:?}");
    h.settle();
    assert_eq!(h.app.pane_mut(pane).unwrap().tab().path, want);
}

/// **`Ctrl+Enter` in the box selects the matches.** Over this folder the filter then goes and the
/// whole folder is back with them selected; over a search of the subfolders it stays, because
/// clearing it would fold the selected files out of sight.
#[test]
fn ctrl_enter_turns_the_matches_into_a_selection() {
    let ctrl_enter = || Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    };
    let selected = |h: &Harness| -> Vec<String> {
        let tab = h.tab(0);
        let dir = tab.dir.as_ref().expect("a listing");
        let mut names: Vec<String> = (0..dir.len())
            .filter(|&i| tab.selected[i])
            .map(|i| dir.name(i).to_owned())
            .collect();
        names.sort();
        names
    };

    // ---- This folder only: the filter goes ------------------------------
    let mut h = Harness::new();
    h.app.perform(&h.ctx.clone(), Action::SetSearchSubfolders(false));
    filter_in_src(&mut h);
    let all = h.tab(0).order.len();
    // The filter is asked of the whole path, so a leaf is anchored by the separator before it.
    h.frame(vec![Event::Text("\\main.rs$".to_owned())]);
    past_the_wait(&mut h);
    h.settle();
    assert_eq!(h.tab(0).match_count(), 1);

    h.take_journal();
    h.modifiers = Modifiers::COMMAND;
    h.frame(vec![ctrl_enter()]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    let done = h.take_journal();
    assert!(done.contains(&"FilterSelect"), "{done:?}");
    assert_eq!(selected(&h), ["main.rs"], "{done:?}");
    // Still so once the cleared filter has settled — which is when the folder as it was before the
    // typing would be put back.
    past_the_wait(&mut h);
    h.settle();
    assert_eq!(selected(&h), ["main.rs"], "the selection from before the filter came back");
    let tab = h.tab(0);
    assert!(tab.filter.is_empty(), "the filter is still up: {:?}", tab.filter);
    assert_eq!(tab.order.len(), all, "the whole folder is not back");
    let on = tab.cursor.and_then(|at| tab.entry_at(at)).expect("a cursor");
    assert_eq!(tab.dir.as_ref().unwrap().name(on), "main.rs", "the cursor is not on the match");
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        None,
        "Ctrl+Enter left the keyboard in the box"
    );

    // Nothing matched: nothing changes, and the caret stays to fix the pattern.
    filter_in_src_again(&mut h);
    h.frame(vec![Event::Text("no such file anywhere".to_owned())]);
    past_the_wait(&mut h);
    h.settle();
    h.modifiers = Modifiers::COMMAND;
    h.frame(vec![ctrl_enter()]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    assert_eq!(selected(&h), ["main.rs"], "an empty match changed the selection");
    assert_eq!(h.tab(0).filter, "no such file anywhere");
    assert!(h.ctx.memory(|m| m.focused()).is_some(), "the caret left the box");

    // ---- A search of the subfolders: the filter stays -------------------
    let mut h = Harness::new();
    h.app.perform(&h.ctx.clone(), Action::SetSearchSubfolders(true));
    filter_in_src(&mut h);
    h.frame(vec![Event::Text("\\mod.rs$".to_owned())]);
    past_the_wait(&mut h);
    h.settle();
    assert!(h.tab(0).flat, "the filter did not search the subfolders");
    let found = h.tab(0).match_count();
    assert!(found > 3, "too few mod.rs under src to mean anything: {found}");

    h.modifiers = Modifiers::COMMAND;
    h.frame(vec![ctrl_enter()]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    let tab = h.tab(0);
    assert_eq!(tab.filter, "\\mod.rs$", "the search was cleared");
    assert!(tab.flat, "and folded away");
    let picked = selected(&h);
    assert_eq!(picked.len(), found, "matches, and only matches: {picked:?}");
    assert!(
        picked.iter().all(|name| name.ends_with("mod.rs")),
        "a folder on the way to a match was selected: {picked:?}"
    );
}

/// Ctrl+F again, without navigating — for a test that has already filtered.
fn filter_in_src_again(h: &mut Harness) {
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
}

/// **With *Search subfolders too* unticked, a filter narrows this folder and walks nothing** — and
/// the box says which it will do before anything is typed.
#[test]
fn with_subfolders_off_a_filter_stays_in_the_folder() {
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
    // The placeholder, read before the caret goes in: a focused empty box draws none.
    let says = |h: &Harness, words: &str| h.texts().iter().any(|(_, text)| text == words);
    assert!(
        says(&h, "Search"),
        "the box does not say it searches subfolders"
    );
    h.app.perform(&h.ctx.clone(), Action::SetSearchSubfolders(false));
    h.frame(Vec::new());
    assert!(
        says(&h, "Filter this folder"),
        "the box still promises a search of the subfolders"
    );

    filter_in_src_again(&mut h);
    h.frame(vec![Event::Text("mod".to_owned())]);
    past_the_wait(&mut h);
    h.settle();
    assert!(!h.tab(0).flat, "a filter walked the tree with the setting off");
    // Ticked again, under the filter that is up: the walk starts without a keystroke.
    h.take_journal();
    h.app.perform(&h.ctx.clone(), Action::SetSearchSubfolders(true));
    h.frame(Vec::new());
    h.frame(Vec::new());
    assert!(h.take_journal().contains(&"FilterFlat"));
    assert!(h.app.pane_mut(pane).unwrap().tab().flat);
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
    // The filter flattens the folder, so what it narrows is the tree once the walk has landed.
    h.settle();

    let dir = h.tab(0).dir.clone().expect("the listing");
    let order = &h.tab(0).order;
    // Files, as a set. The flattened tree a filter opens also keeps the folders on the way to each
    // match — so they can be opened — and lists rows in tree order; neither is this test's subject.
    let file = |i: usize| !dir.entries[i].is_dir();
    let mut kept: Vec<&str> = order
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| file(i))
        .map(|i| dir.name(i))
        .collect();
    kept.sort_unstable();

    // What "both words, in any order, over the whole path" comes to, worked out here rather
    // than written down as a list of names: whether this repository happens to live somewhere
    // with a `pre` in it then changes both sides together instead of only one, and the test
    // still says what it means to say.
    let mut want: Vec<&str> = (0..dir.len())
        .filter(|&i| file(i))
        .filter(|&i| {
            let path = dir.target(i).to_string_lossy().to_lowercase();
            path.contains("view") && path.contains("pre")
        })
        .map(|i| dir.name(i))
        .collect();
    want.sort_unstable();
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

    // A filter typed in, so the other half of the claim can be checked: it survives. Which flattens
    // the folder on its own — a filter searches the tree — so Ctrl+E below is the way back *out*.
    h.app.panes[0].tab_mut().filter = "sort".to_owned();
    h.app.panes[0].tab_mut().rebuild_order();
    h.frame(Vec::new());
    h.settle();
    assert!(h.app.panes[0].tab().flat, "the filter did not flatten the folder");
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

    // Off by hand, and it stays off: the filter that is still up does not press the button back.
    let tab = h.app.panes[0].tab();
    assert!(!tab.flat, "the filter flattened the folder again over Ctrl+E");
    assert_eq!(tab.filter, "sort", "the filter went with the toggle");
    // Possibly nothing at all: `sort` lives below `src`, not in it, and an empty listing is the
    // right answer for the folder's own children. That the filter survived is the assertion above.
    let names = shown_names(&h, 0);
    assert!(
        names.iter().all(|name| name.contains("sort")),
        "the filter is not being applied to the folder's own listing: {names:?}"
    );
    assert!(
        names.iter().all(|name| !name.contains('\\')),
        "a subfolder's file is still showing, so the folder is still the tree: {names:?}"
    );
}
