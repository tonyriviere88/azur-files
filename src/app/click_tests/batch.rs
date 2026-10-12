//! The batch rename panel: what opens it, what it refuses, and what Rename hands the shell.
//!
//! The fixture is in the sandbox, and nothing here renames anything: under `cfg(test)` a job
//! reports back done without the shell being asked — see `shell::ops::FOR_REAL`. What is tested is
//! everything before that: the panel, the preview, the refusals and the keys.

use super::*;

/// Three files in the sandbox, listed, all selected.
fn three_selected(h: &mut Harness, name: &str) -> (crate::pane::PaneId, PathBuf) {
    let root = crate::sandbox::dir(name);
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).unwrap();
    for file in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(file), file.as_bytes()).unwrap();
    }
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();
    h.app.perform(&h.ctx.clone(), Action::SelectAll(pane));
    h.frame(Vec::new());
    assert_eq!(h.tab(0).selected_count, 3);
    (pane, root)
}

fn key(key: egui::Key) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

fn says(h: &Harness, words: &str) -> bool {
    h.texts().iter().any(|(_, text)| text.contains(words))
}

/// **F2 over several items opens the panel, with the caret in Find; over one, it is the inline
/// field it always was.**
#[test]
#[cfg(windows)]
fn f2_over_a_selection_opens_the_batch_panel() {
    let mut h = Harness::new();
    let (pane, root) = three_selected(&mut h, "batch-open");

    h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
    h.frame(Vec::new());
    h.frame(Vec::new());
    let batch = h.tab(0).batch.as_ref().expect("the panel did not open");
    assert_eq!(batch.items.len(), 3);
    assert!(h.tab(0).renaming.is_none(), "an inline field opened as well");
    assert!(says(&h, "Rename 3 items"), "the panel is not drawn");
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "the caret is not in the panel"
    );

    // Escape closes it, renaming nothing.
    h.take_journal();
    h.frame(vec![key(egui::Key::Escape)]);
    h.frame(Vec::new());
    assert!(h.take_journal().contains(&"BatchCancel"));
    assert!(h.tab(0).batch.is_none());

    // One selected: the inline field.
    h.app.panes[0].tab_mut().select_only(0);
    h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
    assert!(h.tab(0).batch.is_none());
    assert!(h.tab(0).renaming.is_some());

    crate::sandbox::remove(&root);
}

/// **The mode clicked is remembered when it is clicked**, not when something is renamed: the panel
/// is cancelled, and the next F2 still opens in it.
#[test]
#[cfg(windows)]
fn the_mode_clicked_is_the_one_the_panel_opens_in_next_time() {
    let mut h = Harness::new();
    let (pane, root) = three_selected(&mut h, "batch-mode");
    h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
    h.frame(Vec::new());
    assert_eq!(
        h.tab(0).batch.as_ref().unwrap().rule.mode,
        crate::pane::batch::Mode::Replace
    );

    // The button, by the word on it — clicked for real, so this is the rect that answers.
    let at = h
        .texts()
        .into_iter()
        .find(|(_, text)| text == "Pattern")
        .map(|(at, _)| at)
        .expect("the Pattern button is not drawn");
    h.wait();
    let done = h.click_at(at + vec2(4.0, 4.0));
    assert!(done.contains(&"SetBatchMode"), "the click did nothing: {done:?}");
    assert_eq!(
        h.tab(0).batch.as_ref().unwrap().rule.mode,
        crate::pane::batch::Mode::Pattern
    );
    // What the settings file would say now — `config_dirty` itself is already spent, because the
    // app writes within the frame the change landed in.
    assert_eq!(
        h.app.settings().batch_mode,
        crate::pane::batch::Mode::Pattern,
        "the settings would not say Pattern"
    );

    // Cancelled — nothing renamed — and opened again: the mode stands.
    h.frame(vec![key(egui::Key::Escape)]);
    h.frame(Vec::new());
    assert!(h.tab(0).batch.is_none());
    h.app.perform(&h.ctx.clone(), Action::SelectAll(pane));
    h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
    assert_eq!(
        h.tab(0).batch.as_ref().unwrap().rule.mode,
        crate::pane::batch::Mode::Pattern
    );

    crate::sandbox::remove(&root);
}

/// **The preview says what will happen, a conflict blocks Rename, and Rename hands the shell one
/// job with the names the preview showed.**
#[test]
#[cfg(windows)]
fn a_conflict_blocks_rename_and_a_clear_rule_renames_in_one_job() {
    let mut h = Harness::new();
    let (pane, root) = three_selected(&mut h, "batch-rule");
    h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
    h.frame(Vec::new());

    // `b` → `a`: lands on `a.txt`, which stays.
    {
        let batch = h.app.panes[0].tab_mut().batch.as_mut().unwrap();
        batch.rule.find = "b".to_owned();
        batch.rule.replace = "a".to_owned();
    }
    h.frame(Vec::new());
    assert!(says(&h, "1 problem"), "the conflict is not reported");
    // Enter does nothing while a row has a problem.
    h.take_journal();
    h.frame(vec![key(egui::Key::Enter)]);
    h.frame(Vec::new());
    assert!(!h.take_journal().contains(&"BatchApply"), "Enter renamed over a conflict");
    assert!(h.tab(0).batch.is_some(), "the panel closed on a refused Rename");

    // While the panel is open, the listing's own keys are not the listing's: a Delete would act on
    // a selection nobody can see.
    h.frame(vec![key(egui::Key::Delete)]);
    h.frame(Vec::new());
    assert!(!h.take_journal().contains(&"Delete"), "Delete reached the hidden listing");

    // A rule that is clear: a pattern over all three.
    {
        let batch = h.app.panes[0].tab_mut().batch.as_mut().unwrap();
        batch.rule.mode = crate::pane::batch::Mode::Pattern;
        batch.rule.pattern = "photo_{n:03}".to_owned();
    }
    h.frame(Vec::new());
    assert!(says(&h, "3 of 3 will be renamed"), "the summary is wrong");
    assert!(says(&h, "photo_001.txt"), "the preview does not show the new names");

    h.take_journal();
    h.frame(vec![key(egui::Key::Enter)]);
    h.frame(Vec::new());
    assert!(h.take_journal().contains(&"BatchApply"));
    let tab = h.tab(0);
    assert!(tab.batch.is_none(), "the panel stayed open after Rename");
    // The renamed files are what will be selected once the folder is re-read.
    let mut keep = tab.keep_selected.clone();
    keep.sort();
    assert_eq!(keep, ["photo_001.txt", "photo_002.txt", "photo_003.txt"]);

    crate::sandbox::remove(&root);
}
