use super::*;

/// The *folder's* menu — the one an empty-space right click asks for, which is a different
/// shell object from a selection's and not the same one with no items in it.
///
/// What this checks is *which object came back*, and it checks it by verb, because a count
/// cannot tell them apart and for a long time nothing did. Asking `GetUIObjectOf` for the
/// folder-as-an-item returns a perfectly good menu — 29 entries, Cut, Copy, Delete, Rename,
/// 7-Zip, Send To — with no New anywhere in it, and every length assertion that used to be
/// here passed on it comfortably. So the background menu is identified by what it *lacks*:
/// it has Properties, and it has none of the item verbs. That is true of it on every Windows
/// and in every language, which is more than can be said for looking for a label.
#[test]
#[cfg(windows)]
fn a_folder_with_nothing_selected_gets_the_background_menu() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let dir = crate::sandbox::dir("folder-menu");
    std::fs::create_dir_all(&dir).expect("temp dir");

    // `build` fills every submenu on the way, which is what makes New's contents visible.
    let entries = build(&dir, &[]);
    // Worth printing whole on any failure: which menu this is, is the entire question.
    let shown = || {
        entries
            .iter()
            .map(|e| match &e.kind {
                Kind::Submenu { children, .. } => format!(
                    "{} > [{}]",
                    e.label,
                    children
                        .iter()
                        .map(|c| c.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Kind::Separator => "--".to_owned(),
                Kind::Command(_) => e.label.clone(),
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    assert!(!entries.is_empty(), "the folder's background menu came back empty");

    let verbs: Vec<String> = entries
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Command(Command::Shell { verb: Some(verb), .. }) => Some(verb.to_lowercase()),
            _ => None,
        })
        .collect();
    assert!(
        verbs.iter().any(|v| v == "properties"),
        "no Properties on it, so this is not a folder's menu at all: {}",
        shown()
    );
    for item_verb in ["cut", "copy", "delete", "rename", "link"] {
        assert!(
            !verbs.iter().any(|v| v == item_verb),
            "`{item_verb}` is on the background menu, so this is the folder-as-an-item menu \
             and New cannot be in it -- see `context_of`: {}",
            shown()
        );
    }

    // And New itself, found by verb and not by label -- it is "New" here and "Nouveau" on a
    // French Windows, whereas `NewFolder` and `NewLink` are the shell's own names for the two
    // entries it always starts with and are the same in every language.
    let new = entries
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Submenu { children, .. } => Some(children),
            _ => None,
        })
        .find(|children| {
            let has = |wanted: &str| {
                children.iter().any(|c| {
                    matches!(&c.kind, Kind::Command(Command::Shell { verb: Some(verb), .. })
                        if verb == wanted)
                })
            };
            has("NewFolder") && has("NewLink")
        })
        .unwrap_or_else(|| panic!("no New submenu on the folder's menu: {}", shown()));

    // The registered file types under it, which are the rest of what New is for. Each comes
    // with its extension as the canonical verb -- `.txt`, `.bmp` -- and that is also what
    // makes them safe to run: [`invoke`] takes the verb path, so it never has to reproduce
    // the numbering of a submenu it would have had to populate all over again to get right.
    let types: Vec<&str> = new
        .iter()
        .filter_map(|c| match &c.kind {
            Kind::Command(Command::Shell { verb: Some(verb), .. }) if verb.starts_with('.') => {
                Some(verb.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(
        !types.is_empty(),
        "New offers Folder and Shortcut and no file type at all, so the submenu was never \
         filled: {}",
        shown()
    );
    eprintln!("New offers {} file types: {types:?}", types.len());

    crate::sandbox::remove(&dir);
}

/// A temp folder with a file and a subfolder in it, for the tests that need something
/// real to right-click.
#[cfg(windows)]
fn scratch(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = crate::sandbox::dir(name);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("one.txt");
    std::fs::write(&file, b"x").expect("write");
    let sub = dir.join("folder");
    std::fs::create_dir_all(&sub).expect("subdir");
    (dir, file, sub)
}

/// Every canonical verb read off one menu, by label.
///
/// The labels are whatever language this Windows is in and are no use for recognising
/// anything — see [`Command::creates_an_item`] — so these tests match on the verb and carry
/// the label only to put in a failure message somebody has to read.
#[cfg(windows)]
fn verbs_of(parent: &Path, items: &[PathBuf]) -> Vec<(String, String)> {
    let Some((_live, entries)) = super::win::Live::open(parent, items, Depth::Full) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Kind::Command(Command::Shell { verb: Some(verb), .. }) => {
                Some((verb.clone(), entry.label.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The verbs [`crate::app::App::ours_rather_than_the_shell_s`] takes over are really the ones
/// the shell hands out.
///
/// Those hooks are keyed on `open`, `cut` and `copy`, and a hook keyed on a verb that has been
/// renamed does not fail — it *silently stops intercepting*, and the gesture goes back to
/// opening Windows Explorer or to putting the selection on the clipboard without fading the
/// rows. Nothing else in the program would notice, so this is the test that would.
///
/// `paste` is the fourth and is not here: the shell only offers it when there is something on
/// the clipboard, so it needs a test that takes the clipboard over. See
/// [`the_shell_offers_paste_on_a_folder_only_when_the_clipboard_has_files`].
#[test]
#[cfg(windows)]
fn the_verbs_this_program_takes_over_are_still_the_shell_s() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, sub) = scratch("hooked-verbs");

    for (what, items) in [
        ("a selected folder", vec![sub.clone()]),
        ("a selected file", vec![file.clone()]),
        ("both at once", vec![sub.clone(), file.clone()]),
    ] {
        let verbs = verbs_of(&dir, &items);
        for wanted in ["open", "cut", "copy"] {
            assert!(
                verbs.iter().any(|(verb, _)| verb == wanted),
                "the shell no longer offers `{wanted}` on {what}, so the hook that redirects \
                 it into this program is dead code. What it did offer: {:?}",
                verbs
            );
        }
    }

    crate::sandbox::remove(&dir);
}

/// `paste` is on a selected **folder**, not on the folder's background, and only while there
/// is something to paste.
///
/// Worth pinning down, because both halves are load-bearing and neither is obvious:
///
/// - **On the selection, not the background.** The background menu is the view object's — see
///   [`win::context_of`] — and Explorer synthesises its own Paste around that one rather than
///   reading it out of the shell. So there is no background Paste to redirect, and Ctrl+V is
///   what covers pasting into the folder you are looking at.
/// - **Only with a full clipboard.** The entry is absent rather than greyed, so a test that
///   ran with an empty clipboard would conclude the shell offers no Paste anywhere and the
///   hook was pointless. It is not: with files on the clipboard it is there, on every
///   selection that contains at least one folder.
#[test]
#[cfg(windows)]
#[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
fn the_shell_offers_paste_on_a_folder_only_when_the_clipboard_has_files() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, sub) = scratch("hooked-paste");
    let on_folder = || {
        verbs_of(&dir, std::slice::from_ref(&sub))
            .into_iter()
            .any(|(verb, _)| verb == "paste")
    };

    crate::shell::clipboard::clear();
    assert!(
        !on_folder(),
        "the shell offered `paste` with an empty clipboard, so the absence this program \
         relies on to tell 'nothing to paste' from 'no such verb' is not there"
    );

    crate::shell::clipboard::put(
        std::slice::from_ref(&file),
        crate::shell::clipboard::Effect::Copy,
    )
    .expect("put the file on the clipboard");
    assert!(
        crate::shell::clipboard::has_files(),
        "the clipboard did not take the file, so the rest of this proves nothing"
    );
    assert!(
        on_folder(),
        "the shell no longer offers `paste` on a selected folder with a full clipboard, so \
         the hook that redirects it into this program is dead code"
    );
    // And still not on the background, which is why Ctrl+V is the only paste there.
    assert!(
        !verbs_of(&dir, &[]).into_iter().any(|(verb, _)| verb == "paste"),
        "the folder's background menu has grown a Paste — this program synthesises none, so \
         it would now be showing Windows' one and pasting through the shell"
    );

    crate::shell::clipboard::settle_for_tests();
    crate::sandbox::remove(&dir);
}

/// The lazy half of the design: opening the menu must *not* fill the submenus, and
/// filling one afterwards must give what the eager path would have.
///
/// This is the saving that matters on a file -- Open With alone was 41-104 ms of the
/// build -- so a change that quietly went back to prefilling would show up here as a
/// submenu that already had children.
#[test]
#[cfg(windows)]
fn submenus_stay_empty_until_they_are_asked_for() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, _) = scratch("lazy");

    let (mut live, entries) =
        super::win::Live::open(&dir, std::slice::from_ref(&file), Depth::Full)
            .expect("the shell's menu");

    let submenus: Vec<(u32, String)> = entries
        .iter()
        .filter_map(|e| e.kind.unasked().map(|id| (id, e.label.clone())))
        .collect();
    assert!(
        !submenus.is_empty(),
        "a text file on any Windows has at least Open With or Send To: {:?}",
        entries.iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    for entry in &entries {
        if let Kind::Submenu { children, source } = &entry.kind {
            assert!(
                children.is_empty() && source.is_some(),
                "`{}` came back already filled -- opening the menu paid for a submenu \
                 nobody had opened",
                entry.label
            );
        }
    }

    // And asking works: at least one of them has something in it.
    let filled: Vec<(String, usize)> = submenus
        .iter()
        .map(|(id, label)| (label.clone(), live.fill(*id).len()))
        .collect();
    assert!(
        filled.iter().any(|(_, count)| *count > 0),
        "every submenu came back empty when asked, so the fill never reached the \
         extensions: {filled:?}"
    );

    crate::sandbox::remove(&dir);
}

/// Every command knows the route back to the `HMENU` it was read out of.
///
/// Which is what makes an entry with no canonical verb usable at all. `Send to > Documents`
/// and `Open with > Notepad` have none — the shell offers only a numeric id — and that id is
/// handed out by the extension when it *populates* the submenu. [`invoke`] builds the menu
/// afresh, where nothing has populated anything, so without the route down it hands over an
/// id nobody has assigned and the entry does nothing whatsoever. That was measured: 22 of the
/// 57 commands on a folder and 14 of the 53 on a file had no verb.
///
/// Positions and not entry indices, because they are not the same number: separators are
/// coalesced and unusable items dropped on the way out of [`Live::read`].
#[test]
#[cfg(windows)]
fn a_command_carries_the_route_back_to_its_submenu() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, _) = scratch("route");

    let (mut live, entries) =
        super::win::Live::open(&dir, std::slice::from_ref(&file), Depth::Full)
            .expect("the shell's menu");

    for entry in &entries {
        if let Kind::Command(Command::Shell { path, label, .. }) = &entry.kind {
            assert!(
                path.is_empty(),
                "`{label}` is at the top of the menu and thinks it is inside {path:?}"
            );
            assert_eq!(label, &entry.label, "a command was stamped with another's label");
        }
    }

    // Every submenu with anything in it, one level down.
    let mut checked = 0;
    for entry in &entries {
        let Some(id) = entry.kind.unasked() else { continue };
        let children = live.fill(id);
        let inside: Vec<&Vec<u32>> = children
            .iter()
            .filter_map(|c| match &c.kind {
                Kind::Command(Command::Shell { path, .. }) => Some(path),
                _ => None,
            })
            .collect();
        if inside.is_empty() {
            continue;
        }
        checked += 1;
        let first = inside[0].clone();
        assert_eq!(
            first.len(),
            1,
            "`{}` is one level down, so its entries' route is one position: {first:?}",
            entry.label
        );
        for path in inside {
            assert_eq!(
                *path, first,
                "two entries of `{}` disagree about which submenu they are in",
                entry.label
            );
        }

        // And one level deeper, where an off-by-one in appending to the trail would hide:
        // `7-Zip > CRC SHA > MD5` has to come back with both positions, outer first.
        for child in &children {
            let Some(id) = child.kind.unasked() else { continue };
            for deep in live.fill(id) {
                if let Kind::Command(Command::Shell { path, .. }) = &deep.kind {
                    assert_eq!(
                        path.len(),
                        2,
                        "`{} > {} > {}` is two levels down and reports {path:?}",
                        entry.label,
                        child.label,
                        deep.label
                    );
                    assert_eq!(
                        path[0], first[0],
                        "the deeper route does not start where the shallower one did"
                    );
                }
            }
        }
    }
    assert!(
        checked > 0,
        "a text file on any Windows has at least one submenu with commands in it"
    );

    crate::sandbox::remove(&dir);
}

/// The asynchronous half: the builder answers by channel, and a submenu asked for
/// after the fact comes back against the same token.
#[test]
#[cfg(windows)]
fn the_builder_answers_by_channel() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, _) = scratch("builder");

    let ctx = egui::Context::default();
    let mut builder = Builder::new(&ctx);

    // Wait for one answer. Generous, because the whole point is that this is slow.
    fn next(builder: &mut Builder) -> Said {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(said) = builder.poll() {
                return said;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the builder thread never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    let token = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);
    let entries = match next(&mut builder) {
        Said::Built { token: got, entries, .. } => {
            assert_eq!(got, token, "answered for a menu nobody asked for");
            entries
        }
        _ => panic!("the first answer to a build should be the menu"),
    };
    assert!(
        entries.len() > 3,
        "the builder came back with only {} entries",
        entries.len()
    );

    // The submenu is still on the builder's thread, and can be filled from here.
    let (id, label) = entries
        .iter()
        .find_map(|e| e.kind.unasked().map(|id| (id, e.label.clone())))
        .expect("a submenu");
    builder.fill(token, id);
    match next(&mut builder) {
        Said::Filled { token: got, id: got_id, children } => {
            assert_eq!(got, token);
            assert_eq!(got_id, id);
            assert!(!children.is_empty(), "`{label}` filled to nothing");
        }
        _ => panic!("expected the submenu"),
    }

    // Closing frees the shell's side; a fill against a closed token is still answered,
    // and answered emptily, rather than reaching a menu that is gone.
    builder.close(token);
    builder.fill(token, id);
    match next(&mut builder) {
        Said::Filled { children, .. } => assert!(children.is_empty()),
        _ => panic!("expected the submenu"),
    }

    crate::sandbox::remove(&dir);
}

