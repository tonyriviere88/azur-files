use super::*;

#[test]
fn a_job_knows_which_folders_it_changes() {
    let job = Job::Move {
        items: vec![
            PathBuf::from(r"C:\a\one.txt"),
            PathBuf::from(r"C:\a\two.txt"),
            PathBuf::from(r"C:\b\three.txt"),
        ],
        into: PathBuf::from(r"C:\dest"),
    };
    let touched = job.touches();
    assert!(touched.contains(&PathBuf::from(r"C:\a")));
    assert!(touched.contains(&PathBuf::from(r"C:\b")));
    assert!(touched.contains(&PathBuf::from(r"C:\dest")));
    assert_eq!(touched.len(), 3, "and each folder once: {touched:?}");
}

#[test]
fn a_delete_only_touches_the_sources() {
    let job = Job::Delete {
        items: vec![PathBuf::from(r"C:\a\one.txt")],
        to_bin: true,
    };
    assert_eq!(job.touches(), [PathBuf::from(r"C:\a")]);
}

/// A job started the way the window starts them does not reach the shell from a test.
///
/// The guard this checks is the one described on [`FOR_REAL`], and the reason it is worth a
/// test of its own is how the bug behaved: the job goes to a thread, so the test that asked
/// for it finished and *passed* while the shell's confirmation dialog was on screen asking
/// whether to permanently delete a folder out of this repository. There was nothing in any
/// test output to notice, which is exactly the class of failure a guard is for.
///
/// The job named here is the one that was actually issued: `Shift+Delete` on the first row of
/// `CARGO_MANIFEST_DIR`.
#[test]
fn a_test_cannot_hand_a_job_to_the_shell_by_accident() {
    use std::sync::atomic::Ordering;

    assert!(
        !FOR_REAL.load(Ordering::SeqCst),
        "the real shell must be off unless a test has asked for it"
    );

    let ctx = egui::Context::default();
    let mut ops = Operations::new();
    ops.start(
        Job::Delete {
            items: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".cargo")],
            to_bin: false,
        },
        Owner::default(),
        &ctx,
    );

    // It reports back as a job that did nothing, rather than being dropped silently: the
    // window takes the folders in `touched` as its cue to re-read, and a job that never
    // answers leaves `Copying 1 item…` in the status line for the rest of the session.
    let finished = ops.drain();
    assert_eq!(finished.len(), 1, "the job never reported back");
    assert!(finished[0].error.is_none(), "{:?}", finished[0].error);
    assert!(
        ops.in_progress().is_none(),
        "the status line still says something is running"
    );
    assert!(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".cargo").exists(),
        "the guard let a permanent delete through"
    );

    // ---- Both directions, on something expendable ----------------------
    //
    // The delete above cannot be un-guarded to prove the guard does anything, for the
    // obvious reason. A new folder can: it is the one job that completes without the shell
    // asking anything, so the same call can be watched through the gate shut and open.
    #[cfg(windows)]
    {
        let root = crate::sandbox::dir("guard");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("temp dir");

        let make = |ops: &mut Operations| {
            ops.start(
                Job::NewFolder {
                    parent: root.clone(),
                    name: "made".to_owned(),
                },
                Owner::default(),
                &ctx,
            );
            // The work is on a thread, so the answer has to be waited for rather than
            // assumed either way.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while ops.in_progress().is_some() && std::time::Instant::now() < deadline {
                ops.drain();
                std::thread::yield_now();
            }
            std::fs::read_dir(&root).into_iter().flatten().count()
        };

        assert_eq!(make(&mut ops), 0, "a guarded job reached the shell anyway");
        {
            let _for_real = for_real();
            assert_eq!(
                make(&mut ops),
                1,
                "the opt-in did not reach the shell — every test that needs it is \
                 testing nothing"
            );
        }
        crate::sandbox::remove(&root);
    }

    assert!(
        !FOR_REAL.load(Ordering::SeqCst),
        "the guard has to close again when it goes out of scope"
    );
}

