//! The context menu, end to end: this program's half drawn immediately, the shell's filled in
//! when it answers.

use super::*;

/// The whole chain, through the app: a right click opens a menu straight away, the
/// builder thread fills in the shell's entries, and hovering a submenu fetches that.
///
/// The pieces are tested where they live -- `shell::menu` for the shell, `ui::menu` for
/// the drawing -- and what is only tested here is the wiring between them: `pump_menu`
/// taking delivery against the right token, and draining what the menu asked for on the
/// way back out. That wiring is easy to get subtly wrong and impossible to notice,
/// because a menu that never fills its submenus looks exactly like a menu whose
/// submenus are empty.
#[test]
#[cfg(windows)]
fn a_right_click_opens_a_menu_now_and_fills_it_from_the_shell() {
    let _serialised = crate::shell::serialised();
    let mut h = Harness::new();

    // What a baseline frame of this window costs, to compare the asking one against.
    let mut baseline = std::time::Duration::ZERO;
    for _ in 0..5 {
        let at = std::time::Instant::now();
        h.frame(Vec::new());
        baseline = baseline.max(at.elapsed());
    }

    h.app.open_folder_menu(&h.ctx.clone());
    let at = std::time::Instant::now();
    h.frame(Vec::new());
    let asking = at.elapsed();
    // The cheapest `QueryContextMenu` measured on this machine was 130 ms, for a folder;
    // a file was 130-690. So a frame that raised the menu and came in well under a tenth
    // of a second did not ask the shell anything on the way, which is the whole point.
    // Compared against a frame of the same window rather than against a constant, because
    // the constant that matters is the shell's and this bound only has to be under it.
    assert!(
        asking < baseline + std::time::Duration::from_millis(100),
        "the frame that raised the menu took {asking:?} against a {baseline:?} baseline \
         -- something on it went to the shell"
    );
    // And nothing is on screen yet: a menu that appeared here would be a menu that grew
    // afterwards, which is what this deliberately does not do.
    assert!(h.app.menu.is_none(), "the menu appeared before it was ready");
    assert!(h.app.menu_pending(), "and it is not on its way either");

    // It arrives whole, a moment later.
    let waited = std::time::Instant::now();
    while h.app.menu_pending() {
        h.frame(Vec::new());
        assert!(
            waited.elapsed() < std::time::Duration::from_secs(20),
            "the builder never delivered"
        );
    }
    let menu = h.app.menu.as_ref().expect("open by now");
    // Windows' menu, with exactly two entries of ours in it. This program used to put half a dozen
    // above the shell's — those are gone. Paste is here because this is the *background* menu
    // ([`App::open_folder_menu`] raises that one), the single menu the shell hands over with a gap in
    // it; `Copy path(s)` is on every menu. See `crate::shell::menu::Own::Paste` and `Own::CopyPaths`,
    // and `the_drag_answers_paste_and_copy_paths_are_the_only_entries_of_our_own` for the fence
    // around that list.
    //
    // In that order, which is where each goes: Paste at the top and `Copy path(s)` beside Properties.
    assert!(
        menu.entries.len() > 3,
        "the shell should have filled the menu: {:?}",
        menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    let ours: Vec<&str> = menu
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                crate::shell::menu::Kind::Command(crate::shell::menu::Command::Own(_))
            )
        })
        .map(|e| e.label.as_str())
        .collect();
    assert_eq!(
        ours,
        ["Paste", "Copy path(s)"],
        "the background menu should be Windows' own with this program's Paste above it and \
         Copy path(s) beside Properties, and nothing else of ours"
    );
    // And that one really is beside Properties rather than merely somewhere below Paste, because
    // that position is what keeps it out of the scrolling part — see `ui::menu::pinned_from`.
    let at = menu
        .entries
        .iter()
        .position(|e| e.label == "Copy path(s)")
        .expect("just found above");
    assert_eq!(
        crate::shell::menu::properties_at(&menu.entries),
        Some(at + 1),
        "Copy path(s) is not directly above the shell's Properties: {:?}",
        menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
    );

    // Every shell submenu is there and empty, which is the point of the lazy fill.
    let submenus: Vec<usize> = menu
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind.unasked().is_some())
        .map(|(i, _)| i)
        .collect();
    assert!(
        !submenus.is_empty(),
        "a folder's menu has 7-Zip, Send To or Give access to in it: {:?}",
        menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
    );

    // Opening each one asks for it, and what comes back has to be what the shell has in
    // it. Every one of them, and counting what arrived rather than merely that an answer
    // did: an answer of "nothing" also marks a submenu filled, so a version of this that
    // waited for the flag passed while every submenu in the program was empty.
    let mut counts: Vec<(String, usize)> = Vec::new();
    for index in submenus {
        h.app.menu.as_mut().unwrap().open = vec![index];
        let waited = std::time::Instant::now();
        loop {
            h.frame(Vec::new());
            let done = h
                .app
                .menu
                .as_ref()
                .is_some_and(|m| m.entries[index].kind.unasked().is_none());
            if done {
                break;
            }
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(20),
                "the submenu was never filled -- nothing carried the ask to the builder"
            );
        }
        let menu = h.app.menu.as_ref().unwrap();
        let count = match &menu.entries[index].kind {
            crate::shell::menu::Kind::Submenu { children, .. } => children.len(),
            _ => 0,
        };
        counts.push((menu.entries[index].label.clone(), count));
    }
    assert!(
        counts.iter().any(|(_, count)| *count > 0),
        "every submenu in the menu came back empty: {counts:?}"
    );

    h.app.close_menu();
    h.frame(Vec::new());
}