/// Where the time in a context menu actually goes.
///
/// Not an assertion -- a measurement, and the one that decided the design of this
/// file. The numbers it printed are in the note at the top. What to look at: the whole
/// menu against the part the user waits for, which is now only `Live::open` reading the
/// top level, and the submenu fill that a hover pays for instead.
#[test]
#[ignore = "measures the shell; run explicitly, single-threaded, with --nocapture"]
#[cfg(windows)]
fn what_the_shell_menu_takes_to_build() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, sub) = scratch("menu-cost");

    for (what, parent, items) in [
        ("a text file", dir.clone(), vec![file.clone()]),
        ("a folder", dir.clone(), vec![sub.clone()]),
        ("empty space", dir.clone(), Vec::new()),
        (
            r"a folder in C:\",
            PathBuf::from(r"C:\"),
            vec![PathBuf::from(r"C:\Windows")],
        ),
    ] {
        for round in 1..=3 {
            let whole = std::time::Instant::now();
            let all = build(&parent, &items);
            let whole = whole.elapsed();

            let root = std::time::Instant::now();
            let Some((mut live, entries)) = super::win::Live::open(&parent, &items, Depth::Full) else {
                continue;
            };
            let root = root.elapsed();

            // Every submenu, one at a time, the way a hover pays for it.
            let mut fills = Vec::new();
            for entry in &entries {
                if let Some(id) = entry.kind.unasked() {
                    let at = std::time::Instant::now();
                    let children = live.fill(id);
                    fills.push((
                        entry.label.clone(),
                        children.len(),
                        at.elapsed().as_secs_f32() * 1e3,
                    ));
                }
            }

            eprintln!(
                "{what} #{round}: whole menu {:>7.1}ms ({} entries)   top level only \
                 {:>7.1}ms ({} entries)",
                whole.as_secs_f32() * 1e3,
                all.len(),
                root.as_secs_f32() * 1e3,
                entries.len()
            );
            for (label, count, ms) in fills {
                eprintln!("    fill `{label}` {ms:>7.1}ms ({count} children)");
            }
        }
    }

    crate::sandbox::remove(&dir);
}