/// The engine itself, end to end, on real files.
///
/// Only the operations that cannot raise a dialog: a copy into an empty folder, a
/// rename to a free name and a new folder all complete without asking anything, so
/// the test finishes on its own. Delete is deliberately not exercised here — a
/// permanent delete prompts, and a recycle would leave litter in the user's own
/// Recycle Bin, which a test has no business doing.
#[test]
#[cfg(windows)]
fn the_shell_engine_copies_renames_and_creates() {
    use std::time::{Duration, Instant};

    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = crate::sandbox::dir("ops");
    let from = root.join("from");
    let into = root.join("into");
    std::fs::create_dir_all(&from).expect("temp dir");
    std::fs::create_dir_all(&into).expect("temp dir");
    let one = from.join("one.txt");
    std::fs::write(&one, b"one").expect("write");

    /// Run a job to completion, on a thread of its own.
    ///
    /// On a thread of its own because that is where production runs it, and because
    /// `run` initialises an apartment and *uninitialises* it on the way out — doing
    /// that on the test thread would tear down the apartment every other shell test
    /// on this thread is relying on, and the failure would surface somewhere else
    /// entirely.
    fn run_now(job: Job) -> Option<String> {
        std::thread::spawn(move || super::run(&job, Owner::default()).0)
            .join()
            .expect("the operation thread panicked")
    }

    // ---- Copy ----
    assert_eq!(
        run_now(Job::Copy {
            items: vec![one.clone()],
            into: into.clone(),
        }),
        None
    );
    assert!(
        into.join("one.txt").exists(),
        "the shell should have copied the file"
    );
    assert!(one.exists(), "and left the original alone");

    // ---- Rename ----
    assert_eq!(
        run_now(Job::Rename {
            item: into.join("one.txt"),
            name: "two.txt".to_owned(),
        }),
        None
    );
    assert!(into.join("two.txt").exists(), "renamed");
    assert!(!into.join("one.txt").exists(), "and the old name is gone");

    // ---- New folder ----
    assert_eq!(
        run_now(Job::NewFolder {
            parent: into.clone(),
            name: "made".to_owned(),
        }),
        None
    );
    // `NewItem` returns before the directory entry is necessarily visible, so this
    // waits rather than asserting on a race.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !into.join("made").is_dir() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(into.join("made").is_dir(), "the folder should have been made");

    crate::sandbox::remove(&root);
}

/// What the shell actually puts on screen, and for which operation.
///
/// A probe rather than an assertion: the answer is other people's UI. Every window this
/// process owns is listed before and after the operation starts, and whatever is new is
/// what the shell raised -- class, title, and whether it is owned by this program's
/// window. Each one is then closed so the run finishes on its own.
///
/// Uses `target/sandbox`, which is expendable.
#[test]
#[ignore = "puts real shell dialogs on screen; run explicitly with --nocapture"]
#[cfg(windows)]
fn what_the_shell_puts_on_screen() {
    use std::time::{Duration, Instant};

    let _serialised = crate::shell::serialised();
    crate::shell::init();

    // Joined a component at a time: a `join("target/sandbox")` keeps the forward slashes,
    // and `SHCreateItemFromParsingName` refuses a path that has any in it.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("dialogs");
    crate::sandbox::remove(&root);
    let from = root.join("from");
    let into = root.join("into");
    std::fs::create_dir_all(&from).expect("sandbox");
    std::fs::create_dir_all(&into).expect("sandbox");
    std::fs::write(from.join("one.txt"), b"from").expect("write");
    std::fs::write(into.join("one.txt"), b"a different one").expect("write");
    std::fs::write(from.join("gone.txt"), b"to delete").expect("write");
    std::fs::write(from.join("binned.txt"), b"to recycle").expect("write");

    for (what, job) in [
        (
            "copy onto an existing name",
            Job::Copy {
                items: vec![from.join("one.txt")],
                into: into.clone(),
            },
        ),
        (
            "permanent delete",
            Job::Delete {
                items: vec![from.join("gone.txt")],
                to_bin: false,
            },
        ),
        (
            "recycle",
            Job::Delete {
                items: vec![from.join("binned.txt")],
                to_bin: true,
            },
        ),
    ] {
        let before = windows_of_this_process();
        let handle = std::thread::spawn(move || super::run(&job, Owner::default()).0);

        // Watch for anything new for a couple of seconds, then shut it.
        let mut seen: Vec<(String, String)> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            for (hwnd, class, title) in windows_of_this_process() {
                if before.iter().any(|(h, _, _)| *h == hwnd) {
                    continue;
                }
                if seen.iter().any(|(c, t)| *c == class && *t == title) {
                    continue;
                }
                seen.push((class.clone(), title.clone()));
                println!("  {what}: `{title}` [{class}]");
                close(hwnd);
            }
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let outcome = handle.join().expect("the operation thread panicked");
        if seen.is_empty() {
            println!("  {what}: nothing on screen");
        }
        println!("  {what}: finished as {outcome:?}");
    }

    crate::sandbox::remove(&root);
}