/// A menu the shell has not finished making can be given up on, and says so while it lasts.
///
/// The window used to have nothing to say about a menu that was on its way, which is fine
/// for the tenth of a second a folder takes. On an executable on a mapped share it is
/// twenty-four seconds — measured, see [`crate::shell::menu::Builder`] — and twenty-four
/// seconds of a window that shows nothing, reacts to nothing you can see, and then puts up a
/// menu at a place you have long since stopped pointing at is not a wait, it is a fault.
///
/// So there is a cursor for it, and Escape means never mind. What Escape cannot do is stop
/// `QueryContextMenu`, so what is tested at the end is the part that matters: that the answer
/// nobody wants any more does not arrive later and open a menu on its own.
#[test]
#[cfg(windows)]
fn escape_gives_up_on_a_menu_the_shell_is_still_making() {
    use std::sync::atomic::Ordering;

    let _serialised = crate::shell::serialised();
    let mut h = Harness::new();
    h.settle();

    // Stand in for the share. Long enough to press Escape inside, short enough that the
    // abandoned worker is finished before the assertion that it changed nothing.
    const STALL: u64 = 1_500;
    crate::shell::menu::STALLED.store(0, Ordering::SeqCst);
    crate::shell::menu::STALL_MS.store(STALL, Ordering::SeqCst);

    h.app.open_folder_menu(&h.ctx.clone());
    h.frame(Vec::new());
    // Cleared once the worker has picked it up, and not before: clearing it straight after
    // the frame races the thread that is about to read it.
    let waited = std::time::Instant::now();
    while crate::shell::menu::STALLED.load(Ordering::SeqCst) == 0 {
        h.frame(Vec::new());
        assert!(
            waited.elapsed() < std::time::Duration::from_secs(5),
            "the worker never started the stalled build"
        );
    }
    crate::shell::menu::STALL_MS.store(0, Ordering::SeqCst);
    assert!(h.app.menu_pending(), "the ask never went out");
    assert!(h.app.menu.is_none());

    // A second frame, and the cursor says the window is working on something.
    h.frame(Vec::new());
    assert!(h.app.menu_pending(), "the stalled build answered immediately");
    assert_eq!(
        h.cursor,
        egui::CursorIcon::Progress,
        "nothing on screen says a menu is being waited for"
    );

    h.frame(vec![Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(
        !h.app.menu_pending(),
        "Escape did not give up on the menu that was being built"
    );

    // And the answer, when it finally comes, is nobody's: no menu appears out of the blue a
    // second and a half after the click that asked for it was called off.
    let waited = std::time::Instant::now();
    while waited.elapsed() < std::time::Duration::from_millis(STALL + 800) {
        h.frame(Vec::new());
        assert!(
            h.app.menu.is_none() && !h.app.menu_pending(),
            "the abandoned menu turned up {:?} after Escape",
            waited.elapsed()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        crate::shell::menu::STALLED.load(Ordering::SeqCst),
        1,
        "the build under test never actually stalled, so nothing here was given up on"
    );
}

/// The whole chain behind `New > Text Document`: the shell makes the file, the watcher notices,
/// the folder is re-read, and the row that appeared arrives selected with its name being edited.
///
/// Real from end to end — a real verb through [`crate::shell::Modal`], a real
/// `ReadDirectoryChangesW`, a real re-read — because the timing is where this goes wrong and
/// no harness reproduces it. The two halves race by construction: the rename *starts* on a
/// re-read, and the file appearing is what asks for one. So the second half of this test is
/// simply waiting, with frames running, to see whether the field is still on the same file
/// after every notification the creation produced has been and gone.
#[test]
#[ignore = "asks the shell to create a real file; run explicitly, single-threaded"]
#[cfg(windows)]
fn new_from_the_shell_menu_ends_in_a_rename_field() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("new-menu");
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
    assert_eq!(
        h.tab(0).names(),
        ["already.txt".to_owned()],
        "the sandbox as it starts"
    );

    // The real entry, off the real background menu. `.txt` because `Document texte` is a label
    // and this has to run on an English Windows too.
    let entries = crate::shell::menu::build(&dir, &[]);
    let command = entries
        .iter()
        .filter_map(|e| match &e.kind {
            crate::shell::menu::Kind::Submenu { children, .. } => Some(children),
            _ => None,
        })
        .flatten()
        .find_map(|c| match &c.kind {
            crate::shell::menu::Kind::Command(
                command @ crate::shell::menu::Command::Shell { verb: Some(verb), .. },
            ) if verb == ".txt" => Some(command.clone()),
            _ => None,
        })
        .expect("`New > Text Document` on the folder's background menu");

    // The same decision the menu makes, then the same invoke.
    h.app.watch_for_a_new_item(pane, &dir, &command);
    assert!(
        h.tab(0).name_the_new.is_some(),
        "the listing was not written down, so there is nothing to recognise the new file by"
    );
    assert!(h.app.modal.send(crate::shell::Request::Invoke {
        parent: dir.clone(),
        items: Vec::new(),
        command,
        depth: crate::shell::menu::Depth::Full,
        owner: crate::shell::Owner::default(),
    }));

    // Frames *and* real time: the shell's write and the watcher's settle are both real, and the
    // watcher's clock is the frame's.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while h.tab(0).renaming.is_none() {
        let on_disk: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(
            std::time::Instant::now() < deadline,
            "20 s after the verb nothing is being renamed. On disk: {on_disk:?}. In the \
             listing: {:?}. Snapshot still pending: {}",
            h.tab(0).names(),
            h.tab(0).name_the_new.is_some()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.time += 0.05;
        h.frame(Vec::new());
    }

    let (entry, text) = h.tab(0).renaming.clone().expect("a rename");
    let leaf = h.tab(0).dir.as_ref().expect("a listing").leaf(entry).to_owned();
    assert_ne!(
        leaf, "already.txt",
        "the file that was here before is the one being renamed"
    );
    assert!(leaf.ends_with(".txt"), "renaming `{leaf}`");
    assert_eq!(
        text, leaf,
        "the field has to start from the name the file actually has"
    );
    assert_eq!(h.tab(0).selected_count, 1, "and be the only thing selected");

    // Now the part that matters: it has to survive every later notification the creation set
    // off. A stale entry index would still be on screen here, pointing at another file.
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.time += 0.05;
        h.frame(Vec::new());
    }
    let (entry, still) = h
        .tab(0)
        .renaming
        .clone()
        .expect("the rename field went away while nothing was touching it");
    assert_eq!(
        h.tab(0).dir.as_ref().expect("a listing").leaf(entry),
        leaf,
        "a later re-read moved the rename field onto a different file -- committing it would \
         have renamed the wrong one"
    );
    assert_eq!(still, leaf, "and the text has to be untouched");

    crate::sandbox::remove(&dir);
}

/// Which items get the short menu, and that a short one admits to being short.
///
/// The rule is narrow on purpose. What is slow is not the network: on the share this was
/// measured against, a 41 MB `.lib` builds a full menu in 1.8 s and the folder itself in
/// 0.2 s. It is an *executable* on a share, where the time is linear in the file's size
/// because something reads all of it. A blanket rule for network paths would drop 7-Zip and
/// Send To from every file on the share to fix a problem only executables have.
#[test]
#[cfg(windows)]
fn only_a_network_executable_gets_the_short_menu() {
    use crate::shell::menu::Depth;

    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let unc = PathBuf::from(r"\\somehost\someshare\bin");

    for (what, folder, items, want) in [
        (
            "a local executable",
            here.clone(),
            vec![here.join("thing.exe")],
            Depth::Full,
        ),
        (
            "an executable on a share",
            unc.clone(),
            vec![unc.join("thing.exe")],
            Depth::Fast,
        ),
        (
            "a DLL on a share",
            unc.clone(),
            vec![unc.join("thing.DLL")],
            Depth::Fast,
        ),
        (
            "a text file on a share",
            unc.clone(),
            vec![unc.join("notes.txt")],
            Depth::Full,
        ),
        (
            "a big archive on a share",
            unc.clone(),
            vec![unc.join("everything.7z")],
            Depth::Full,
        ),
        (
            "the folder itself, on a share",
            unc.clone(),
            Vec::new(),
            Depth::Full,
        ),
        (
            "a mixed selection with an executable in it, on a share",
            unc.clone(),
            vec![unc.join("notes.txt"), unc.join("thing.exe")],
            Depth::Fast,
        ),
    ] {
        assert_eq!(
            App::menu_depth(&folder, &items),
            want,
            "{what}: wrong depth"
        );
    }

    // A UNC path is a network path by construction; the extended-length and device forms
    // start the same way and are not.
    assert!(crate::shell::over_network(&unc));
    assert!(!crate::shell::over_network(&here));
    assert!(!crate::shell::over_network(Path::new(r"\\?\C:\Windows")));
    assert!(!crate::shell::over_network(Path::new(r"\\.\PhysicalDrive0")));
    assert!(!crate::shell::over_network(Path::new("")));
}

/// A short menu is still a menu: asked at [`Depth::Fast`](crate::shell::menu::Depth::Fast), the
/// shell's own verbs come back even though the extensions' do not.
#[test]
#[cfg(windows)]
fn a_short_menu_is_still_a_menu() {
    let _serialised = crate::shell::serialised();
    let mut h = Harness::new();
    h.settle();

    // A local file, so the menu is quick; the point here is the depth it was asked at, not
    // where the file is.
    let pane = h.app.panes[0].id;
    let folder = h.app.pane_mut(pane).expect("the pane").tab().path.clone();
    let items = vec![folder.join("Cargo.toml")];
    h.app.asking = Some(Asking {
        token: h
            .app
            .menu_builder
            .build(&folder, &items, crate::shell::menu::Depth::Fast),
        pane,
        at: egui::pos2(200.0, 200.0),
        items,
        folder,
        depth: crate::shell::menu::Depth::Fast,
        diffable: false,
        since: h.ctx.cumulative_pass_nr(),
        asked: std::time::Instant::now(),
    });

    let waited = std::time::Instant::now();
    while h.app.menu_pending() {
        h.frame(Vec::new());
        assert!(
            waited.elapsed() < std::time::Duration::from_secs(20),
            "the reduced menu never arrived"
        );
    }
    let menu = h.app.menu.as_ref().expect("a menu");
    assert!(
        menu.entries.len() > 3,
        "the reduced menu is too short to be one: {:?}",
        menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    h.app.close_menu();
    h.frame(Vec::new());
}

/// Anything on a share that is *still* slow gets asked for again with less.
///
/// The extension list in [`App::menu_depth`] covers what was measured. This covers what was
/// not: whatever this machine's extensions decide to read a whole file for next. Two and a
/// half seconds, because the slowest *full* menu measured on that share for something that
/// was not an executable was 1.8 s — so past this it is somebody inspecting the file rather
/// than the share being busy.
#[test]
#[cfg(windows)]
fn a_slow_network_menu_is_asked_for_again_with_less() {
    use std::sync::atomic::Ordering;
    use crate::shell::menu::Depth;

    let _serialised = crate::shell::serialised();
    let mut h = Harness::new();
    h.settle();

    // Nothing here should reach the shell: a UNC path to a host that does not exist would
    // spend the test's whole budget in DNS and SMB timeouts. The stall stands in for the
    // slow build, and is left set so the retry stalls too.
    crate::shell::menu::STALLED.store(0, Ordering::SeqCst);
    crate::shell::menu::STALL_MS.store(4_000, Ordering::SeqCst);

    let pane = h.app.panes[0].id;
    let folder = PathBuf::from(r"\\somehost\someshare\bin");
    let items = vec![folder.join("mystery.dat")];
    let first = h
        .app
        .menu_builder
        .build(&folder, &items, Depth::Full);
    h.app.asking = Some(Asking {
        token: first,
        pane,
        at: egui::pos2(200.0, 200.0),
        items,
        folder,
        depth: Depth::Full,
        diffable: false,
        // Already past the deadline, so the test does not have to sit through it.
        since: h.ctx.cumulative_pass_nr(),
        asked: std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(3))
            .expect("a monotonic clock three seconds old"),
    });

    h.frame(Vec::new());

    let asking = h.app.asking.as_ref().expect("still asking, with less");
    assert_eq!(
        asking.depth,
        Depth::Fast,
        "a full menu on a share was still being waited for three seconds in"
    );
    assert_ne!(
        asking.token, first,
        "the depth changed but the ask did not, so nothing was re-asked"
    );

    h.app.close_menu();
    crate::shell::menu::STALL_MS.store(0, Ordering::SeqCst);
    h.frame(Vec::new());
}

/// A new folder arrives selected, with its name open for editing.
///
/// `New folder` on its own is half a gesture: nobody wants a folder called `New folder`, and
/// in Explorer creating one and naming it is a single action. Which name to open is the
/// shell's answer rather than this program's guess -- ask for `New folder` when one already
/// exists and what appears is `New folder (2)` -- so it comes back through
/// `IFileOperationProgressSink`, and this is the test that it comes back at all.
#[test]
#[cfg(windows)]
fn a_new_folder_opens_its_name_for_editing() {
    let _serialised = crate::shell::serialised();
    // The whole point is that the *shell* picks the name, so the shell has to be asked.
    // Inside `target/sandbox`; see `crate::shell::ops::FOR_REAL`.
    let _for_real = crate::shell::ops::for_real();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("newfolder");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    // One already there, so the shell has to pick a different name and this cannot pass by
    // guessing "New folder".
    std::fs::create_dir_all(root.join("New folder")).expect("sandbox");

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

    h.app.perform(&h.ctx.clone(), Action::NewFolder(pane));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.frame(Vec::new());
        let renaming = h
            .app
            .pane_mut(pane)
            .expect("the pane")
            .tab()
            .renaming
            .clone();
        if let Some((_, name)) = renaming {
            assert_ne!(
                name, "New folder",
                "the shell had to pick another name, and this is editing the old folder"
            );
            assert!(
                name.starts_with("New folder"),
                "the row opened for editing is `{name}`"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the new folder never opened for editing; the program said {:?}",
            h.app.notice
        );
    }

    crate::sandbox::remove(&root);
}

/// Right-clicking a folder with nothing in it has to give the folder's own menu.
///
/// It gave nothing. `rows` is what wires a listing's clicks up, and a listing with nothing
/// in it never reaches `rows` -- an empty folder draws one line of text over a body that
/// nothing was listening to. Which is the one place `New folder` and `Paste` are most
/// wanted, so it was also the least forgiving place to do nothing.
///
/// Asserted on the *action*, not on the menu: whether the shell then has entries for that
/// folder is `shell::menu`'s business and is tested there. What was missing here was
/// anything happening at all.
#[test]
#[cfg(windows)]
fn a_right_click_in_an_empty_folder_still_raises_a_menu() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("empty");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");

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
    assert_eq!(
        h.app.pane_mut(pane).expect("the pane").tab().order.len(),
        0,
        "the folder has to be empty, or this proves nothing"
    );

    // The middle of the listing, which in an empty folder is the middle of the message.
    let body = h.app.panes[0].rect;
    let at = body.center();
    let raised = h.click_with(at, PointerButton::Secondary, Modifiers::NONE);
    assert!(
        raised.contains(&"ShellMenu"),
        "a right click in an empty folder produced {raised:?}"
    );

    crate::sandbox::remove(&root);
}