/// The reduced menu has to still be a menu — the shell's own verbs, all of them working.
///
/// [`Depth::Fast`] exists because on an executable on a share the full query takes
/// twenty-four seconds and this one takes half of one. What it buys is worth nothing if what
/// comes back cannot cut, copy, delete, rename or open. Checked against a local file, where
/// both are fast, because what is being tested is the *content* of the reduced menu; the
/// timings are in the note on [`Depth`].
#[test]
#[cfg(windows)]
fn the_reduced_menu_is_still_a_menu() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, _) = scratch("reduced");

    let verbs = |depth: Depth| -> Vec<String> {
        let (_live, entries) = super::win::Live::open(&dir, std::slice::from_ref(&file), depth)
            .expect("the shell's menu");
        entries
            .iter()
            .filter_map(|e| match &e.kind {
                Kind::Command(Command::Shell { verb: Some(verb), .. }) => {
                    Some(verb.to_lowercase())
                }
                _ => None,
            })
            .collect()
    };
    let full = verbs(Depth::Full);
    let fast = verbs(Depth::Fast);

    // Everything anybody actually does to a file. `open` is deliberately in here: it is the
    // default verb, and a menu whose default is missing is not a context menu.
    for wanted in ["open", "cut", "copy", "delete", "rename", "properties"] {
        assert!(
            fast.iter().any(|v| v == wanted),
            "the reduced menu has no `{wanted}` in it, so it is not usable: {fast:?}"
        );
    }
    // And it really is reduced, or there would be no point to any of this.
    assert!(
        fast.len() < full.len(),
        "the reduced menu came back with as much as the full one ({} against {}), so the \
         flags did nothing: {fast:?}",
        fast.len(),
        full.len()
    );
    eprintln!("full {} verbs, reduced {} verbs", full.len(), fast.len());

    crate::sandbox::remove(&dir);
}

