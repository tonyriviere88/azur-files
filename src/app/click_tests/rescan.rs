//! A folder changing underneath the listing: what is re-read, and what the view keeps.

use super::*;

/// A folder waiting to name a new file is not silenced by the git-write filter.
///
/// The filter drops a folder's own change for [`GIT_WRITE_SETTLE`] after git last answered,
/// because `git::read` writes the index and that write comes straight back through the folder's
/// own watch — a loop that never stops on its own. The documented cost is that a real change in
/// the same window is missed until something else touches the folder.
///
/// For `New >` that cost is the whole feature. Measured before this: git answered at 1.23 s and
/// the shell's file landed before 2.57 s, so in **any** folder git has something to say about —
/// which is most of the ones this program gets used in — the new file did not appear at all.
///
/// Shell-free on purpose: a plain write is the same notification, so this runs in the normal
/// suite. `git_settled_at` is re-stamped every round to hold the filter's condition open, which
/// makes the two halves a clean A/B on `name_the_new` alone rather than a race with the clock.
#[test]
#[cfg(windows)]
fn a_folder_waiting_for_a_new_file_hears_about_it_through_the_git_filter() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("new-through-filter");
    crate::sandbox::remove(&dir);
    std::fs::create_dir_all(&dir).expect("sandbox");
    std::fs::write(dir.join("already.txt"), b"x").expect("a file");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: dir.clone(),
        },
    );
    h.settle();

    // `git_settled_at` is the whole of what arms the filter — it does not also check that there
    // is a repository — so stamping it stands in for git having just answered, without waiting
    // on a real `git status` and without making this a race against the clock. Re-stamped every
    // round, so the window stays open for as long as the round loop runs.
    let run = |h: &mut Harness, rounds: usize| {
        for _ in 0..rounds {
            h.app.panes[0].tab_mut().git_settled_at = Some(h.time);
            std::thread::sleep(std::time::Duration::from_millis(15));
            h.time += 0.05;
            h.frame(Vec::new());
        }
    };

    // The control: nothing is waiting, so the filter does what it is there for.
    std::fs::write(dir.join("unasked.txt"), b"y").expect("a file");
    run(&mut h, 60);
    assert_eq!(
        h.tab(0).names(),
        ["already.txt".to_owned()],
        "the filter is not armed, so this test's other half proves nothing"
    );

    // And the same notification, with a name waiting to be given.
    h.app.panes[0].tab_mut().name_the_new = Some(h.tab(0).names());
    std::fs::write(dir.join("asked-for.txt"), b"z").expect("a file");
    run(&mut h, 60);

    let (entry, text) = h
        .tab(0)
        .renaming
        .clone()
        .expect("the change was swallowed, so the new file never arrived to be named");
    assert_eq!(
        h.tab(0).dir.as_ref().expect("a listing").leaf(entry),
        "asked-for.txt",
        "the wrong row is being renamed"
    );
    assert_eq!(text, "asked-for.txt");
    assert!(
        h.tab(0).name_the_new.is_none(),
        "the snapshot has to be consumed, or the filter stays bypassed for good"
    );

    crate::sandbox::remove(&dir);
}

/// A folder that changed on disk is re-read without blanking what is on screen.
///
/// `Tab::refresh` drops the listing, which is right for F5 — somebody asked, and a moment
/// of "Reading..." is the honest answer. It is wrong for a watcher: a build writing into the
/// folder would flash the pane on every file. So the old rows stay up until the new ones
/// land, and the selection comes across with them.
#[test]
fn a_changed_folder_is_re_read_without_blanking_it() {
    let mut h = Harness::new();
    h.settle();
    let path = h.tab(0).path.clone();
    assert!(h.tab(0).order.len() > 2, "the crate root has rows");
    h.app.panes[0].tab_mut().select_only(1);
    let chosen = {
        let tab = h.tab(0);
        tab.entry_at(1)
            .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
    };
    assert!(chosen.is_some(), "and a row to select");

    h.app.folder_changed(&path);
    assert!(
        h.tab(0).dir.is_some(),
        "the rows on screen have to stay on screen"
    );
    assert!(
        h.tab(0).awaiting.is_some(),
        "and a fresh read has to be on its way"
    );
    assert!(
        h.app.loader.cached(&path).is_none(),
        "with the cached copy dropped, or the re-read hands back what it already had \
         and the change is never seen"
    );

    h.settle();
    assert!(h.tab(0).dir.is_some(), "the re-read landed");
    assert_eq!(
        h.tab(0).selected_count,
        1,
        "and what was selected is still selected"
    );
    let after = {
        let tab = h.tab(0);
        tab.cursor
            .and_then(|at| tab.entry_at(at))
            .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
    };
    assert_eq!(after, chosen, "and it is the same row, by name");
}

