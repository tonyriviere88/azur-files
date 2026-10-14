//! The command palette: the key that opens it, what narrows it, and what a pick does.
//!
//! The fixture is a folder in the sandbox. Nothing picked here touches a file: the commands driven
//! are switches and a focus, and the tests that walk the whole catalogue only ask what each entry
//! *would* do — [`App::palette_action`] resolves, it does not perform.

use super::*;
use crate::app::palette::{listed, Command};

/// A window on a sandbox folder of three files, all selected, so the selection's commands are listed.
fn on_three_files(name: &str) -> Harness {
    let root = crate::sandbox::fresh(name);
    for file in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(file), file.as_bytes()).unwrap();
    }
    let mut h = Harness::opening(vec![root]);
    let pane = h.app.panes[0].id;
    h.app.perform(&h.ctx.clone(), Action::SelectAll(pane));
    h.frame(Vec::new());
    assert_eq!(h.tab(0).selected_count, 3);
    h
}

fn open_palette(h: &mut Harness) {
    h.chord(egui::Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(h.app.palette.is_some(), "Ctrl+Shift+P did not put the palette up");
    h.frame(Vec::new());
}

fn labels(h: &Harness) -> Vec<String> {
    h.app
        .palette
        .as_ref()
        .map(|p| p.rows.iter().map(|row| row.label.clone()).collect())
        .unwrap_or_default()
}

/// **`Ctrl+Shift+P` is the palette and only the palette.** `Ctrl+P` is the preview panel, and both
/// of egui's ways of reading a key match it with Shift held too — so a palette read after it would
/// open the panel as well.
#[test]
fn ctrl_shift_p_opens_the_palette_and_not_the_preview() {
    let mut h = on_three_files("palette-key");
    h.chord(egui::Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    let journal = h.take_journal();
    assert!(journal.contains(&"TogglePalette"), "{journal:?}");
    assert!(!journal.contains(&"TogglePreview"), "the preview opened too: {journal:?}");
    assert!(h.app.palette.is_some());
    assert!(!h.tab(0).preview.open);

    // The same key from inside the palette's own field takes it down.
    h.frame(Vec::new());
    h.chord(egui::Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(h.app.palette.is_none(), "the second Ctrl+Shift+P left it up");
    assert!(!h.tab(0).preview.open);
}

/// **Words in any order, as the filter box reads them** — not a fuzzy match, and not a prefix.
#[test]
fn words_in_any_order_narrow_to_the_command() {
    let mut h = on_three_files("palette-words");
    open_palette(&mut h);
    let all = labels(&h).len();

    h.frame(vec![Event::Text("hidden show".to_owned())]);
    h.frame(Vec::new());
    assert_eq!(labels(&h), ["Show hidden files"], "out of {all} entries");

    // Letters out of order are not a match: this is not a fuzzy finder.
    h.frame(vec![Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]);
    h.frame(vec![Event::Text("shwhdn".to_owned())]);
    h.frame(Vec::new());
    let found = labels(&h);
    assert!(
        !found.iter().any(|label| label == "Show hidden files"),
        "a subsequence matched: {found:?}"
    );
}

/// **`Enter` runs the highlighted row, and the palette goes.** The first row is highlighted until
/// something moves it, so two words and `Enter` is the whole gesture.
#[test]
fn enter_runs_the_first_match() {
    let mut h = on_three_files("palette-enter");
    let before = h.app.show_hidden;
    open_palette(&mut h);
    h.frame(vec![Event::Text("hidden show".to_owned())]);
    h.frame(Vec::new());
    h.take_journal();

    h.frame(tap(egui::Key::Enter));
    let journal = h.take_journal();
    assert!(journal.contains(&"ToggleHidden"), "{journal:?}");
    assert!(h.app.palette.is_none(), "the palette stayed up after running a command");
    assert_ne!(h.app.show_hidden, before);

    // And it is remembered, first, under a heading of its own, for the next time nothing is typed.
    assert_eq!(h.app.palette_recent.first().map(String::as_str), Some("Hidden"));
    open_palette(&mut h);
    let palette = h.app.palette.as_ref().unwrap();
    assert_eq!(palette.rows[0].label, "Show hidden files");
    assert_eq!(palette.rows[0].heading, "Recently used");
}

/// **A click on a row runs it** — the row has to be on top of whatever is under the palette.
#[test]
fn a_click_on_a_row_runs_it() {
    let mut h = on_three_files("palette-click");
    open_palette(&mut h);
    h.frame(vec![Event::Text("left panel".to_owned())]);
    h.frame(Vec::new());
    let at = h
        .texts()
        .into_iter()
        .find(|(_, text)| text == "Left panel")
        .map(|(at, _)| at)
        .expect("the row is not drawn");
    let before = h.app.sidebar_shown;
    // A real move first — from somewhere, to the row — so the rows answer to the pointer rather
    // than to the keyboard. The first position a harness reports is no movement at all.
    h.frame(vec![Event::PointerMoved(pos2(1.0, 1.0))]);
    h.frame(vec![Event::PointerMoved(at + vec2(4.0, 4.0))]);
    let journal = h.click_at(at + vec2(4.0, 4.0));
    assert!(journal.contains(&"ToggleSidebar"), "{journal:?}");
    assert_ne!(h.app.sidebar_shown, before);
    assert!(h.app.palette.is_none());
}

/// **`Escape` closes it and does nothing else** — in particular not the listing's `Escape`, which
/// would clear the selection behind it.
#[test]
fn escape_closes_it_and_leaves_the_selection() {
    let mut h = on_three_files("palette-escape");
    open_palette(&mut h);
    h.frame(tap(egui::Key::Escape));
    h.frame(Vec::new());
    assert!(h.app.palette.is_none());
    assert_eq!(h.tab(0).selected_count, 3, "the Escape reached the listing");
}

/// **`Filter this folder` puts the caret in the filter box**, as `Ctrl+F` does.
#[test]
fn filter_this_folder_puts_the_caret_in_the_box() {
    let mut h = on_three_files("palette-filter");
    open_palette(&mut h);
    h.frame(vec![Event::Text("filter folder".to_owned())]);
    h.frame(Vec::new());
    h.frame(tap(egui::Key::Enter));
    h.frame(Vec::new());
    h.frame(Vec::new());
    assert!(h.app.palette.is_none());
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "nothing has the keyboard, so the filter box was not given it"
    );
    h.frame(vec![Event::Text("b".to_owned())]);
    assert_eq!(h.tab(0).filter, "b", "the keystroke did not land in the filter box");
}

/// **Every entry has a name of its own**, for the recent list — two entries under one name would
/// remember each other.
#[test]
fn every_entry_has_its_own_name() {
    let h = on_three_files("palette-names");
    let mut names = std::collections::HashSet::new();
    for (command, _) in h.app.catalogue() {
        let key = command.key().expect("a command with no name");
        assert!(names.insert(key.clone()), "two entries are both called {key}");
    }
}

/// The actions [`listed`] promises the palette offers, read out of the function itself so that the
/// list is not written twice: every `Action::` named before its `=> true`.
fn promised() -> Vec<&'static str> {
    let source = include_str!("../palette.rs");
    let start = source
        .find("pub(crate) fn listed")
        .expect("`listed` is not in palette.rs");
    let end = start + source[start..].find("=> true").expect("`listed` has no `=> true` arm");
    source[start..end]
        .split("Action::")
        .skip(1)
        .map(|rest| rest.split(|c: char| !c.is_alphanumeric()).next().unwrap_or(rest))
        .collect()
}

/// Every entry the palette lists, resolved: each has to come to an action, and that action has to be
/// one [`listed`] says the palette offers.
fn reach(h: &Harness, reached: &mut std::collections::HashSet<&'static str>) {
    for (command, row) in h.app.catalogue() {
        let action = h
            .app
            .palette_action(&h.ctx, command, false)
            .unwrap_or_else(|| panic!("`{}` comes to nothing", row.label));
        assert!(
            listed(&action),
            "`{}` comes to {}, which `listed` says is not on the palette",
            row.label,
            action.name()
        );
        reached.insert(action.name());
    }
}

/// **Every action [`listed`] promises is on the palette**, and nothing on the palette comes to one it
/// does not. `listed` is exhaustive, so a new `Action` does not compile until it has been put in one
/// of its two lists; this is what makes the first list a promise rather than a label.
///
/// Entries are left off the palette when they cannot apply, so the catalogue is read in each of the
/// few states between which every entry appears: two folders selected, a file selected under the
/// cursor, a pane with two tabs and a closed one, a history to go back and forward in, and a folder diff. Five
/// commands need what a test cannot arrange — an update on offer, an operation to undo — and are
/// resolved directly instead, as is a tab to go to.
#[test]
fn every_listed_action_is_on_the_palette() {
    let root = crate::sandbox::fresh("palette-reach");
    for name in ["left", "right"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("notes.txt"), b"notes").unwrap();
    let mut h = Harness::opening(vec![root.clone()]);
    let ctx = h.ctx.clone();
    let pane = h.app.panes[0].id;
    let mut reached = std::collections::HashSet::new();
    // Sampled from the real clipboard every frame, so set after the last one.
    let settled = |h: &mut Harness| {
        h.settle();
        h.app.clipboard_has_files = true;
    };

    // The two folders selected, the cursor on one. Folders sort first, so they are rows 0 and 1.
    settled(&mut h);
    let tab = h.app.panes[0].tab_mut();
    for row in [0, 1] {
        tab.cursor = Some(row);
        tab.toggle_at_cursor();
    }
    tab.cursor = Some(0);
    reach(&h, &mut reached);

    // The file alone, selected and under the cursor.
    let tab = h.app.panes[0].tab_mut();
    tab.clear_selection();
    tab.cursor = Some(2);
    tab.toggle_at_cursor();
    reach(&h, &mut reached);

    // Two tabs and one closed, and a history either side of where the pane is.
    h.app.perform(&ctx, Action::NewTab { pane });
    h.app.perform(&ctx, Action::CloseTab { pane, tab: 1 });
    h.app.perform(&ctx, Action::NewTab { pane });
    for folder in ["left", "right"] {
        let path = root.join(folder);
        h.app.perform(&ctx, Action::Navigate { pane, path });
    }
    h.app.perform(&ctx, Action::Back(pane));
    settled(&mut h);
    reach(&h, &mut reached);

    // A folder diff, for its switches.
    h.app.perform(&ctx, Action::FolderDiff);
    settled(&mut h);
    reach(&h, &mut reached);

    for command in [
        Command::Undo,
        Command::Redo,
        Command::InstallUpdate,
        Command::ReleasePage,
        Command::SkipUpdate,
        Command::Tab { pane, tab: 0 },
    ] {
        let action = h.app.palette_action(&ctx, command.clone(), false).unwrap();
        assert!(listed(&action), "{command:?} comes to {}", action.name());
        reached.insert(action.name());
    }

    let promised = promised();
    assert!(promised.len() > 50, "only {} actions read out of `listed`", promised.len());
    let missing: Vec<&str> = promised
        .into_iter()
        .filter(|name| !reached.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "`listed` promises these and no palette entry comes to them: {missing:?}"
    );
}