/// Run a job on its own thread, closing any window the shell raises, and report both
/// what it finished as and what it put on screen.
#[cfg(all(test, windows))]
fn run_watching(job: Job) -> (Option<String>, Vec<(String, String)>) {
    use std::time::{Duration, Instant};

    let before = windows_of_this_process();
    let handle = std::thread::spawn(move || super::run(&job, Owner::default()).0);
    let mut seen: Vec<(String, String)> = Vec::new();
    // Watched for a while before anything is closed. A shell operation raises a progress
    // window of its own accord and finishes behind it; closing that on sight cancels work
    // that was never waiting for an answer, which is how this probe first reported a
    // same-folder copy as "interrupted".
    let patience = Instant::now() + Duration::from_millis(1500);
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let mut fresh = Vec::new();
        for (hwnd, class, title) in windows_of_this_process() {
            if before.iter().any(|(h, _, _)| *h == hwnd) {
                continue;
            }
            if !seen.iter().any(|(c, t)| *c == class && *t == title) {
                seen.push((class, title));
            }
            fresh.push(hwnd);
        }
        if handle.is_finished() {
            break;
        }
        if Instant::now() > patience {
            // Still going, so something is waiting to be answered.
            for hwnd in fresh {
                close(hwnd);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    (
        handle.join().expect("the operation thread panicked"),
        seen,
    )
}

/// Every visible top-level window this process owns, with its class and title.
#[cfg(all(test, windows))]
fn windows_of_this_process() -> Vec<(isize, String, String)> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = &mut *(lparam.0 as *mut Vec<(isize, String, String)>);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != GetCurrentProcessId() || !IsWindowVisible(hwnd).as_bool() {
            return TRUE;
        }
        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut class);
        let mut title = [0u16; 512];
        let m = GetWindowTextW(hwnd, &mut title);
        found.push((
            hwnd.0 as isize,
            String::from_utf16_lossy(&class[..n.max(0) as usize]),
            String::from_utf16_lossy(&title[..m.max(0) as usize]),
        ));
        TRUE
    }

    let mut found: Vec<(isize, String, String)> = Vec::new();
    // SAFETY: the callback only writes through the pointer it is handed, which outlives
    // the enumeration.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