/// Two folders of enough files to scroll, in the crate's own `target`.
#[cfg(windows)]
fn tall_sandbox(name: &str, files: usize) -> PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join(name);
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    for i in 0..files {
        std::fs::write(root.join(format!("file-{i:02}.txt")), b"x").expect("a file");
    }
    root
}

/// Where the listing is scrolled to survives everything except going somewhere else.
///
/// Two claims, and they pull in opposite directions, which is why they are one test:
///
/// - **A re-read keeps its place.** Every file operation ends in one, and so does anything
///   the context menu does — so a scroll that does not survive it means acting on a file
///   two hundred rows down and being sent back to the top to find it again.
/// - **A different folder starts at the top.** Nothing else makes sense: row 200 of the
///   folder you just left is not row 200 of anything.
#[test]
#[cfg(windows)]
fn a_re_read_keeps_its_place_and_a_new_folder_does_not() {
    let one = tall_sandbox("scroll-a", 80);
    let two = tall_sandbox("scroll-b", 80);

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let go = |h: &mut Harness, path: &std::path::Path| {
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: path.to_path_buf(),
            },
        );
        h.settle();
    };
    go(&mut h, &one);

    // Down a long way, the way the wheel does it.
    h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(30.0 * crate::pane::ROW_HEIGHT);
    h.frame(Vec::new());
    h.frame(Vec::new());
    let was = h.tab(0).scroll_y;
    assert!(was > 100.0, "the listing did not scroll at all ({was})");

    // A re-read: what every file operation and every context-menu action ends in.
    h.app.perform(&h.ctx.clone(), Action::Refresh(pane));
    h.settle();
    assert_eq!(
        h.tab(0).scroll_y, was,
        "a re-read of the same folder moved the listing"
    );

    // And the right click that asks for a context menu, which is where this was reported:
    // the menu is what the user was looking at, and the listing behind it had gone back to
    // the top.
    let row = h.row_center(0, 4);
    h.click_with(row, PointerButton::Secondary, Modifiers::NONE);
    assert_eq!(
        h.tab(0).scroll_y, was,
        "the right click itself sent the listing back to the top"
    );
    // And once the menu is actually on screen, which is a few hundred milliseconds later:
    // the shell builds it off-thread, so the frames above have only asked for it.
    let waited = std::time::Instant::now();
    while h.app.menu_pending() {
        h.frame(Vec::new());
        assert!(
            waited.elapsed() < std::time::Duration::from_secs(20),
            "the builder never delivered"
        );
    }
    h.frame(Vec::new());
    assert!(h.app.menu.is_some(), "the menu is not up, so this proves nothing");
    assert_eq!(
        h.tab(0).scroll_y, was,
        "the menu appearing sent the listing back to the top"
    );
    h.app.close_menu();
    h.frame(Vec::new());
    assert_eq!(
        h.tab(0).scroll_y, was,
        "dismissing the menu sent the listing back to the top"
    );

    // Somewhere else, and back: both start at the top.
    go(&mut h, &two);
    assert_eq!(
        h.tab(0).scroll_y, 0.0,
        "a different folder opened part-way down"
    );
    go(&mut h, &one);
    assert_eq!(
        h.tab(0).scroll_y, 0.0,
        "coming back to a folder opened where the last visit left it"
    );

    // And each tab keeps its own place, which is the same fact from the other side: the
    // offset egui remembers belongs to the pane, so without this a tab coming to the front
    // shows wherever the tab before it had got to.
    h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(20.0 * crate::pane::ROW_HEIGHT);
    h.frame(Vec::new());
    h.frame(Vec::new());
    let deep = h.tab(0).scroll_y;
    assert!(deep > 100.0, "the first tab did not scroll ({deep})");

    h.app.perform(&h.ctx.clone(), Action::NavigateNewTab { pane, path: two.clone() });
    h.settle();
    assert_eq!(h.tab(0).scroll_y, 0.0, "a new tab opened part-way down");

    h.app.perform(&h.ctx.clone(), Action::ActivateTab { pane, tab: 0 });
    h.frame(Vec::new());
    h.frame(Vec::new());
    assert_eq!(
        h.tab(0).scroll_y, deep,
        "coming back to the first tab lost where it was"
    );

    crate::sandbox::remove(&one);
    crate::sandbox::remove(&two);
}

