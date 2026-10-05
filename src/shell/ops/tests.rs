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
/// permanent delete prompts, and a recycle puts something in the user's own Recycle
/// Bin, which a test should only do if it takes it back out again.
///
/// Which is what [`a_recycled_file_comes_back_from_the_bin`] does, because the whole point of it
/// is the taking back out. It says exactly what it leaves in the bin and when.
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

/// **What the sink is told, and how much of it can be relied on.**
///
/// The whole of undo rests on this: [`Outcome`] is filled from `IFileOperationProgressSink`, and
/// two things about that were assumptions until this test measured them on a real operation.
///
/// 1. **Does one sink registered with `Advise` hear about every item**, or does `NewItem` only
///    report to the sink passed in its own last argument? The old code passed a per-item sink to
///    `NewItem` and nothing else, so the question had never come up.
/// 2. **Is `psiNewlyCreated` actually there?** It is documented as optional on all five `Post…`
///    callbacks, and it is the only exact answer to "what did this produce".
///
/// Measured here, in `target/sandbox`, on operations that raise no dialog:
///
/// | operation | reported by the `Advise` sink | `psiNewlyCreated` | `psznewname` |
/// | --- | --- | --- | --- |
/// | copy into an empty folder | `created`, one path | `…\into\one.txt` | `one.txt` |
/// | copy into the folder it is already in | `created`, one path | `…\from\one - Copie.txt` | `one - Copie.txt` |
/// | move between folders | `moved`, both ends | `…\from\two.txt` | `two.txt` |
/// | rename | `moved`, both ends | present | — |
/// | new folder, twice | `created`, one path each | `…\into\made`, then `…\into\made (2)` | `made`, `made (2)` |
///
/// So `Advise` alone is enough — the per-item sink `NewItem` used to be passed is gone — and
/// `psiNewlyCreated` was present every time.
///
/// The last column is the surprise, and it is the reason the fallback in `win::landed` is not as
/// alarming as it looks: `psznewname` turns out to be the name the shell *settled on*, not the
/// one it was asked for. `one - Copie.txt` and `made (2)` are both in it. That is not in any
/// contract, though, so the item is still what is used.
///
/// Assertions and not a print-out, because every row is load-bearing. If `psiNewlyCreated` stops
/// arriving, undo starts working off composed paths — and this is where that is noticed rather
/// than in somebody's folder.
#[test]
#[cfg(windows)]
fn the_sink_reports_what_the_shell_actually_did() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = sandbox("sink");
    let from = root.join("from");
    let into = root.join("into");
    std::fs::create_dir_all(&from).expect("sandbox");
    std::fs::create_dir_all(&into).expect("sandbox");
    let one = from.join("one.txt");
    std::fs::write(&one, b"one").expect("write");

    // ---- A copy says where the copy landed ----
    let outcome = outcome_of(Job::Copy {
        items: vec![one.clone()],
        into: into.clone(),
    });
    assert_eq!(
        outcome.created,
        [into.join("one.txt")],
        "the sink did not report the copy it made"
    );
    assert!(outcome.moved.is_empty() && outcome.recycled.is_empty());

    // ---- And into the folder it is already in, where the name is the shell's ----
    //
    // The row of the table that a guess gets wrong, and the reason the sink exists at all: what
    // arrives is `one - Copy.txt` under whatever name this Windows is in, and nothing on this
    // side could have worked it out.
    let outcome = outcome_of(Job::Copy {
        items: vec![one.clone()],
        into: from.clone(),
    });
    let [made] = &outcome.created[..] else {
        panic!("expected exactly one copy, got {:?}", outcome.created)
    };
    assert_ne!(*made, one, "the copy was reported as the original");
    assert_eq!(made.parent(), Some(from.as_path()));
    assert!(
        made.exists(),
        "the sink reported {}, which is not there — undo would recycle nothing",
        made.display()
    );
    crate::sandbox::remove_file(made);

    // ---- A rename says both ends ----
    let outcome = outcome_of(Job::Rename {
        item: into.join("one.txt"),
        name: "two.txt".to_owned(),
    });
    assert_eq!(
        outcome.moved,
        [(into.join("one.txt"), into.join("two.txt"))],
        "a rename has to report the name it came from as well as the one it went to, or there \
         is nothing to put back"
    );

    // ---- A move says both ends, across folders ----
    let outcome = outcome_of(Job::Move {
        items: vec![into.join("two.txt")],
        into: from.clone(),
    });
    assert_eq!(outcome.moved, [(into.join("two.txt"), from.join("two.txt"))]);

    // ---- A new folder says the name the shell settled on ----
    //
    // Twice, because the second is where the name stops being the one that was asked for — which
    // is the property `After::NameIt` has depended on since before there was a history.
    for expected in ["made", "made (2)"] {
        let outcome = outcome_of(Job::NewFolder {
            parent: into.clone(),
            name: "made".to_owned(),
        });
        assert_eq!(
            outcome.created,
            [into.join(expected)],
            "`Advise` alone did not hear about the new folder"
        );
        assert_eq!(outcome.created_name().as_deref(), Some(expected));
        // `NewItem` returns before the directory entry is necessarily visible, and the second
        // pass round this loop depends on the first folder being there to collide with.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !into.join(expected).is_dir() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(into.join(expected).is_dir(), "the folder was never made");
    }

    crate::sandbox::remove(&root);
}