/// A menu the shell is taking for ever over must not be able to hold up the next one.
///
/// This is the whole reason [`Builder`] spawns a worker per menu. Right-clicking a 14 MB
/// executable on a mapped share puts `QueryContextMenu` inside one extension for
/// twenty-four seconds — measured; see the note on [`Builder`] — and with one thread and a
/// queue, every menu asked for in that time waited behind it. A right click on a local file
/// did nothing at all, which is what it looks like from the outside: the context menu has
/// stopped working.
///
/// The stall stands in for the share. What is being tested is not the shell's speed but the
/// scheduling: that the second answer arrives while the first build is demonstrably still
/// running, and that the first one's answer never turns up.
#[test]
#[cfg(windows)]
fn a_slow_menu_does_not_hold_up_the_next_one() {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, _) = scratch("cancel");

    let ctx = egui::Context::default();
    let mut builder = Builder::new(&ctx);

    /// Long enough that a real menu built inside it cannot be the stall finishing early,
    /// short enough that the abandoned worker is gone before the suite is.
    const STALL: u64 = 8_000;
    STALLED.store(0, Ordering::SeqCst);
    STALL_MS.store(STALL, Ordering::SeqCst);
    let slow = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);

    // Wait until the worker is really inside it, so what follows is a menu asked for
    // during a slow build rather than after one.
    let deadline = Instant::now() + Duration::from_secs(5);
    while STALLED.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "the worker never started the slow build"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // Cleared only now: the stall is a property of the *first* build, and clearing it before
    // that build had picked it up would race it.
    STALL_MS.store(0, Ordering::SeqCst);
    assert!(builder.busy(), "a build is outstanding and the builder says it is not");
    assert!(
        builder.poll().is_none(),
        "the slow build answered in no time, so it was not slow and this proves nothing"
    );

    // The second menu, asked for with the first still running.
    let at = Instant::now();
    let quick = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);
    let entries = loop {
        if let Some(said) = builder.poll() {
            match said {
                Said::Built { token, entries, .. } => {
                    assert_ne!(
                        token, slow,
                        "the abandoned build answered, and its answer was taken"
                    );
                    assert_eq!(token, quick);
                    break entries;
                }
                Said::Filled { .. } => {}
            }
        }
        assert!(
            at.elapsed() < Duration::from_millis(STALL / 2),
            "the second menu has been {:?} and has not arrived -- it is queued behind the \
             first, which is the bug",
            at.elapsed()
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let took = at.elapsed();

    assert!(
        entries.len() > 3,
        "the second menu came back with only {} entries, so it was not really built",
        entries.len()
    );
    assert!(
        !builder.busy(),
        "the second menu has been delivered and the builder still thinks it is waiting"
    );
    eprintln!("the second menu took {took:?} while the first had {STALL} ms left to run");

    crate::sandbox::remove(&dir);
}

/// Where the time goes on a file the shell has to reach across a network for.
///
/// `YAFE_PROBE="H:\some\file.exe" cargo test probe_menu_costs -- --ignored --nocapture`
#[test]
#[ignore = "measures the shell against a path of your choosing"]
#[cfg(windows)]
fn probe_menu_costs() {
    let Some(path) = std::env::var_os("YAFE_PROBE") else {
        eprintln!("set YAFE_PROBE to a file or folder");
        return;
    };
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    for path in path.to_string_lossy().split(';').filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        let parent = path.parent().unwrap_or(&path).to_owned();

        // The decision in `App::menu_depth` is made on the UI thread before anything slow is
        // allowed to happen, so what it costs is part of the claim.
        let at = std::time::Instant::now();
        let remote = crate::shell::over_network(&parent);
        let cold = at.elapsed();
        let at = std::time::Instant::now();
        let _ = crate::shell::over_network(&parent);
        eprintln!(
            "  over_network -> {remote}: {:.1} µs uncached, {:.1} µs cached",
            cold.as_secs_f64() * 1e6,
            at.elapsed().as_secs_f64() * 1e6
        );
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        eprintln!("\n--- {} ({} bytes)", path.display(), size);
        for _ in 1..=2 {
            let at = std::time::Instant::now();
            let opened = super::win::Live::open(&parent, std::slice::from_ref(&path), Depth::Full);
            let took = at.elapsed().as_secs_f32() * 1e3;
            match opened {
                Some((live, entries)) => {
                    eprintln!("  Live::open {took:>9.1} ms -> {} entries", entries.len());
                    drop(live);
                }
                None => eprintln!("  no menu at all ({took:.1} ms)"),
            }
        }
        super::win::probe(&parent, std::slice::from_ref(&path));
        super::win::probe_flags(&parent, std::slice::from_ref(&path));
    }
}