/// A change made by anybody re-reads the folder on its own.
///
/// **This is what a context-menu action now relies on.** Invoking a shell verb used to
/// re-read the folder unconditionally, because there is no way to be told what the verb did
/// — so `Copy`, `Properties` and `Open with` each threw the listing away and built it again
/// to discover that nothing had changed. Nothing does that any more, which is only correct
/// because the folder is watched: this is the test that says the watching works, end to end,
/// through a real `ReadDirectoryChangesW` handle and a real file appearing.
#[test]
#[cfg(windows)]
fn a_change_on_disk_re_reads_the_folder_by_itself() {
    let root = tall_sandbox("watched", 3);

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();
    assert_eq!(h.tab(0).order.len(), 3, "the sandbox should have three files");

    // Somebody else's change — a shell verb, Explorer, a terminal. `std::fs`, never the
    // shell: see `shell::ops::FOR_REAL`.
    std::fs::write(root.join("arrived.txt"), b"x").expect("a fourth file");

    let waited = std::time::Instant::now();
    while h.tab(0).order.len() != 4 {
        h.frame(Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(
            waited.elapsed() < std::time::Duration::from_secs(10),
            "the folder was never re-read: still {} rows",
            h.tab(0).order.len()
        );
    }

    crate::sandbox::remove(&root);
}

/// A folder that reads quickly says nothing about reading.
///
/// `Reading…` used to be drawn the moment a folder was asked for, and a local folder comes
/// back in single-digit milliseconds — so it was a word that flashed up and vanished on every
/// navigation, in the exact place the listing was about to be. It now waits for
/// [`crate::pane::SLOW_SCAN`], which is what the second half of this checks: silence is not
/// the same thing as never saying it.
#[test]
#[cfg(windows)]
fn a_quick_folder_never_says_it_is_reading() {
    let root = tall_sandbox("quick", 4);
    let reading = |h: &Harness| h.texts().iter().any(|(_, text)| text.contains("Reading"));

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    assert!(!reading(&h), "it starts by claiming to read something");

    // Somewhere it has never been, so the scan really goes to the loader rather than coming
    // straight back out of its cache.
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    // Real sleeps between frames, so the scan lands in a frame or two rather than in fifty:
    // the clock this is asserting against is egui's, which the harness advances by a
    // sixtieth per frame, so a listing that took fifty frames to arrive would be half a
    // *second* as far as the program is concerned and would be right to say so.
    let started = h.time;
    for _ in 0..12 {
        h.frame(Vec::new());
        assert!(
            !reading(&h),
            "a folder that read in under {}s said it was reading",
            crate::pane::SLOW_SCAN
        );
        if h.tab(0).dir.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    assert!(h.tab(0).dir.is_some(), "the listing never arrived");
    assert!(
        h.time - started < crate::pane::SLOW_SCAN,
        "the frames above took {:.2}s of the program's own time, which is past the \
         threshold — so the assertion inside the loop proved nothing",
        h.time - started
    );

    // And a scan that *is* slow says so. Faked by putting the clock back rather than by
    // finding a slow disk: `awaiting` is set so the frame does not start a new scan and
    // stamp the time again.
    {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.refresh();
        tab.awaiting = Some(u64::MAX);
        tab.asked_at = Some(h.time - crate::pane::SLOW_SCAN - 1.0);
    }
    h.frame(Vec::new());
    assert!(
        reading(&h),
        "a scan a second old still has not admitted to waiting"
    );

    crate::sandbox::remove(&root);
}