/// **A callback with neither an item nor a name must not take the process down.**
///
/// The table above says `psiNewlyCreated` was present every time, and it was — for the operations
/// that table could run, which are the ones that raise no dialog. A **replace** raises one, so it
/// is not up there, and it is the case where the shell gives `win::landed` neither an item with a
/// path nor a name: nothing is newly created by writing over a file that already exists.
///
/// It crashed. Replacing a file on `\\fileserver\web` faulted at `ucrtbase!wcslen`, called from
/// `PostCopyItem` through `landed` and out of `PCWSTR::to_string`, which is `wcslen` on whatever
/// pointer it is given and checks nothing:
///
/// ```text
/// ucrtbase!wcslen+0x5f                                    rdx=0
/// azur_files+0x2fc534
/// azur_files+0xd7b21
/// windows_storage!CFileOperation::_NotifyPostCopyItemCallback+0x40
/// windows_storage!CFileOperation::NotifyPostCopyItem+0x90
/// ```
///
/// The two offsets into this crate are from the build that faulted and mean nothing in any
/// other; what the trace is here for is the shape — the shell calling in, and `wcslen` at the
/// bottom of it.
///
/// So this is the shape of that callback, made directly rather than by asking the shell for a
/// conflict nobody can answer without a mouse. A null `Ref` is what the shell passes for an absent
/// item, and `Ref::default()` is that same null. The destination folder is real, because it was
/// real in the crash — that is what gets past the `into` line and as far as the name.
#[test]
#[cfg(windows)]
fn a_replace_reports_no_item_and_no_name_without_crashing() {
    use windows::core::PCWSTR;
    use windows_core::Ref;

    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = sandbox("landed");
    std::fs::create_dir_all(&root).expect("sandbox");

    // SAFETY: a pure lookup; the item is dropped at the end of the test.
    let folder = unsafe { item(&root) }.expect("the sandbox folder as an IShellItem");

    assert_eq!(
        landed(Ref::default(), Ref::from(&folder), &PCWSTR::null()),
        None,
        "a callback with no item and no name has no path to report"
    );

    // And the name is still used when there is one, or the guard would have bought safety by
    // throwing away the answer undo works from.
    let named: Vec<u16> = "one.txt\0".encode_utf16().collect();
    assert_eq!(
        landed(
            Ref::default(),
            Ref::from(&folder),
            &PCWSTR(named.as_ptr()),
        ),
        Some(root.join("one.txt")),
        "the folder-and-name fallback still composes a path"
    );

    crate::sandbox::remove(&root);
}