/// Does `InvokeCommand` accept the three kinds of entry that used to do nothing?
///
/// Each one really runs, so this launches whatever it launches — a shell, a terminal, an
/// editor — against a file inside the sandbox. Nothing here deletes, moves or copies
/// anything. What is being read is the `HRESULT`: the failures this is here for were silent,
/// and `InvokeCommand` refusing a verb is the only signal the shell gives.
///
/// - a plain registered verb on the folder's **background** menu, which is where `%V` and so
///   `lpDirectory` matter;
/// - an `IExplorerCommand` on a selected folder, whose canonical verb is a CLSID in braces
///   and which `CMF_OPTIMIZEFORINVOKE` used to leave out of the rebuilt menu entirely;
/// - an entry with **no** canonical verb inside a submenu, which can only be named by an id
///   that does not exist until the submenu has been populated.
#[test]
#[ignore = "probe: really runs the verbs, so windows open"]
#[cfg(windows)]
fn probe_invoking() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, sub) = scratch("invoking");

    /// The first entry, at any depth, that `pick` likes — with the path it was found under.
    fn find(
        entries: &[Entry],
        pick: &dyn Fn(&str, &Command) -> bool,
        prefix: &str,
    ) -> Option<(String, Command)> {
        for entry in entries {
            match &entry.kind {
                Kind::Command(command) if pick(&entry.label, command) => {
                    return Some((format!("{prefix}{}", entry.label), command.clone()));
                }
                Kind::Submenu { children, .. } => {
                    let deeper = format!("{prefix}{} > ", entry.label);
                    if let Some(found) = find(children, pick, &deeper) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }

    let cases: [(&str, Vec<PathBuf>, Box<dyn Fn(&str, &Command) -> bool>); 3] = [
        (
            "a plain verb on the folder's background",
            Vec::new(),
            Box::new(|_label: &str, c: &Command| {
                matches!(c, Command::Shell { verb: Some(v), .. } if v == "git_shell")
            }),
        ),
        (
            "an IExplorerCommand (CLSID verb) on a selected folder",
            vec![sub.clone()],
            Box::new(|label: &str, c: &Command| {
                label.contains("Terminal")
                    && matches!(c, Command::Shell { verb: Some(v), .. } if v.starts_with('{'))
            }),
        ),
        (
            "a no-verb entry inside a submenu, on a file",
            vec![file.clone()],
            Box::new(|label: &str, c: &Command| {
                (label.contains("Bloc-notes") || label.contains("Notepad"))
                    && matches!(c, Command::Shell { verb: None, path, .. } if !path.is_empty())
            }),
        ),
    ];

    for (what, items, pick) in cases {
        let entries = build(&dir, &items);
        match find(&entries, pick.as_ref(), "") {
            Some((label, command)) => {
                eprintln!("--- {what}\n    invoking `{label}`: {command:?}");
                invoke(&dir, &items, &command, Depth::Full, crate::shell::Owner::default());
            }
            None => eprintln!("--- {what}\n    not installed on this machine, skipped"),
        }
    }
    // The scratch folder is deliberately *not* removed: whatever was launched may still have
    // it open, and this is a probe somebody is watching rather than a test that has to tidy.
}

/// Which of a folder's two menus can actually run Properties, and the fix that follows.
///
/// The complaint was that Properties works on a selected item and does nothing on the folder
/// itself, and the reason is not a verb this program read wrongly — the shell **answers `S_OK`**
/// and shows nothing. There is no failure to fall back from and nothing to log, which is why the
/// entry was inert rather than noisy. So [`win::as_an_item`] invokes it against the folder as an
/// item, and this is the test of both halves of that claim.
///
/// A Properties sheet is a window rather than a return value, so what is asserted is the window.
/// Real sheets open while this runs and are closed on the way out, which is why it is `#[ignore]`d
/// — and why it is worth running by hand after touching any of `invoke`.
#[test]
#[ignore = "opens real Properties sheets; run explicitly, single-threaded"]
#[cfg(windows)]
fn properties_on_a_background_menu_goes_through_the_folder_as_an_item() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let dir = crate::sandbox::dir("bg-props");
    std::fs::write(dir.join("one.txt"), b"x").expect("write");
    let name = dir
        .file_name()
        .expect("a named folder")
        .to_string_lossy()
        .to_string();

    // Half one: the background menu's own Properties, invoked the way `invoke` used to. The
    // shell says it ran it — that is the assertion — and no sheet ever appears.
    assert!(
        super::win::probe_properties(&dir, &[]),
        "the background menu's `properties` was refused outright, which is not what was \
         measured: it answers S_OK and does nothing. The note on `win::as_an_item` needs \
         rewriting rather than trusting."
    );
    let none = wait_for_a_window(4);
    assert!(
        none.is_empty(),
        "the background menu's Properties has started working on its own, so the swap in \
         `win::as_an_item` is no longer needed: {none:?}"
    );

    // Half two: the same entry through `invoke`, which swaps the folder in as an item.
    let (_live, entries) =
        super::win::Live::open(&dir, &[], Depth::Full).expect("the background menu");
    let properties = entries
        .iter()
        .find_map(|e| match &e.kind {
            Kind::Command(command @ Command::Shell { verb: Some(verb), .. })
                if verb.eq_ignore_ascii_case("properties") =>
            {
                Some(command.clone())
            }
            _ => None,
        })
        .expect("every folder's background menu has Properties on it");
    invoke(&dir, &[], &properties, Depth::Full, crate::shell::Owner::default());

    let shown = wait_for_a_window(10);
    let closing: Vec<isize> = shown.iter().map(|(hwnd, _)| *hwnd).collect();
    let titles: Vec<&str> = shown.iter().map(|(_, title)| title.as_str()).collect();
    // Closed before the assertion, so a failure does not leave a sheet standing.
    close_these(&closing);
    assert!(
        titles.iter().any(|title| title.contains(&name)),
        "no Properties sheet for `{name}` turned up, so the folder's own Properties is doing \
         nothing again. Windows this process had: {titles:?}"
    );

    crate::sandbox::remove(&dir);
}

/// Every visible top-level window this process owns, as `(HWND, title)`.
#[cfg(windows)]
fn windows_of_this_process() -> Vec<(isize, String)> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };
    type Found = Vec<(isize, String)>;
    // SAFETY: called by `EnumWindows` for the duration of the call below, with `lparam`
    // pointing at the `Found` on that frame's stack and nothing else touching it.
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Found);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() && IsWindowVisible(hwnd).as_bool() {
            let mut text = [0u16; 256];
            let read = GetWindowTextW(hwnd, &mut text);
            out.push((hwnd.0 as isize, String::from_utf16_lossy(&text[..read as usize])));
        }
        TRUE
    }
    let mut out: Found = Vec::new();
    // SAFETY: the pointer is to `out`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut Found as isize));
    }
    out
}

/// Wait for a window of this process to turn up, up to `secs`.
///
/// **Answering calls while waiting, not sleeping.** The shell puts a property sheet on a thread
/// of its own, so `InvokeCommand` returns long before the sheet is on screen — measured, the call
/// came back in under a millisecond and the sheet arrived about a second later — and getting there
/// involves calls back into this apartment. A test that slept through them would be testing a
/// deadlock.
#[cfg(windows)]
fn wait_for_a_window(secs: u64) -> Vec<(isize, String)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let found = windows_of_this_process();
        if !found.is_empty() || std::time::Instant::now() >= deadline {
            return found;
        }
        crate::shell::answering_calls(100);
    }
}

/// Ask each of these windows to close, and give them a moment to.
#[cfg(windows)]
fn close_these(windows: &[isize]) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
    for hwnd in windows {
        // SAFETY: posting to a window of this process; a stale handle is answered with an error.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(*hwnd as *mut std::ffi::c_void)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
    crate::shell::answering_calls(300);
}