/// A right click in a row asks about the file if it lands on it, and about the folder if not.
///
/// A row is mostly space, and the space around a name is the listing's background as much as
/// the gap under the last file is — so the two halves of a row are two different questions.
/// The alternative was that the only way to reach the folder's own menu, in a folder taller
/// than the pane, was to find a gap that might not be there.
///
/// The same rule the drag already followed, and asserted the same way: by clicking at a
/// coordinate and seeing what the program did with it.
#[test]
#[cfg(windows)]
fn a_right_click_asks_about_the_file_or_the_folder_by_where_it_lands() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;

    // The second row's name, and where it was drawn — a point on the name is a point on the
    // file, whatever the padding around it happens to be.
    let name = {
        let tab = h.tab(0);
        let entry = tab.entry_at(1).expect("the test folder has a second row");
        tab.dir.as_ref().expect("a listing").name(entry).to_owned()
    };
    let rows = h
        .ctx
        .read_response(Id::new(("rows-hit", pane)))
        .map(|r| r.rect)
        .expect("the listing takes the pointer");
    let y = h.row_center(0, 1).y;
    let ink = h
        .texts()
        .into_iter()
        .find(|(_, text)| *text == name)
        .map(|(at, _)| at)
        .unwrap_or_else(|| panic!("`{name}` is not drawn in the listing"));

    // ---- On the name: that file, and the menu for it --------------------
    let raised = h.click_with(
        pos2(ink.x + 2.0, y),
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert!(raised.contains(&"ShellMenu"), "{raised:?}");
    assert_eq!(
        h.tab(0).selected_count,
        1,
        "a right click on a name has to select it first"
    );
    let asked = h
        .app
        .asking
        .as_ref()
        .expect("the menu is built off-thread and is still on its way")
        .items
        .clone();
    assert_eq!(asked.len(), 1, "the menu was asked about {asked:?}");
    assert!(
        asked[0].ends_with(&name),
        "the menu was asked about {asked:?} rather than about `{name}`"
    );

    // ---- In the space of a row that *is* selected ------------------------
    //
    // The exception, and the same one the drag makes: the files are picked out already, so
    // the menu is the selection's wherever in the row the click lands. Taking the selection
    // away because the pointer was between two columns would undo work rather than ask a
    // question.
    h.wait();
    // Two points into the row, which is left of the icon and so on nothing.
    let (beside_first, beside_second) = (
        pos2(rows.left() + 2.0, y),
        pos2(rows.left() + 2.0, h.row_center(0, 2).y),
    );
    let raised = h.click_with(beside_first, PointerButton::Secondary, Modifiers::NONE);
    assert!(raised.contains(&"ShellMenu"), "{raised:?}");
    assert_eq!(
        h.tab(0).selected_count,
        1,
        "a right click in a selected row's own space dropped the selection"
    );
    assert_eq!(
        h.app.asking.as_ref().expect("on its way").items.len(),
        1,
        "and it stopped being the selection's menu"
    );

    // ---- In the space of a row that is not -------------------------------
    h.wait();
    let at = beside_second;
    assert!(
        h.hovers(Id::new(("rows-hit", pane)), at),
        "the point beside the icon is not in the listing at all"
    );
    let raised = h.click_with(at, PointerButton::Secondary, Modifiers::NONE);
    assert!(raised.contains(&"ShellMenu"), "{raised:?}");
    assert_eq!(
        h.tab(0).selected_count,
        0,
        "a right click that was not on a file left one selected"
    );
    assert!(
        h.app
            .asking
            .as_ref()
            .expect("on its way")
            .items
            .is_empty(),
        "the menu is not the folder's"
    );
}