/// **Undo of a move and of a rename, through the real shell.**
///
/// [`Job::PutBack`] is what Ctrl+Z issues for both, and the pair inside one folder takes the
/// `RenameItem` path rather than the move engine — see `win::perform`. Both are exercised here,
/// from separate folders in one job, which is the shape a paste of a multi-folder cut leaves
/// behind and the reason `PutBack` carries a destination per item.
#[test]
#[cfg(windows)]
fn a_move_and_a_rename_can_be_put_back() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = sandbox("put-back");
    let a = root.join("a");
    let b = root.join("b");
    let dest = root.join("dest");
    for dir in [&a, &b, &dest] {
        std::fs::create_dir_all(dir).expect("sandbox");
    }
    std::fs::write(a.join("one.txt"), b"one").expect("write");
    std::fs::write(b.join("two.txt"), b"two").expect("write");
    // The rename half: it stays in `dest` and only its name changes.
    std::fs::write(dest.join("three.txt"), b"three").expect("write");

    let outcome = outcome_of(Job::Move {
        items: vec![a.join("one.txt"), b.join("two.txt")],
        into: dest.clone(),
    });
    assert_eq!(outcome.moved.len(), 2, "{:?}", outcome.moved);
    assert_eq!(
        outcome_of(Job::Rename {
            item: dest.join("three.txt"),
            name: "renamed.txt".to_owned(),
        })
        .moved
        .len(),
        1
    );

    // Everything back at once, which is two folders and a rename in one job.
    let back = Job::PutBack {
        items: vec![
            (dest.join("one.txt"), a.join("one.txt")),
            (dest.join("two.txt"), b.join("two.txt")),
            (dest.join("renamed.txt"), dest.join("three.txt")),
        ],
    };
    assert_eq!(run_now(back), None, "the put-back was refused");

    assert!(a.join("one.txt").exists(), "one.txt did not go back to a/");
    assert!(b.join("two.txt").exists(), "two.txt did not go back to b/");
    assert!(
        dest.join("three.txt").exists(),
        "the rename was not taken back"
    );
    assert!(
        !dest.join("one.txt").exists() && !dest.join("renamed.txt").exists(),
        "the put-back copied instead of moving: {:?}",
        std::fs::read_dir(&dest)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name())
            .collect::<Vec<_>>()
    );

    crate::sandbox::remove(&root);
}