/// The redirect has somewhere to go: the folder as an item still offers `properties`.
///
/// [`win::as_an_item`] sends a background menu's Properties to the folder-as-an-item menu, by
/// verb. If the shell ever stopped putting `properties` on one of those two menus, nothing would
/// fail — the entry would go back to doing nothing at all, silently, which is the bug this was.
/// So both ends are pinned here, and cheaply: no command is invoked and no window opens.
#[test]
#[cfg(windows)]
fn both_of_a_folder_s_menus_carry_the_properties_verb() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let dir = crate::sandbox::dir("properties-verb");

    for (what, items) in [
        ("the folder's background", Vec::new()),
        ("the folder as an item", vec![dir.clone()]),
    ] {
        let verbs = verbs_of(&dir, &items);
        assert!(
            verbs
                .iter()
                .any(|(verb, _)| verb.eq_ignore_ascii_case("properties")),
            "no `properties` on {what}, so `win::as_an_item` redirects one nothing to another. \
             What it did offer: {verbs:?}"
        );
    }

    crate::sandbox::remove(&dir);
}

/// Only a *background* Properties is invoked against something else. Decided without the shell.
#[test]
#[cfg(windows)]
fn nothing_but_a_background_properties_is_swapped_for_the_folder() {
    use super::win::as_an_item;

    let folder = PathBuf::from(r"D:\Sources");
    let file = PathBuf::from(r"D:\Sources\one.txt");
    let swapped = [folder.clone()];

    assert_eq!(
        as_an_item(&folder, &[], Some("properties")).as_deref(),
        Some(swapped.as_slice()),
        "the folder's own Properties is the one entry that needs the swap"
    );
    // The verb is the shell's string, read back as it wrote it, so the case is not ours to rely
    // on — the same rule `super::properties_at` follows.
    assert_eq!(
        as_an_item(&folder, &[], Some("Properties")).as_deref(),
        Some(swapped.as_slice())
    );
    // A selection's Properties already works, and is about the selection rather than the folder.
    assert_eq!(
        as_an_item(&folder, std::slice::from_ref(&file), Some("properties")),
        None
    );
    // Everything else in a background menu runs against the folder it was read from.
    for verb in ["NewFolder", "NewLink", ".txt", "git_shell", "OpenWithCode", "paste", "view"] {
        assert_eq!(as_an_item(&folder, &[], Some(verb)), None, "`{verb}`");
    }
    // And an entry with no canonical verb is not guessed at.
    assert_eq!(as_an_item(&folder, &[], None), None);
}

/// Invoking a shell command has to actually do it, and the result has to be usable.
///
/// The menu's own Cut, Copy, Paste, Delete and Rename are the *shell's* entries, so they go
/// through [`invoke`] by canonical verb, on [`crate::shell::Modal`]. Copy is the one to test
/// with, because whether it worked is a fact about the clipboard rather than an opinion.
///
/// It did not work, and the way it failed is the point. `InvokeCommand` put the file on the
/// clipboard perfectly well -- read from the invoking thread it was right there -- and
/// `IsClipboardFormatAvailable` from anywhere else said the clipboard held no files at all.
/// Clipboard data belongs to the apartment that put it there, and reading it from elsewhere
/// is a call back into that apartment. The modal thread was parked in `recv()` and answered
/// nothing, so a Copy from the context menu was a copy nobody could paste. Hence the wait in
/// `Modal`'s loop; see [`crate::shell::answering_calls`].
///
/// Goes through the real `Modal` rather than a thread of its own, because a thread of its
/// own is what made this look like it worked: read the clipboard on the thread that wrote it
/// and everything is fine.
#[test]
#[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
#[cfg(windows)]
fn a_shell_verb_from_the_menu_actually_runs() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    crate::shell::clipboard::settle_for_tests();

    let (dir, file, _) = scratch("verb");

    let entries = build(&dir, std::slice::from_ref(&file));
    let copy = entries
        .iter()
        .find_map(|e| match &e.kind {
            Kind::Command(command @ Command::Shell { verb: Some(verb), .. })
                if verb.eq_ignore_ascii_case("copy") =>
            {
                Some(command.clone())
            }
            _ => None,
        })
        .expect("every file's menu has a Copy with a canonical verb");

    let ctx = egui::Context::default();
    let mut modal = crate::shell::Modal::new(&ctx);
    assert!(modal.send(crate::shell::Request::Invoke {
        parent: dir.clone(),
        items: vec![file.clone()],
        command: copy,
        depth: Depth::Full,
        owner: crate::shell::Owner::default(),
    }));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while modal.poll().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the modal thread never came back"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(
        crate::shell::clipboard::has_files(),
        "the shell's Copy ran and the clipboard says it holds no files -- the thread that \
         owns them is not answering for them"
    );
    let on_clipboard = crate::shell::clipboard::get()
        .expect("and it has to read back, since that is what a paste does");
    assert_eq!(on_clipboard.items.len(), 1, "{:?}", on_clipboard.items);
    assert!(
        on_clipboard.items[0]
            .to_string_lossy()
            .to_lowercase()
            .ends_with("one.txt"),
        "{:?}",
        on_clipboard.items
    );

    crate::shell::clipboard::clear();
    crate::sandbox::remove(&dir);
}

/// New is spotted by verb, because a label is whatever language Windows is in.
#[test]
fn the_shell_s_new_entries_are_recognised_by_verb_and_not_by_label() {
    let shell = |verb: Option<&str>| Command::Shell {
        verb: verb.map(str::to_owned),
        id: 0,
        path: Vec::new(),
        label: String::new(),
    };
    // The New submenu as it reads out of this machine, where the labels are `Dossier`,
    // `Raccourci`, `Document texte` and the verbs are these.
    for verb in ["NewFolder", "NewLink", ".txt", ".bmp", ".docx", ".library-ms", ".7z"] {
        assert!(shell(Some(verb)).creates_an_item(), "`{verb}` makes a file");
    }
    // The rest of a background menu, and the item menu's verbs with it.
    for verb in [
        "properties", "paste", "open", "cut", "copy", "delete", "rename", "link", "view", ".",
        "..", ".a b", "Open with Code",
    ] {
        assert!(
            !shell(Some(verb)).creates_an_item(),
            "`{verb}` does not make a file"
        );
    }
    // An extension offering no canonical verb cannot be told apart, and is not guessed at.
    assert!(!shell(None).creates_an_item());
    assert!(!Command::Own(Own::CopyHere).creates_an_item());
}