/// Ask a window to go away, which for a shell dialog is a cancel.
#[cfg(all(test, windows))]
fn close(hwnd: isize) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
    // SAFETY: posting is asynchronous and safe against a window that has already gone.
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut std::ffi::c_void)),
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// A path with forward slashes in it has to work, because one can get this far.
///
/// `--open=C:/Windows` is a perfectly ordinary thing to type, and `std::fs` is perfectly
/// happy with it -- the listing appears, the icons are asked for, the menu is asked for.
/// The shell *parses* paths rather than passing them to the kernel, and refuses a slash,
/// so every one of those quietly did nothing. Normalising in `shell::wide` fixes all of
/// them at once; this is the check that it stays fixed.
#[test]
#[cfg(windows)]
fn the_shell_takes_a_path_with_forward_slashes() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = sandbox("slashes");
    let one = root.join("one.txt");
    std::fs::write(&one, b"one").expect("write");

    let slashed = PathBuf::from(one.to_string_lossy().replace('\\', "/"));
    assert!(
        slashed.exists(),
        "the slashed path has to be a real path to std::fs, or this proves nothing"
    );
    assert!(
        slashed.to_string_lossy().contains('/'),
        "and it has to actually have a slash in it: {}",
        slashed.display()
    );

    // SAFETY: a pure lookup; nothing is retained.
    unsafe {
        assert!(
            item(&slashed).is_ok(),
            "the shell refused {} -- `wide` is not normalising separators",
            slashed.display()
        );
    }

    crate::sandbox::remove(&root);
}

#[test]
fn what_counts_as_a_copy_into_its_own_folder() {
    let here = PathBuf::from(r"C:\Temp");
    assert!(all_already_in(&[here.join("a.txt")], &here));
    // Windows paths are case-insensitive and so is this.
    assert!(all_already_in(&[PathBuf::from(r"C:\TEMP\a.txt")], &here));
    // A slashed destination is the same destination.
    assert!(all_already_in(&[here.join("a.txt")], Path::new("C:/Temp")));
    // One item from somewhere else makes it a real name clash, which the user answers.
    assert!(!all_already_in(
        &[here.join("a.txt"), PathBuf::from(r"C:\Other\a.txt")],
        &here
    ));
    assert!(!all_already_in(&[], &here), "and nothing is not a copy");
}

/// Ctrl+C then Ctrl+V in the same folder, which is the one collision Explorer does not
/// ask about.
///
/// The name is the shell's and it is localised -- `one - Copy.txt` in English, measured as
/// `one - Copie.txt` here -- so what is asserted is that a second file appeared and that
/// nothing was put on screen to get it.
#[test]
#[cfg(windows)]
fn a_copy_into_its_own_folder_renames_rather_than_asking() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = sandbox("same-folder");
    let one = root.join("one.txt");
    std::fs::write(&one, b"one").expect("write");

    let (outcome, on_screen) = run_watching(Job::Copy {
        items: vec![one.clone()],
        into: root.clone(),
    });
    assert_eq!(outcome, None, "the copy should have gone through");
    assert!(
        on_screen.is_empty(),
        "nothing should have been asked, and this came up: {on_screen:?}"
    );

    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("read the folder back")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "expected two files, found {names:?}");
    assert!(names.contains(&"one.txt".to_owned()), "{names:?}");
    let copy = names.iter().find(|n| *n != "one.txt").expect("the copy");
    assert!(
        copy.starts_with("one ") && copy.ends_with(".txt"),
        "the shell named the copy `{copy}`, which does not look like Explorer's"
    );

    crate::sandbox::remove(&root);
}

/// A fresh, empty folder under `target/sandbox`, which is expendable.
#[cfg(all(test, windows))]
fn sandbox(name: &str) -> PathBuf {
    // Joined a component at a time: `join("target/sandbox")` would keep the forward
    // slashes, and half of what is tested here is about exactly that.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join(name);
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    root
}

#[test]
fn descriptions_count_and_name_the_operation() {
    assert_eq!(
        Job::Delete {
            items: vec![PathBuf::from("x")],
            to_bin: true
        }
        .describe(),
        "Recycling 1 item…"
    );
    assert_eq!(
        Job::Delete {
            items: vec![PathBuf::from("x"), PathBuf::from("y")],
            to_bin: false
        }
        .describe(),
        "Deleting 2 items…"
    );
    assert_eq!(
        Job::Copy {
            items: vec![PathBuf::from("x"), PathBuf::from("y")],
            into: PathBuf::from("z")
        }
        .describe(),
        "Copying 2 items…"
    );
}