/// **Delete to the Recycle Bin, then Ctrl+Z: the whole round trip, through the real bin.**
///
/// The one test here that touches something outside `target/sandbox`, and it is worth being exact
/// about what: it recycles two files **from** the sandbox, which puts them in the user's own
/// Recycle Bin for a moment, and then restores them — which is the operation under test, and which
/// is what takes them back out again. Nothing else in the bin is read, matched or touched.
///
/// It is written to leave nothing behind on the way through. If the restore fails the test fails
/// loudly *and* says what to look for, because the residue is then real: two files named
/// `recycled.txt` and `searched.txt`, originally under `target\sandbox\recycle`, sitting in the bin
/// where the user can put them back or empty them.
///
/// Both routes into [`crate::shell::ops::bin`] are covered, because they fail differently:
///
/// | | |
/// | --- | --- |
/// | `recycled.txt` | restored from the ID list `PostDeleteItem` handed over |
/// | `searched.txt` | its ID list thrown away first, so the bin has to be searched by original path |
#[test]
#[cfg(windows)]
fn a_recycled_file_comes_back_from_the_bin() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = sandbox("recycle");
    let exact = root.join("recycled.txt");
    let searched = root.join("searched.txt");
    std::fs::write(&exact, b"from the id list").expect("write");
    std::fs::write(&searched, b"from a search").expect("write");

    let outcome = outcome_of(Job::Delete {
        items: vec![exact.clone(), searched.clone()],
        to_bin: true,
    });
    assert!(
        !exact.exists() && !searched.exists(),
        "the files were not recycled, so there is nothing to restore"
    );
    assert_eq!(
        outcome.recycled.len(),
        2,
        "the sink did not report what went to the bin: {outcome:?}"
    );
    for item in &outcome.recycled {
        assert!(
            item.from == exact || item.from == searched,
            "the bin item is not one of the two deleted: {item:?}"
        );
    }

    // **While they are in there, the bin as this program lists it.** Each is a row named by the
    // path it came from and leading to the `$R…` file the shell said it is held as — the two things
    // [`crate::fs::recycle`] reads out of the `$I…` beside it — and the menu for that row is the
    // bin's own, with Restore and a permanent Delete on it. Read, never invoked: the restore below
    // is what takes them out again.
    let bin = crate::fs::recycle::listing(std::time::Instant::now());
    assert!(bin.recycled);
    for item in &outcome.recycled {
        let held = item.bin.as_ref().expect("the sink named the file in the bin");
        // Compared the way Windows compares paths: the volume's folder is `$RECYCLE.BIN` on NTFS
        // and whatever the formatter wrote elsewhere.
        let fold = |path: &Path| path.to_string_lossy().to_lowercase();
        let row = (0..bin.len())
            .find(|&i| fold(&bin.target(i)) == fold(held))
            .unwrap_or_else(|| {
                panic!(
                    "{} is not in the listing of the bin, which holds {:?}",
                    held.display(),
                    (0..bin.len()).map(|i| bin.target(i)).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            Path::new(bin.name(row)),
            item.from.as_path(),
            "the row is not named by where it came from"
        );
        assert!(crate::fs::recycle::is_held(held));
    }
    let held = outcome.recycled[0].bin.clone().expect("named above");
    let verbs = crate::shell::menu::tests::verbs_of(
        &crate::fs::recycle::location(),
        std::slice::from_ref(&held),
    );
    for wanted in ["undelete", "delete"] {
        assert!(
            verbs.iter().any(|(verb, _)| verb.eq_ignore_ascii_case(wanted)),
            "the menu for an item in the bin has no {wanted}: {verbs:?}"
        );
    }

    // The second route: the same items, with what the shell said about the bin thrown away, so
    // `bin::find` has to identify them by where they came from. Restored in one job with the
    // first, which is also what proves a mixed batch works.
    let items: Vec<crate::shell::ops::Recycled> = outcome
        .recycled
        .iter()
        .map(|item| crate::shell::ops::Recycled {
            from: item.from.clone(),
            bin: if item.from == searched {
                None
            } else {
                item.bin.clone()
            },
        })
        .collect();

    let refused = run_now(Job::Restore { items });
    assert_eq!(
        refused, None,
        "the restore was refused — `recycled.txt` and `searched.txt` are now in the Recycle \
         Bin, under {}",
        root.display()
    );

    // `undelete` is the shell's own Restore and it returns before the file is necessarily back,
    // exactly as `NewItem` does — so this waits rather than asserting on a race.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !(exact.exists() && searched.exists()) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        exact.exists(),
        "the file the shell named was not put back — `recycled.txt` is still in the Recycle Bin, \
         originally from {}",
        root.display()
    );
    assert!(
        searched.exists(),
        "the file found by searching the bin was not put back — `searched.txt` is still in the \
         Recycle Bin, originally from {}",
        root.display()
    );
    assert_eq!(
        std::fs::read(&exact).expect("read it back"),
        b"from the id list",
        "it came back as a different file"
    );
    assert_eq!(
        std::fs::read(&searched).expect("read it back"),
        b"from a search"
    );

    crate::sandbox::remove(&root);
}