/// Re-measures the `CMF_OPTIMIZEFORINVOKE` table on [`super::invoke`]. See
/// [`super::win::probe_invokable`].
#[test]
#[ignore = "probe"]
#[cfg(windows)]
fn probe_invokable() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let (dir, file, sub) = scratch("invokable");
    // **`YAFE_PROBE` is checked before it is used, not after.** This test used to take the
    // variable, enumerate the menu, and finish with `std::fs::remove_dir_all(&dir)` to clear
    // the scratch folder up. Given `YAFE_PROBE=D:\Sources\MyTools\yet-another-file-explorer`
    // that last line deleted the repository. The cleanup is guarded now
    // ([`crate::sandbox::remove`]), so the damage cannot repeat either way — but failing here
    // says which variable is wrong before anything has run, rather than after.
    let dir = match std::env::var_os("YAFE_PROBE") {
        Some(given) => {
            let given = PathBuf::from(given);
            crate::sandbox::guard("YAFE_PROBE", &[given.clone()]);
            given
        }
        None => dir,
    };
    eprintln!("--- the folder's background menu of {}", dir.display());
    super::win::probe_invokable(&dir, &[]);
    eprintln!("--- a text file");
    super::win::probe_invokable(&dir, std::slice::from_ref(&file));
    eprintln!("--- a selected folder");
    super::win::probe_invokable(&dir, std::slice::from_ref(&sub));
    crate::sandbox::remove(&dir);
}

/// The right-drag answers, Paste and `Copy path(s)` are the only entries of our own.
///
/// This test is the fence around that, and the list is short on purpose: everything else this
/// program used to put above the shell's menu is gone, and a context menu that starts
/// collecting entries of its own again stops being the menu it claims to be. Each one is here
/// because of the question rather than because it was handy — "a right-button drag just landed",
/// Paste on empty space, which the shell's background menu does not carry, and the paths of the
/// selection written with the slash *this program* was told to write. See [`Own::Paste`] and
/// [`Own::CopyPaths`], where the case for each is made; `Copy path(s)` is the one that overlaps
/// something Windows offers, and the note there is why the overlap is not the whole story.
#[test]
fn the_drag_answers_paste_and_copy_paths_are_the_only_entries_of_our_own() {
    assert_eq!(Own::CopyHere.label(), "Copy here");
    assert_eq!(Own::MoveHere.label(), "Move here");
    assert_eq!(Own::LinkHere.label(), "Create shortcuts here");
    assert_eq!(Own::Cancel.label(), "Cancel");
    assert_eq!(Own::Paste.label(), "Paste");
    assert_eq!(Own::CopyPaths.label(), "Copy path(s)");
    let entry = Entry::own(Own::CopyHere);
    assert!(entry.enabled);
    assert!(!entry.checked);
    assert!(
        entry.shortcut.is_empty(),
        "a drop answer is not on a shortcut"
    );
}

/// The background menu gets a Paste and the shell's entries keep their order under it.
///
/// The greying is the part worth pinning: an entry that vanishes with an empty clipboard reads
/// as a program with no Paste at all, which is the complaint this whole entry exists to answer.
#[test]
fn the_background_menu_gets_this_program_s_paste() {
    let shell_entries = || {
        vec![
            Entry {
                label: "Nouveau".to_owned(),
                shortcut: String::new(),
                kind: Kind::unfilled(7),
                enabled: true,
                checked: false,
                icon: None,
            },
            Entry {
                label: "Propriétés".to_owned(),
                shortcut: String::new(),
                kind: Kind::Command(Command::Shell {
                    verb: Some("properties".to_owned()),
                    id: 0,
                    path: Vec::new(),
                    label: "Propriétés".to_owned(),
                }),
                enabled: true,
                checked: false,
                icon: None,
            },
        ]
    };

    let with = with_our_paste(shell_entries(), true);
    assert!(
        matches!(with[0].kind, Kind::Command(Command::Own(Own::Paste))),
        "Paste is not the first entry of the background menu"
    );
    assert_eq!(with[0].label, "Paste");
    assert!(with[0].enabled, "there was something to paste and it was greyed");
    assert!(
        matches!(with[1].kind, Kind::Separator),
        "nothing divides ours from the shell's"
    );
    assert_eq!(
        with[2..].iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
        ["Nouveau", "Propriétés"],
        "the shell's own entries were reordered or lost"
    );
    // The submenu is still filled by its opaque id, which is the thing that used to break the
    // moment anything was put above the shell's entries. See the note on `Kind::Submenu`.
    assert_eq!(with[2].kind.unasked(), Some(7));

    // Nothing to paste greys it rather than removing it.
    let empty = with_our_paste(shell_entries(), false);
    assert!(
        matches!(empty[0].kind, Kind::Command(Command::Own(Own::Paste))),
        "Paste disappeared when the clipboard was empty"
    );
    assert!(!empty[0].enabled, "Paste was offered with nothing to paste");

    // And no separator dangling off a menu the shell gave nothing for.
    let alone = with_our_paste(Vec::new(), true);
    assert_eq!(alone.len(), 1, "a separator with nothing under it");
}