/// Two folders selected: the menu Windows builds for them gets `Folder diff` straight after its Open,
/// and choosing it opens a diff of the two. One folder and a file do not get it.
///
/// Against the real shell's menu, because where the entry goes is decided by what the shell put there:
/// the verb this looks for is the shell's, and a test menu made up for the purpose would find it by
/// construction. Nothing on the menu is invoked.
#[test]
#[cfg(windows)]
fn two_folders_selected_offer_a_folder_diff_after_open() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let dir = crate::sandbox::fresh("menu-folder-diff");
    for folder in ["one", "two"] {
        std::fs::create_dir_all(dir.join(folder)).expect("a folder");
    }
    std::fs::write(dir.join("file.txt"), b"x").expect("a file");

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

    // Select these rows, raise the selection's menu, and wait for the shell to fill it.
    fn menu_over(h: &mut Harness, names: &[&str]) -> crate::ui::menu::Open {
        let tab = h.app.panes[0].tab_mut();
        let positions: Vec<usize> = names
            .iter()
            .map(|name| {
                let entry = tab.names().iter().position(|n| n == name).expect("listed");
                tab.order.iter().position(|&i| i as usize == entry).expect("on show")
            })
            .collect();
        tab.select_only(positions[0]);
        for &at in &positions[1..] {
            tab.toggle(at);
        }
        h.app.open_folder_menu(&h.ctx.clone());
        h.frame(Vec::new());
        let waited = std::time::Instant::now();
        while h.app.menu_pending() {
            h.frame(Vec::new());
            assert!(waited.elapsed() < std::time::Duration::from_secs(20), "no menu");
        }
        let menu = h.app.menu.take().expect("open by now");
        h.app.menu_builder.close(menu.token);
        menu
    }
    let is_diff = |e: &crate::shell::menu::Entry| {
        matches!(
            e.kind,
            crate::shell::menu::Kind::Command(crate::shell::menu::Command::Own(
                crate::shell::menu::Own::FolderDiff
            ))
        )
    };

    let menu = menu_over(&mut h, &["one", "two"]);
    let labels: Vec<&str> = menu.entries.iter().map(|e| e.label.as_str()).collect();
    let open = menu
        .entries
        .iter()
        .position(|e| {
            matches!(&e.kind, crate::shell::menu::Kind::Command(
                crate::shell::menu::Command::Shell { verb: Some(verb), .. },
            ) if verb.eq_ignore_ascii_case("open"))
        })
        .unwrap_or_else(|| panic!("the shell's Open on two folders: {labels:?}"));
    assert!(
        menu.entries.get(open + 1).is_some_and(is_diff),
        "`Folder diff` is not straight after Open: {labels:?}"
    );
    assert_eq!(menu.entries.iter().filter(|e| is_diff(e)).count(), 1, "{labels:?}");

    // Choosing it is a diff of the two, in a new tab of this pane.
    let action = h
        .app
        .own_menu_action(&menu, crate::shell::menu::Own::FolderDiff)
        .expect("an action");
    h.app.perform(&h.ctx.clone(), action);
    h.settle();
    let tab = h.tab(0);
    let twin = tab.diff.as_ref().and_then(|d| d.twin).expect("a diff tab in front");
    assert_eq!(tab.path, dir.join("one"));
    let right = h.app.panes.iter().find(|p| p.id == twin).expect("its right half");
    assert_eq!(right.tab().path, dir.join("two"));

    // Back to the folder, and a folder with a file is not two folders.
    let diff = h.app.panes[0].active;
    h.app.perform(&h.ctx.clone(), Action::CloseTab { pane, tab: diff });
    h.settle();
    let menu = menu_over(&mut h, &["one", "file.txt"]);
    assert!(
        !menu.entries.iter().any(is_diff),
        "offered on a folder and a file: {:?}",
        menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    crate::sandbox::remove(&dir);
}