/// **The whole loop: a move, Ctrl+Z, Ctrl+Y — through `Operations`, on real files.**
///
/// Everything above this tests one link. This tests the chain, and it goes through the same
/// [`Operations::start_then`] the window uses rather than calling [`super::run`] directly, so the
/// `FOR_REAL` gate and the sandbox guard are both in the path — as is the thread, the channel and
/// [`Done`] arriving a frame later, which is where a history keyed on anything but what travelled
/// with the job would come apart.
///
/// A **move** and not a copy, deliberately: undoing a copy recycles it, and a test that leaves
/// something in the user's Recycle Bin has to be the test whose subject *is* the Recycle Bin. This
/// one covers the loop; [`a_recycled_file_comes_back_from_the_bin`] covers that.
#[test]
#[cfg(windows)]
fn a_move_can_be_undone_and_redone_through_the_history() {
    use crate::shell::ops::history::History;

    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let _for_real = for_real();

    let ctx = egui::Context::default();
    let root = sandbox("history");
    let from = root.join("from");
    let dest = root.join("dest");
    std::fs::create_dir_all(&from).expect("sandbox");
    std::fs::create_dir_all(&dest).expect("sandbox");
    let one = from.join("one.txt");
    std::fs::write(&one, b"one").expect("write");

    let mut ops = Operations::new();
    let mut history = History::default();

    /// Wait for whatever is running, then hand every answer to the history exactly as
    /// `App::collect_operations` does.
    ///
    /// Cloned because the history takes ownership and this test also wants to look at what
    /// arrived, which the window has no reason to do.
    fn settle(ops: &mut Operations, history: &mut History) -> Vec<Done> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut arrived = Vec::new();
        while std::time::Instant::now() < deadline {
            for done in ops.drain() {
                history.record(done.clone());
                arrived.push(done);
            }
            if ops.in_progress().is_none() {
                break;
            }
            std::thread::yield_now();
        }
        arrived
    }

    // ---- The move ----
    ops.start(
        Job::Move {
            items: vec![one.clone()],
            into: dest.clone(),
        },
        Owner::default(),
        &ctx,
    );
    let arrived = settle(&mut ops, &mut history);
    assert_eq!(arrived.len(), 1, "the move never reported back");
    assert_eq!(arrived[0].error, None);
    assert!(dest.join("one.txt").exists(), "the move did not happen");
    assert!(
        history.can_undo(),
        "a move that worked was not put on the undo stack: {:?}",
        arrived[0].outcome
    );
    assert!(!history.can_redo());

    // ---- Ctrl+Z ----
    let back = history.undo().expect("an undo job");
    ops.start_then(back, After::Settle, Owner::default(), &ctx);
    let arrived = settle(&mut ops, &mut history);
    assert_eq!(arrived.len(), 1, "the undo never reported back");
    assert_eq!(arrived[0].error, None, "the undo was refused");
    assert!(one.exists(), "Ctrl+Z did not put the file back in from/");
    assert!(
        !dest.join("one.txt").exists(),
        "Ctrl+Z left a copy behind in dest/"
    );
    // The entry has crossed over, and only because the reversal reported that it worked.
    assert!(history.can_redo(), "the undone move is not redoable");
    assert!(!history.can_undo(), "the undo was itself put on the stack");

    // ---- Ctrl+Y ----
    let again = history.redo().expect("a redo job");
    ops.start_then(again, After::Settle, Owner::default(), &ctx);
    let arrived = settle(&mut ops, &mut history);
    assert_eq!(arrived.len(), 1, "the redo never reported back");
    assert_eq!(arrived[0].error, None, "the redo was refused");
    assert!(dest.join("one.txt").exists(), "Ctrl+Y did not move it again");
    assert!(!one.exists(), "Ctrl+Y left the original where it was");
    assert!(history.can_undo() && !history.can_redo());

    crate::sandbox::remove(&root);
}

