//! The engine against real files, every one of them under `target/sandbox`.
//!
//! None of this reaches the shell — the engine is `CopyFile2` and `MoveFileExW` — so none of it
//! needs [`crate::shell::ops::for_real`]. The sandbox guard inside [`super::win::run`] holds it to
//! the sandbox all the same.

use super::*;
use std::fs;

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn copy(items: Vec<PathBuf>, into: &Path) -> Job {
    Job::Copy {
        items,
        into: into.to_path_buf(),
    }
}

fn moving(items: Vec<PathBuf>, into: &Path) -> Job {
    Job::Move {
        items,
        into: into.to_path_buf(),
    }
}

/// Run a job to the end on this thread, and say what it came back as.
fn ran(job: &Job, transfer: &Transfer) -> Ran {
    run(job, transfer).expect("a job this engine takes")
}

#[test]
fn only_plain_paths_somewhere_else_are_taken() {
    let into = Path::new(r"D:\dest");
    let elsewhere = vec![PathBuf::from(r"C:\src\one.txt")];
    assert!(suits(&elsewhere, into));
    assert!(suits(&[PathBuf::from(r"\\server\share\a")], into));
    // Already there: the shell's localised `- Copy` name, which nothing here can make.
    assert!(!suits(&[PathBuf::from(r"D:\dest\one.txt")], into));
    assert!(!suits(&[PathBuf::from(r"d:\DEST\one.txt")], into));
    // Into itself, and below itself.
    assert!(!suits(&[PathBuf::from(r"D:\dest")], into));
    assert!(!suits(&[PathBuf::from(r"D:\")], Path::new(r"D:\dest")));
    assert!(!suits(&[PathBuf::from(r"D:\de")], Path::new(r"D:\de\st")));
    // But a sibling that merely starts with the same letters is somewhere else.
    assert!(suits(&[PathBuf::from(r"D:\de")], Path::new(r"D:\dest")));
    // Not a path this engine can act on.
    assert!(!suits(&elsewhere, Path::new(crate::fs::recycle::LOCATION)));
    assert!(!suits(&[], into));
}

#[test]
fn keep_both_numbers_before_the_extension() {
    let taken = |taken: &'static [&'static str]| move |p: &Path| taken.iter().any(|t| p == Path::new(t));
    assert_eq!(
        free_name(Path::new(r"C:\d\one.txt"), taken(&[])),
        PathBuf::from(r"C:\d\one (2).txt")
    );
    assert_eq!(
        free_name(Path::new(r"C:\d\one.txt"), taken(&[r"C:\d\one (2).txt"])),
        PathBuf::from(r"C:\d\one (3).txt")
    );
    assert_eq!(
        free_name(Path::new(r"C:\d\.gitignore"), taken(&[])),
        PathBuf::from(r"C:\d\.gitignore (2)")
    );
    assert_eq!(
        free_name(Path::new(r"C:\d\Makefile"), taken(&[])),
        PathBuf::from(r"C:\d\Makefile (2)")
    );
}

#[cfg(windows)]
#[test]
fn a_tree_is_copied_whole_and_recorded_once() {
    let root = crate::sandbox::fresh("fast-copy-tree");
    let src = root.join("src");
    write(&src.join("a.txt"), "alpha");
    write(&src.join("sub").join("b.txt"), "bravo");
    fs::create_dir_all(src.join("sub").join("empty")).unwrap();
    // Big enough for more than one chunk, so the progress callback is what counts it.
    let big = vec![7u8; 3 * 1024 * 1024];
    fs::write(src.join("big.bin"), &big).unwrap();
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = copy(vec![src.clone()], &dest);
    let transfer = Transfer::new(1, &job, None);
    let ran = ran(&job, &transfer);

    assert_eq!(ran.error, None);
    assert!(!ran.aborted);
    let there = dest.join("src");
    assert_eq!(read(&there.join("a.txt")), "alpha");
    assert_eq!(read(&there.join("sub").join("b.txt")), "bravo");
    assert!(there.join("sub").join("empty").is_dir());
    assert_eq!(fs::read(there.join("big.bin")).unwrap(), big);
    // The folder is what undo takes back, and nothing inside it: that goes with it.
    assert_eq!(ran.outcome.created, vec![there]);
    // The source is untouched.
    assert_eq!(read(&src.join("a.txt")), "alpha");

    let done = transfer.snapshot();
    assert_eq!(done.phase, Phase::Finished);
    assert_eq!((done.files_found, done.files_done), (3, 3));
    assert_eq!(done.bytes_done, done.bytes_found);
}

#[cfg(windows)]
#[test]
fn a_folder_that_is_there_already_is_merged_and_never_recorded() {
    let root = crate::sandbox::fresh("fast-copy-merge");
    let src = root.join("src");
    write(&src.join("new.txt"), "new");
    write(&src.join("same.txt"), "incoming");
    write(&src.join("fresh").join("c.txt"), "charlie");
    let dest = root.join("dest");
    write(&dest.join("src").join("same.txt"), "existing");
    write(&dest.join("src").join("theirs.txt"), "theirs");

    let job = copy(vec![src.clone()], &dest);
    let transfer = Transfer::new(1, &job, None);
    transfer.always(Choice::KeepBoth);
    let ran = ran(&job, &transfer);

    assert_eq!(ran.error, None);
    let there = dest.join("src");
    assert_eq!(read(&there.join("same.txt")), "existing");
    assert_eq!(read(&there.join("same (2).txt")), "incoming");
    assert_eq!(read(&there.join("theirs.txt")), "theirs");
    assert_eq!(read(&there.join("fresh").join("c.txt")), "charlie");
    // **Not `dest\src`**, which was there first: undo recycling it would take `theirs.txt` along.
    let mut created = ran.outcome.created.clone();
    created.sort();
    let mut expected = vec![
        there.join("fresh"),
        there.join("new.txt"),
        there.join("same (2).txt"),
    ];
    expected.sort();
    assert_eq!(created, expected);
}

#[cfg(windows)]
#[test]
fn skip_and_replace_do_what_they_say() {
    for (choice, expected) in [(Choice::Skip, "existing"), (Choice::Replace, "incoming")] {
        let root = crate::sandbox::fresh("fast-copy-clash");
        let file = root.join("src").join("one.txt");
        write(&file, "incoming");
        let dest = root.join("dest");
        write(&dest.join("one.txt"), "existing");

        let job = copy(vec![file], &dest);
        let transfer = Transfer::new(1, &job, None);
        transfer.always(choice);
        let ran = ran(&job, &transfer);

        assert_eq!(ran.error, None, "{choice:?}");
        assert_eq!(read(&dest.join("one.txt")), expected, "{choice:?}");
        // Neither made anything new for undo to take away: a skip made nothing, and a replace
        // left the same name holding different bytes.
        assert!(ran.outcome.created.is_empty(), "{choice:?}");
    }
}

/// The real conversation: a worker stops on the question, the window answers it, the worker goes on.
#[cfg(windows)]
#[test]
fn a_conflict_waits_for_its_answer() {
    let root = crate::sandbox::fresh("fast-copy-ask");
    let file = root.join("src").join("one.txt");
    write(&file, "incoming");
    let dest = root.join("dest");
    write(&dest.join("one.txt"), "existing");

    let job = copy(vec![file], &dest);
    let transfer = Transfer::new(1, &job, None);
    let ran = std::thread::scope(|scope| {
        let worker = scope.spawn(|| ran(&job, &transfer));
        let asked = loop {
            if let Some(clash) = transfer.snapshot().question {
                break clash;
            }
            assert!(!worker.is_finished(), "finished without asking");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(asked.name, "one.txt");
        assert_eq!(asked.incoming.size, 8);
        assert_eq!(asked.existing.size, 8);
        transfer.answer(Choice::Replace, false);
        worker.join().unwrap()
    });
    assert_eq!(ran.error, None);
    assert_eq!(read(&dest.join("one.txt")), "incoming");
    assert!(transfer.snapshot().question.is_none());
}

#[cfg(windows)]
#[test]
fn a_move_inside_one_volume_is_a_rename() {
    let root = crate::sandbox::fresh("fast-move-rename");
    let src = root.join("src");
    write(&src.join("a.txt"), "alpha");
    write(&src.join("sub").join("b.txt"), "bravo");
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = moving(vec![src.clone()], &dest);
    let transfer = Transfer::new(1, &job, None);
    let ran = ran(&job, &transfer);

    assert_eq!(ran.error, None);
    assert!(!src.exists());
    assert_eq!(read(&dest.join("src").join("sub").join("b.txt")), "bravo");
    assert_eq!(ran.outcome.moved, vec![(src, dest.join("src"))]);
}

#[cfg(windows)]
#[test]
fn a_move_onto_a_folder_merges_into_it() {
    let root = crate::sandbox::fresh("fast-move-merge");
    let src = root.join("src");
    write(&src.join("a.txt"), "alpha");
    write(&src.join("same.txt"), "incoming");
    let dest = root.join("dest");
    write(&dest.join("src").join("theirs.txt"), "theirs");
    write(&dest.join("src").join("same.txt"), "existing");

    let job = moving(vec![src.clone()], &dest);
    let transfer = Transfer::new(1, &job, None);
    transfer.always(Choice::Skip);
    let ran = ran(&job, &transfer);

    assert_eq!(ran.error, None);
    let there = dest.join("src");
    assert_eq!(read(&there.join("a.txt")), "alpha");
    assert_eq!(read(&there.join("theirs.txt")), "theirs");
    assert_eq!(read(&there.join("same.txt")), "existing");
    // What was skipped is still at the source, so the source folder stays to hold it.
    assert_eq!(read(&src.join("same.txt")), "incoming");
    assert!(!src.join("a.txt").exists());
    assert_eq!(ran.outcome.moved, vec![(src.join("a.txt"), there.join("a.txt"))]);
}

#[cfg(windows)]
#[test]
fn a_cancelled_job_says_so() {
    let root = crate::sandbox::fresh("fast-copy-cancel");
    let src = root.join("src");
    write(&src.join("a.txt"), "alpha");
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = copy(vec![src], &dest);
    let transfer = Transfer::new(1, &job, None);
    transfer.cancel();
    let ran = ran(&job, &transfer);

    assert!(ran.aborted);
    assert!(!dest.join("src").exists());
}

/// A junction inside a tree is copied as a junction, pointing where it pointed — never followed.
#[cfg(windows)]
#[test]
fn a_junction_is_copied_as_a_link() {
    let root = crate::sandbox::fresh("fast-copy-junction");
    let target = root.join("target");
    write(&target.join("inside.txt"), "inside");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    let link = src.join("link");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = copy(vec![src], &dest);
    let transfer = Transfer::new(1, &job, None);
    let ran = ran(&job, &transfer);

    assert_eq!(ran.error, None);
    let copied = dest.join("src").join("link");
    let meta = fs::symlink_metadata(&copied).unwrap();
    use std::os::windows::fs::MetadataExt;
    assert_ne!(meta.file_attributes() & 0x400, 0, "not a reparse point");
    // Through the copy, the original target.
    assert_eq!(read(&copied.join("inside.txt")), "inside");
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn the_shell_keeps_what_is_its_own() {
    let root = crate::sandbox::fresh("fast-copy-declines");
    let file = root.join("one.txt");
    write(&file, "one");
    // Into its own folder: the shell's `one - Copy.txt`.
    let job = copy(vec![file.clone()], &root);
    assert!(run(&job, &Transfer::new(1, &job, None)).is_none());
    // Nothing at the path.
    let job = copy(vec![root.join("missing.txt")], &root.join("elsewhere"));
    assert!(run(&job, &Transfer::new(1, &job, None)).is_none());
    // Not a copy at all.
    let job = Job::Delete {
        items: vec![file],
        to_bin: true,
    };
    assert!(run(&job, &Transfer::new(1, &job, None)).is_none());
}

/// `mklink /J`, inside the sandbox.
#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
}

/// A destination that is the item under another name — here through a junction back up the tree —
/// is the shell's, though the paths as written say it is somewhere else. Followed, the walk would
/// list the folder it was creating inside, and go on making `src\inner\src\inner\…` until the
/// paths ran out.
#[cfg(windows)]
#[test]
fn a_folder_into_itself_under_another_name_is_the_shells() {
    let root = crate::sandbox::fresh("fast-copy-alias");
    let src = root.join("src");
    write(&src.join("inner").join("a.txt"), "alpha");
    let alias = root.join("alias");
    junction(&alias, &root);
    let inner = alias.join("src").join("inner");

    // Into itself.
    assert!(suits(std::slice::from_ref(&src), &inner), "the paths alone should not see it");
    let job = copy(vec![src.clone()], &inner);
    assert!(run(&job, &Transfer::new(1, &job, None)).is_none());
    // Into the folder it is already in.
    let job = copy(vec![src.join("inner").join("a.txt")], &inner);
    assert!(run(&job, &Transfer::new(1, &job, None)).is_none());
    // Nothing was made by either.
    assert_eq!(fs::read_dir(src.join("inner")).unwrap().count(), 1);
}

/// Somewhere this process may not write is the shell's, which can ask for elevation.
#[cfg(windows)]
#[test]
fn a_folder_this_process_may_not_write_into_is_the_shells() {
    const ADD: u32 = 0x2 | 0x4; // FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY
    let root = crate::sandbox::fresh("fast-copy-denied");
    let file = root.join("one.txt");
    write(&file, "one");
    let locked = root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    let icacls = |args: &[&str]| {
        let done = std::process::Command::new("icacls")
            .arg(&locked)
            .args(args)
            .output()
            .unwrap();
        assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stdout));
    };
    // Everyone, denied adding a file or a folder — which is what a protected folder says to a
    // process that is not elevated.
    icacls(&["/deny", "*S-1-1-0:(AD,WD)"]);

    let open = win::may(&root, ADD);
    let shut = win::may(&locked, ADD);
    let job = copy(vec![file], &locked);
    let declined = run(&job, &Transfer::new(1, &job, None)).is_none();
    // Lifted before asserting, so a failure does not leave the next run a folder it cannot clear.
    icacls(&["/remove:d", "*S-1-1-0"]);

    assert!(open, "the sandbox itself read as not writable");
    assert!(!shut, "the denial was not seen");
    assert!(declined, "a copy into it was not handed to the shell");
    assert_eq!(fs::read_dir(&locked).unwrap().count(), 0);
}

/// Too big for FAT32, and not enough room: both said before the bytes are written, and the second
/// one waits for its answer with nothing being copied meanwhile.
#[cfg(windows)]
#[test]
fn a_file_too_big_and_a_disk_too_full_are_said_before_the_copy() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static FREE: AtomicU64 = AtomicU64::new(2);
    fn measure(_: &Path) -> Option<u64> {
        Some(FREE.load(Ordering::SeqCst))
    }

    let root = crate::sandbox::fresh("fast-copy-room");
    let src = root.join("src");
    write(&src.join("big.bin"), "0123456789");
    write(&src.join("small.txt"), "abc");
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = copy(vec![src], &dest);
    let transfer = Transfer::new(1, &job, None);
    // A FAT32 stick with a five-byte limit and two bytes free.
    let space = |_: &Path| win::Space {
        free: 2,
        limit: Some(5),
        needed: 0,
        asking: true,
        measure,
    };
    let ran = std::thread::scope(|scope| {
        let worker = scope.spawn(|| win::run_with(&job, &transfer, space).unwrap());
        let short = loop {
            if let Some(short) = transfer.snapshot().short {
                break short;
            }
            assert!(!worker.is_finished(), "finished without asking about room");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!((short.needed, short.free), (3, 2));
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !dest.join("src").join("small.txt").exists(),
            "a file was copied while the question was up"
        );
        // Something has been cleared off the disk, and looking again finds it.
        FREE.store(1 << 40, Ordering::SeqCst);
        transfer.make_room(Room::TryAgain);
        worker.join().unwrap()
    });

    assert_eq!(read(&dest.join("src").join("small.txt")), "abc");
    assert!(!dest.join("src").join("big.bin").exists());
    assert_eq!(transfer.snapshot().failed, 1);
    let error = ran.error.expect("the file too big was not reported");
    assert!(error.contains("FAT32"), "{error}");
}

/// A sandbox file held open with no sharing at all, the way a program that has it open for writing
/// keeps everybody else out.
#[cfg(windows)]
fn hold_open(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// Wait for the job on `worker` to stop on a snag, and say what it is.
#[cfg(windows)]
fn snagged<T>(transfer: &Transfer, worker: &std::thread::ScopedJoinHandle<'_, T>) -> Snag {
    loop {
        if let Some(snag) = transfer.snapshot().snag {
            return snag;
        }
        assert!(!worker.is_finished(), "finished without asking about the file in use");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A file another program has open is asked about, not failed — and once that program lets go,
/// Try again copies it.
#[cfg(windows)]
#[test]
fn a_file_in_use_is_asked_about_and_tried_again() {
    let root = crate::sandbox::fresh("fast-copy-in-use");
    let file = root.join("src").join("busy.txt");
    write(&file, "busy");
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = copy(vec![file.clone()], &dest);
    let transfer = Transfer::new(1, &job, None);
    let held = hold_open(&file);
    let ran = std::thread::scope(|scope| {
        let worker = scope.spawn(|| ran(&job, &transfer));
        let snag = snagged(&transfer, &worker);
        assert_eq!(snag.trouble, Trouble::InUse);
        assert_eq!(snag.path, file);
        drop(held);
        transfer.mend(Mend::TryAgain, false);
        worker.join().unwrap()
    });
    assert_eq!(ran.error, None);
    assert_eq!(read(&dest.join("busy.txt")), "busy");
    assert_eq!(ran.outcome.created, vec![dest.join("busy.txt")]);
}

/// Skip is the user's answer, not a failure: nothing lands, nothing is reported as wrong, and the
/// rest of the job goes on.
#[cfg(windows)]
#[test]
fn a_file_in_use_that_is_skipped_is_not_a_failure() {
    let root = crate::sandbox::fresh("fast-copy-in-use-skip");
    let src = root.join("src");
    write(&src.join("busy.txt"), "busy");
    write(&src.join("free.txt"), "free");
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = copy(vec![src.clone()], &dest);
    let transfer = Transfer::new(1, &job, None);
    transfer.always_in_use(Mend::Skip);
    let held = hold_open(&src.join("busy.txt"));
    let ran = ran(&job, &transfer);
    drop(held);

    assert_eq!(ran.error, None);
    assert!(!dest.join("src").join("busy.txt").exists());
    assert_eq!(read(&dest.join("src").join("free.txt")), "free");
    assert_eq!(transfer.snapshot().failed, 0);
}

/// The same for a move inside one volume, where it is the rename that the open file refuses.
#[cfg(windows)]
#[test]
fn a_file_in_use_holds_up_its_move_until_tried_again() {
    let root = crate::sandbox::fresh("fast-move-in-use");
    let file = root.join("src").join("busy.txt");
    write(&file, "busy");
    let dest = root.join("dest");
    fs::create_dir_all(&dest).unwrap();

    let job = moving(vec![file.clone()], &dest);
    let transfer = Transfer::new(1, &job, None);
    let held = hold_open(&file);
    let ran = std::thread::scope(|scope| {
        let worker = scope.spawn(|| ran(&job, &transfer));
        assert_eq!(snagged(&transfer, &worker).trouble, Trouble::InUse);
        drop(held);
        transfer.mend(Mend::TryAgain, false);
        worker.join().unwrap()
    });
    assert_eq!(ran.error, None);
    assert!(!file.exists());
    assert_eq!(read(&dest.join("busy.txt")), "busy");
    assert_eq!(ran.outcome.moved, vec![(file, dest.join("busy.txt"))]);
}