/// `Copy path(s)` goes in directly above Properties, and nothing else moves.
///
/// Directly above, because that is what the drawing code pins with it: `ui::menu::pinned_from`
/// starts the unscrollable tail at the divider above them both, and a row anywhere else in the level
/// would be inside the part that scrolls. The two agree about which row Properties is by asking
/// [`properties_at`] rather than each testing the verb for itself.
///
/// The rest of it is [`with_our_paste`]'s claim again, and it has to be made again because it is
/// about a *different* operation: inserting into the middle of the level cannot disturb the shell's
/// entries, because nothing in one is a position in this list — an id and a submenu path are
/// positions in the shell's own `HMENU`, and a submenu's `source` is an opaque number.
#[test]
fn our_copy_paths_goes_in_directly_above_properties() {
    let shell = |label: &str, verb: &str| Entry {
        label: label.to_owned(),
        shortcut: String::new(),
        kind: Kind::Command(Command::Shell {
            verb: Some(verb.to_owned()),
            id: 42,
            path: vec![3],
            label: label.to_owned(),
        }),
        enabled: true,
        checked: false,
        icon: None,
    };
    let submenu = |label: &str, source: u32| Entry {
        label: label.to_owned(),
        shortcut: String::new(),
        kind: Kind::unfilled(source),
        enabled: true,
        checked: false,
        icon: None,
    };
    // A menu shaped like the real thing: entries, a submenu, the divider, Properties.
    let menu = || {
        vec![
            shell("Ouvrir", "open"),
            submenu("Envoyer vers", 7),
            Entry::separator(),
            shell("Propriétés", "properties"),
        ]
    };

    let with = with_our_copy_paths(menu());
    assert_eq!(
        with.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
        ["Ouvrir", "Envoyer vers", "", "Copy path(s)", "Propriétés"],
        "the entry is not between Properties and the divider above it"
    );
    assert!(matches!(
        with[3].kind,
        Kind::Command(Command::Own(Own::CopyPaths))
    ));
    assert!(with[3].enabled);
    // The shell's entries came through untouched — the id and the submenu path a command is
    // invoked by are positions in the shell's `HMENU` and not in this list, and the submenu is
    // still asked for by its opaque id.
    assert_eq!(with[1].kind.unasked(), Some(7));
    match &with[0].kind {
        Kind::Command(Command::Shell { id, path, verb, .. }) => {
            assert_eq!((*id, path.as_slice(), verb.as_deref()), (42, [3].as_slice(), Some("open")));
        }
        other => panic!("the shell's Open came out as {other:?}"),
    }

    // Recognised by **verb**, so an extension whose label happens to read Properties does not
    // become the anchor: this one goes above the shell's, at the bottom.
    let mut impostor = menu();
    impostor.insert(1, shell("Properties", "com.example.props"));
    let with = with_our_copy_paths(impostor);
    assert_eq!(
        with.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
        ["Ouvrir", "Properties", "Envoyer vers", "", "Copy path(s)", "Propriétés"]
    );

    // And a menu with no Properties at all takes it on the end rather than losing it. No Windows
    // has handed one of those over — `CMF_DEFAULTONLY` and `CMF_NOVERBS` both keep Properties — but
    // a menu is somebody else's list and the entry has to go somewhere.
    let none = with_our_copy_paths(vec![shell("Ouvrir", "open")]);
    assert_eq!(
        none.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
        ["Ouvrir", "Copy path(s)"]
    );
    let nothing = with_our_copy_paths(Vec::new());
    assert_eq!(nothing.len(), 1, "the entry went missing with nothing to put it beside");
}

#[test]
#[cfg(windows)]
fn shell_labels_lose_their_ampersands_and_keep_their_shortcuts() {
    use super::win::split_label;
    assert_eq!(split_label("&Open"), ("Open".to_owned(), String::new()));
    assert_eq!(
        split_label("Cu&t\tCtrl+X"),
        ("Cut".to_owned(), "Ctrl+X".to_owned())
    );
    // `&&` is a literal ampersand, which "Scan && Repair" depends on.
    assert_eq!(
        split_label("Scan && Repair"),
        ("Scan & Repair".to_owned(), String::new())
    );
    assert_eq!(split_label(""), (String::new(), String::new()));
}

/// The chain that actually breaks: paths, the parent folder, the shell's
/// `IContextMenu`, a populated `HMENU`, and reading it back into entries.
#[test]
#[cfg(windows)]
fn the_shell_fills_a_menu_that_reads_back() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let dir = crate::sandbox::dir("menu");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("one.txt");
    std::fs::write(&file, b"x").expect("write");

    let entries = build(&dir, std::slice::from_ref(&file));
    assert!(
        entries.len() > 3,
        "the shell offered only {} entries -- Open, Cut, Copy, Delete, Rename and \
         Properties should all be there at a minimum",
        entries.len()
    );

    // Every entry has to be drawable and doable.
    for entry in &entries {
        match &entry.kind {
            Kind::Separator => {}
            Kind::Submenu { children, .. } => {
                assert!(!children.is_empty(), "{}", entry.label)
            }
            Kind::Command(_) => assert!(!entry.label.is_empty()),
        }
        assert!(
            !entry.label.contains('&') || entry.label.matches('&').count() == 1,
            "`{}` still has an accelerator marker in it",
            entry.label
        );
        assert!(!entry.label.contains('\t'), "`{}`", entry.label);
    }

    // The commands worth having should be recognisable by verb.
    let verbs: Vec<String> = entries
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Command(Command::Shell { verb, .. }) => verb.clone(),
            _ => None,
        })
        .collect();
    assert!(
        verbs.iter().any(|v| v.eq_ignore_ascii_case("properties")),
        "no Properties verb among {verbs:?}"
    );
    assert!(
        verbs.iter().any(|v| v.eq_ignore_ascii_case("copy")),
        "no Copy verb among {verbs:?}"
    );

    // And the folder's own menu, which is a different shell object and not this one with the
    // items left out. Only that it answers at all is checked here; *which* object answered is
    // `a_folder_with_nothing_selected_gets_the_background_menu`, since a count cannot say.
    assert!(
        !build(&dir, &[]).is_empty(),
        "the folder's background menu came back empty"
    );

    crate::sandbox::remove(&dir);
}

#[test]
#[cfg(windows)]
fn submenus_are_populated_rather_than_left_empty() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let dir = crate::sandbox::dir("submenu");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("one.txt");
    std::fs::write(&file, b"x").expect("write");

    let entries = build(&dir, std::slice::from_ref(&file));
    let submenus: Vec<&Entry> = entries
        .iter()
        .filter(|e| matches!(e.kind, Kind::Submenu { .. }))
        .collect();
    // A plain text file on any Windows has at least "Open with" or "Send to".
    assert!(
        !submenus.is_empty(),
        "no submenu came back at all, which means `WM_INITMENUPOPUP` is not reaching \
         the extensions: {:?}",
        entries.iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    for submenu in submenus {
        if let Kind::Submenu { children, .. } = &submenu.kind {
            assert!(
                !children.is_empty(),
                "`{}` came back empty -- the submenu was never asked to fill itself",
                submenu.label
            );
        }
    }

    crate::sandbox::remove(&dir);
}