/// **An Alt-drag, and Ctrl+Z after it.**
///
/// [`Job::Link`] is the one job that goes through neither `IFileOperation` nor the Recycle Bin, so
/// what it reports is written by hand rather than by a sink — which makes "does undo work for it"
/// a real question rather than a consequence. It does, and this is why: the shortcuts it wrote are
/// the ones it reports, so the history's answer is a recycle of exactly those.
#[test]
#[cfg(windows)]
fn shortcuts_from_a_drop_can_be_undone() {
    use crate::shell::ops::history::History;

    let _serialised = crate::shell::serialised();
    crate::shell::init();
    let _for_real = for_real();

    let ctx = egui::Context::default();
    let root = sandbox("link-undo");
    let from = root.join("from");
    let into = root.join("into");
    std::fs::create_dir_all(&from).expect("sandbox");
    std::fs::create_dir_all(&into).expect("sandbox");
    let one = from.join("one.txt");
    let two = from.join("two.txt");
    std::fs::write(&one, b"one").expect("write");
    std::fs::write(&two, b"two").expect("write");

    let mut ops = Operations::new();
    let mut history = History::default();
    let settle = |ops: &mut Operations, history: &mut History| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut arrived: Vec<Done> = Vec::new();
        while std::time::Instant::now() < deadline {
            for done in ops.drain() {
                history.record(done.clone());
                arrived.push(done);
            }
            if ops.in_progress().is_none() {
                break;
            }
            std::thread::yield_now();
        }
        arrived
    };

    ops.start(
        Job::Link {
            items: vec![one.clone(), two.clone()],
            into: into.clone(),
        },
        Owner::default(),
        &ctx,
    );
    let arrived = settle(&mut ops, &mut history);
    assert_eq!(arrived.len(), 1, "the link job never reported back");
    assert_eq!(arrived[0].error, None);
    assert!(into.join("one.txt.lnk").is_file(), "no shortcut for one.txt");
    assert!(into.join("two.txt.lnk").is_file(), "no shortcut for two.txt");
    assert!(one.exists() && two.exists(), "the originals were touched");
    assert_eq!(
        arrived[0].outcome.created,
        [into.join("one.txt.lnk"), into.join("two.txt.lnk")],
        "a link job has to report what it wrote, or there is nothing to undo"
    );

    // ---- Ctrl+Z ----
    //
    // Which recycles them, exactly as the undo of a copy does — so this test puts two `.lnk` files
    // in the user's Recycle Bin, and takes them back out again at the end. An undo that deleted
    // them permanently would be the wrong operation to be testing.
    let back = history.undo().expect("an undo job");
    assert!(
        matches!(&back, Job::Delete { to_bin: true, items } if items.len() == 2),
        "the undo of a link should recycle the shortcuts: {back:?}"
    );
    ops.start_then(back, After::Settle, Owner::default(), &ctx);
    let arrived = settle(&mut ops, &mut history);
    assert_eq!(arrived.len(), 1, "the undo never reported back");
    assert_eq!(arrived[0].error, None, "the undo was refused");
    assert!(
        !into.join("one.txt.lnk").exists() && !into.join("two.txt.lnk").exists(),
        "Ctrl+Z left the shortcuts behind"
    );
    assert!(one.exists() && two.exists(), "and it took the originals");

    // ---- And out of the bin, so nothing of this is left in it ----
    //
    // From the records the recycle itself reported, which is the same route Ctrl+Z would take from
    // here — a second undo, of the undo. Doing it by the same mechanism rather than by hand is what
    // makes the tidying a check as well: two shortcuts went in, and these are the two that name
    // them.
    let recycled = arrived[0].outcome.recycled.clone();
    assert_eq!(
        recycled.len(),
        2,
        "the recycle did not say what went to the bin, so this test cannot clear up after itself"
    );
    assert_eq!(
        run_now(Job::Restore { items: recycled }),
        None,
        "the shortcuts are in the Recycle Bin, named for {}",
        into.display()
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !(into.join("one.txt.lnk").exists() && into.join("two.txt.lnk").exists())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        into.join("one.txt.lnk").exists() && into.join("two.txt.lnk").exists(),
        "the shortcuts did not come back out of the Recycle Bin, and are still in it under {}",
        into.display()
    );

    crate::sandbox::remove(&root);
}

/// Run a job to completion, on a thread of its own.
///
/// On a thread of its own because that is where production runs it, and because [`super::run`]
/// initialises an apartment and *uninitialises* it on the way out — doing that on the test thread
/// would tear down the apartment every other shell test on this thread is relying on, and the
/// failure would surface somewhere else entirely.
#[cfg(all(test, windows))]
fn run_now(job: Job) -> Option<String> {
    finish(job).0
}

/// The same, handing back what the shell said it did and asserting that it worked.
#[cfg(all(test, windows))]
fn outcome_of(job: Job) -> Outcome {
    let described = job.describe();
    let (error, outcome) = finish(job);
    assert_eq!(error, None, "{described} was refused");
    outcome
}

#[cfg(all(test, windows))]
fn finish(job: Job) -> (Option<String>, Outcome) {
    let ran = std::thread::spawn(move || super::run(&job, Owner::default()))
        .join()
        .expect("the operation thread panicked");
    (ran.error, ran.outcome)
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
        let handle = std::thread::spawn(move || super::run(&job, Owner::default()).error);

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
    let handle = std::thread::spawn(move || super::run(&job, Owner::default()).error);
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
    // Both undo jobs say the same thing, which is what they are from where the user is standing:
    // they pressed Ctrl+Z. Whether the shell is being asked to move a file or to empty one out of
    // the Recycle Bin is not something the status line should be explaining.
    assert_eq!(
        Job::PutBack {
            items: vec![(PathBuf::from("x"), PathBuf::from("y"))]
        }
        .describe(),
        "Putting 1 item back…"
    );
    assert_eq!(
        Job::Restore {
            items: vec![
                crate::shell::ops::Recycled {
                    from: PathBuf::from("x"),
                    bin: None
                },
                crate::shell::ops::Recycled {
                    from: PathBuf::from("y"),
                    bin: None
                },
            ]
        }
        .describe(),
        "Putting 2 items back…"
    );
    // Counted in what it makes rather than in what it is given, which is the one description here
    // that does not use `plural`.
    assert_eq!(
        Job::Link {
            items: vec![PathBuf::from("x")],
            into: PathBuf::from("z")
        }
        .describe(),
        "Making a shortcut…"
    );
    assert_eq!(
        Job::Link {
            items: vec![PathBuf::from("x"), PathBuf::from("y")],
            into: PathBuf::from("z")
        }
        .describe(),
        "Making 2 shortcuts…"
    );
}

/// The two undo jobs name the folders a pane has to re-read, at both ends.
///
/// A `PutBack` takes an item out of one folder and puts it in another, and a pane showing either
/// is a pane with a stale listing. Missing the source is the half that looks fine until you have
/// two panes open, which is what this program is for.
#[test]
fn the_undo_jobs_touch_both_ends() {
    let touched = Job::PutBack {
        items: vec![
            (PathBuf::from(r"C:\dest\one.txt"), PathBuf::from(r"C:\a\one.txt")),
            (PathBuf::from(r"C:\dest\two.txt"), PathBuf::from(r"C:\b\two.txt")),
        ],
    }
    .touches();
    assert_eq!(
        touched,
        [
            PathBuf::from(r"C:\a"),
            PathBuf::from(r"C:\b"),
            PathBuf::from(r"C:\dest"),
        ],
        "both ends of every pair, each folder once"
    );

    // A restore has only a destination: the Recycle Bin is not somewhere this program shows, so
    // there is nothing to re-read on the way out of it.
    assert_eq!(
        Job::Restore {
            items: vec![crate::shell::ops::Recycled {
                from: PathBuf::from(r"C:\a\one.txt"),
                bin: None,
            }],
        }
        .touches(),
        [PathBuf::from(r"C:\a")]
    );
}

/// One browsed archive in the sandbox, with a real file beside it — the fixture the two tests below
/// share.
///
/// Really written and really read, because the guards are a cache lookup: a path that was never
/// listed is not an archive whatever it is called, so a fixture that skipped the read would pass
/// against the bug.
#[cfg(windows)]
fn browsed_archive(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = crate::sandbox::fresh(name);
    let pkg = root.join("pkg.zip");
    crate::archive::tests::zip_at(&pkg, &[("a.txt", "first")]);
    std::fs::write(root.join("real.txt"), b"a file on the disk").expect("sandbox");
    crate::fs::scan::scan(&pkg);
    assert!(crate::archive::indexed(&pkg), "the fixture left no cached index");
    (root, pkg)
}

/// Start one job and take back what it said, `None` for a job the guard let through.
///
/// `FOR_REAL` is off in a test run, so the guard is the only thing that can produce a sentence and
/// a job that gets past it reports back having touched nothing. See [`FOR_REAL`].
#[cfg(windows)]
fn refusal(ops: &mut Operations, ctx: &egui::Context, job: Job) -> Option<String> {
    ops.start(job, Owner::default(), ctx);
    let done = ops.drain();
    assert_eq!(done.len(), 1, "the job never reported back");
    done[0].error.clone()
}

/// **A browsed archive can still be deleted**, which is the guard above read the right way round.
///
/// The funnel is where the regression belongs: the Delete key, `Ctrl+X`, a rename and a drag all
/// arrive here. The bug is told once, on
/// `archive::tests::a_browsed_archive_is_still_a_file_that_can_be_deleted`.
#[cfg(windows)]
#[test]
fn a_browsed_archive_can_still_be_deleted() {
    let (_root, pkg) = browsed_archive("ops-delete-archive");
    let ctx = egui::Context::default();
    let mut ops = Operations::new();

    // What must still be refused, first — because the delete below drops the cached index, which is
    // deliberate (see [`Job::emptied`]) and would make this half pass for the wrong reason after.
    let why = refusal(
        &mut ops,
        &ctx,
        Job::Delete {
            items: vec![pkg.join("a.txt")],
            to_bin: true,
        },
    );
    assert!(
        why.as_deref().is_some_and(|why| why.contains("inside an archive")),
        "a file inside an archive must not be handed to IFileOperation: {why:?}"
    );

    // And the archive itself, which is the bug.
    assert_eq!(
        refusal(
            &mut ops,
            &ctx,
            Job::Delete {
                items: vec![pkg.clone()],
                to_bin: true,
            },
        ),
        None,
        "deleting the archive itself was refused as though it were a file inside one"
    );

    // **And the index is still here**, deliberately: dropping it is the window's business once the
    // job has come back, not this funnel's, because a pane may still be showing the archive's rows
    // and the guard above is what stands between one of those rows and `IFileOperation`. See
    // [`Job::emptied`], and `app::tests::deleting_an_archive_stops_it_being_one` for the other end.
    assert!(
        crate::archive::indexed(&pkg),
        "the funnel dropped the index while the archive was still there"
    );
}

/// **And nothing can be written *into* an archive**, which is the other side of the same split.
///
/// `pkg.zip` is a real file, so the item guard says nothing about it — that being the whole point
/// of [`crate::archive::is_virtual_item`]. A paste, a drop, a shortcut or a new folder aimed at the
/// archive's *root* therefore has to be caught by its destination instead, or `IFileOperation` is
/// handed a folder that is a file. Both refusals are asserted, because a fix for either one alone
/// reads like a fix for both.
#[cfg(windows)]
#[test]
#[ignore = "fails on the GitHub Actions runner, passes on a desktop: browses a zip through the archive cache and the file-operation guard"]
fn nothing_can_be_written_into_a_browsed_archive() {
    let (root, pkg) = browsed_archive("ops-into-archive");
    let real = root.join("real.txt");
    let ctx = egui::Context::default();
    let mut ops = Operations::new();

    // A paste into the archive's root: every item is real, and the destination is the archive. Then
    // a new folder in it, which names no items at all.
    for job in [
        Job::Copy {
            items: vec![real.clone()],
            into: pkg.clone(),
        },
        Job::NewFolder {
            parent: pkg.clone(),
            name: "made-up".to_owned(),
        },
    ] {
        let what = job.describe();
        let why = refusal(&mut ops, &ctx, job);
        assert!(
            why.as_deref()
                .is_some_and(|why| why.contains("Nothing can be written into")),
            "{what} into an archive reached the shell: {why:?}"
        );
    }

    // While the same paste into the folder the archive is sitting in is nobody's business but the
    // shell's — the fixture would prove nothing if the destination guard refused everything.
    assert_eq!(
        refusal(
            &mut ops,
            &ctx,
            Job::Copy {
                items: vec![real],
                into: root.clone(),
            },
        ),
        None,
        "an ordinary paste was refused"
    );

    // And a copy empties nothing, which is why it leaves the index alone — the one arm of
    // [`Job::emptied`] that the app-level test would otherwise need a whole window to state.
    assert!(
        Job::Copy {
            items: vec![pkg],
            into: root,
        }
        .emptied()
        .is_empty(),
        "a copy takes nothing away from where it was"
    );
}
