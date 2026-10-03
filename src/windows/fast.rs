//! `CopyFile2` on a pool of threads, `MoveFileExW` for whatever can be renamed, `DeleteFileW` for a
//! permanent delete, and `FindFirstFileExW` for the walk that feeds them all.
//!
//! The Windows half of [`crate::shell::ops::fast`], which has the argument for all of it. This
//! file is the mechanics:
//!
//! - **The walk runs on the job's own thread and the work on the pool.** For a copy, folders are
//!   made by the walk as it reaches them, before any file inside them is queued, so a worker never
//!   finds its folder missing. Each folder's times are set at the end, once everything has landed
//!   in it: setting them earlier is undone by the first file written inside.
//! - **A move is a rename first.** Inside one volume that is the whole job, however big the folder.
//!   `ERROR_NOT_SAME_DEVICE` sends the item through the copy instead, and the source of each file
//!   goes as soon as its copy has landed — so a cancelled move leaves every file at one end or the
//!   other and never at both, and never at neither.
//! - **A delete is files first, folders last.** The pool deletes what the walk lists; the folders
//!   are removed afterwards, deepest first, and one that is not empty stays.
//! - **Conflicts are found by failing, not by looking.** `COPY_FILE_FAIL_IF_EXISTS` costs nothing on
//!   the files that do not clash, which is all of them in the usual case. Asking whether each
//!   destination exists first would be a round trip per file, which over a share is the cost this
//!   engine is here to avoid.
//! - **A link is the link, never what it points at.** A file symbolic link is copied through
//!   `COPY_FILE_COPY_SYMLINK`; a junction or a directory link has its reparse data read and written
//!   onto a new empty folder; either is deleted as itself. None is followed, which is the rule
//!   everywhere else in this program.

use std::ffi::c_void;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_DIR_NOT_EMPTY, ERROR_ENCRYPTION_FAILED,
    ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION, ERROR_NOT_SAME_DEVICE,
    ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, ERROR_REQUEST_ABORTED, ERROR_SHARING_VIOLATION,
    FILETIME, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CopyFile2, CreateDirectoryExW, CreateDirectoryW, CreateFileW, DeleteFileW, FindClose,
    FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetFileAttributesW,
    GetVolumePathNameW, MoveFileExW, RemoveDirectoryW, SetFileAttributesW, SetFileTime,
    COPYFILE2_CALLBACK_CHUNK_FINISHED, COPYFILE2_EXTENDED_PARAMETERS, COPYFILE2_MESSAGE,
    COPYFILE2_MESSAGE_ACTION, COPYFILE2_PROGRESS_CANCEL, COPYFILE2_PROGRESS_CONTINUE,
    COPY_FILE_ALLOW_DECRYPTED_DESTINATION, COPY_FILE_COPY_SYMLINK, COPY_FILE_FAIL_IF_EXISTS,
    COPY_FILE_NO_BUFFERING, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_ATTRIBUTES, FIND_FIRST_EX_LARGE_FETCH, INVALID_FILE_ATTRIBUTES,
    MOVEFILE_REPLACE_EXISTING, OPEN_EXISTING, WIN32_FIND_DATAW,
};

use crate::shell::ops::fast::{
    erasable, free_name, suits, Choice, Clash, Confirm, Facts, Mend, Phase, Room, Short, Snag,
    Transfer, Trouble,
};
use crate::shell::ops::{Job, Outcome, Ran};

/// From this size up a file is copied unbuffered: it would only push everything else out of the
/// cache to be read once, and unbuffered is what `robocopy /J` measures faster for large files.
const UNBUFFERED: u64 = 256 * 1024 * 1024;

/// How many files are in flight at once, for each kind of path.
///
/// A share is the case parallelism exists for — each file is a create, a write and a close, every
/// one a round trip — so it gets the most. A disk that seeks gets one: two files in flight on one
/// spindle is the head going back and forth between them, which is slower than either alone.
const OVER_NETWORK: usize = 8;
const SOLID_STATE: usize = 4;
const SPINNING: usize = 1;

/// Do the job, or `None` if it is the shell's. See [`crate::shell::ops::fast`] for which.
pub(crate) fn run(job: &Job, transfer: &Transfer) -> Option<Ran> {
    run_with(job, transfer, Space::of)
}

/// [`run`], with what the destination can hold supplied rather than asked — which is how a test
/// on the sandbox's own drive gets to be short of room or on FAT32.
pub(crate) fn run_with(
    job: &Job,
    transfer: &Transfer,
    space: impl FnOnce(&Path) -> Space,
) -> Option<Ran> {
    let (items, into, moving) = match job {
        Job::Copy { items, into } => (items, into, false),
        Job::Move { items, into } => (items, into, true),
        // Shift+Delete. Delete to the Recycle Bin is never this engine's — see the module header
        // of [`crate::shell::ops::fast`].
        Job::Delete {
            items,
            to_bin: false,
        } => return erase(items, transfer),
        _ => return None,
    };
    if !suits(items, into) {
        return None;
    }
    // The destination has to be a plain folder: a cloud folder or a link is a decision the shell
    // knows how to make and this does not.
    let target = stat(into)?;
    if !target.is_dir() || target.is_reparse() {
        return None;
    }
    let top = look(items)?;
    // A folder into itself under another name — a `subst` drive, a mapped drive, a junction on the
    // way — which the text of the paths cannot see and the walk would follow for ever.
    if aliased(&top, into) {
        return None;
    }
    // Somewhere only an administrator may write, or a move out of somewhere only an administrator
    // may delete from. The shell puts up the elevation prompt; this would fail file by file.
    //
    // Not asked of a share, where there is no prompt to be had and the server's own rules are not
    // the ones a local check would read.
    if !crate::shell::over_network(into) && !allowed(&top, into, moving) {
        return None;
    }

    #[cfg(test)]
    crate::sandbox::guard(
        "the fast copy",
        &items
            .iter()
            .cloned()
            .chain(std::iter::once(into.clone()))
            .collect::<Vec<_>>(),
    );

    transfer.set_phase(Phase::Running);
    let engine = Engine {
        transfer,
        moving,
        outcome: Mutex::new(Outcome::default()),
    };
    let mut walk = Walk {
        space: space(into),
        ..Walk::default()
    };
    pool(
        transfer,
        workers(items, into),
        "fast-copy",
        |task| engine.copy_file(task),
        |tasks| {
            for (item, found) in &top {
                if transfer.cancelled() {
                    break;
                }
                let Some(name) = item.file_name() else { continue };
                let to = into.join(name);
                if moving {
                    engine.move_item(item, *found, to, tasks, &mut walk);
                } else {
                    engine.copy_tree(item, *found, to, true, tasks, &mut walk);
                }
            }
        },
    );
    engine.settle(&walk);
    let outcome = engine.outcome.into_inner().unwrap_or_else(|p| p.into_inner());

    // The destination; the folders a move emptied; and each folder that something new was made in,
    // which for a merge is deeper than the destination.
    let mut changed: Vec<&Path> = vec![into];
    if moving {
        changed.extend(items.iter().filter_map(|item| item.parent()));
    }
    changed.extend(outcome.created.iter().filter_map(|made| made.parent()));
    for (was, now) in &outcome.moved {
        changed.extend(was.parent());
        changed.extend(now.parent());
    }
    tell_the_shell(changed);

    let ran = transfer.ran(outcome);
    transfer.set_phase(Phase::Finished);
    Some(ran)
}

/// Each top-level item's own entry, or `None` when any is one for the shell: gone, a reparse point
/// — a link, which is the shell's to act on as a link — or a cloud placeholder, which is the
/// provider's to ask about.
fn look(items: &[PathBuf]) -> Option<Vec<(&PathBuf, Found)>> {
    items
        .iter()
        .map(|item| {
            let found = stat(item)?;
            (!found.is_reparse() && !found.is_placeholder()).then_some((item, found))
        })
        .collect()
}

/// Run `feed` on this thread while `threads` workers take what it sends and hand each to `each`.
///
/// Every worker drains rather than abandons the queue once the job is cancelled, so `feed` is never
/// left blocked on it; and holds before each item while the job is paused or short of room. When
/// not one thread could be had, what was queued is worked through here once `feed` is done —
/// slower, and still the job that was asked for.
fn pool<T: Send>(
    transfer: &Transfer,
    threads: usize,
    name: &str,
    each: impl Fn(T) + Sync,
    feed: impl FnOnce(&Sender<T>),
) {
    let (tx, rx) = channel::<T>();
    let rx = Mutex::new(rx);
    let take = || loop {
        let task = match rx.lock() {
            Ok(queue) => match queue.recv() {
                Ok(task) => task,
                Err(_) => return,
            },
            Err(_) => return,
        };
        if transfer.cancelled() {
            continue;
        }
        transfer.hold();
        each(task);
    };
    std::thread::scope(|scope| {
        let mut spawned = 0;
        for n in 0..threads {
            let started = std::thread::Builder::new()
                .name(format!("{name}-{n}"))
                .spawn_scoped(scope, take);
            spawned += usize::from(started.is_ok());
        }
        feed(&tx);
        transfer.counted();
        // Closing the queue is what lets the workers go once it is empty.
        drop(tx);
        if spawned == 0 {
            take();
        }
    });
}

// ---------------------------------------------------------------------------
// Copy and move
// ---------------------------------------------------------------------------

/// What the walk leaves to do once every file has landed, and the room it has spoken for.
#[derive(Default)]
struct Walk {
    /// Folders made here, with the times to give them back.
    made: Vec<(PathBuf, Found)>,
    /// Source folders a move has emptied, in the order they were reached — so backwards is
    /// children before parents.
    emptied: Vec<(PathBuf, Found)>,
    space: Space,
}

/// What the destination can hold, and how much of it the walk has spoken for.
pub(crate) struct Space {
    /// Free when the job started, as far as this job is concerned: what is free now plus what the
    /// job has written since. Refreshed whenever the walk thinks it has run out.
    pub(crate) free: u64,
    /// The largest file the destination's file system can hold, when it has a limit worth
    /// knowing about — FAT32's 4 GB.
    pub(crate) limit: Option<u64>,
    /// What every file the walk has queued adds up to.
    pub(crate) needed: u64,
    /// Whether to keep checking: off once the user has said to go on regardless, and off when
    /// the destination would not say how much room it has.
    pub(crate) asking: bool,
    /// How to ask what is free now. [`free_space`], but for a test.
    pub(crate) measure: fn(&Path) -> Option<u64>,
}

impl Default for Space {
    fn default() -> Self {
        Self {
            free: u64::MAX,
            limit: None,
            needed: 0,
            asking: false,
            measure: free_space,
        }
    }
}

impl Space {
    pub(crate) fn of(into: &Path) -> Self {
        let free = free_space(into);
        Self {
            free: free.unwrap_or(u64::MAX),
            limit: largest_file(into),
            asking: free.is_some(),
            ..Self::default()
        }
    }
}

/// One file for a worker.
struct Task {
    from: PathBuf,
    to: PathBuf,
    found: Found,
    /// Whether this file is the first new thing at its place in the tree, and so what undo will
    /// take back. See [`crate::shell::ops::fast`]'s header on what an [`Outcome`] may and may not
    /// name.
    record: bool,
}

struct Engine<'a> {
    transfer: &'a Transfer,
    moving: bool,
    outcome: Mutex<Outcome>,
}

impl Engine<'_> {
    /// Say what was made, for undo: a copy's new item, or where a move took one.
    fn record(&self, from: &Path, to: &Path) {
        if let Ok(mut outcome) = self.outcome.lock() {
            if self.moving {
                outcome.moved.push((from.to_path_buf(), to.to_path_buf()));
            } else {
                outcome.created.push(to.to_path_buf());
            }
        }
    }

    // ---- The walk ------------------------------------------------------------

    /// Copy `from` to `to`: a folder by making it and queueing what is inside, a file by queueing
    /// it. `record` is whether what is made at `to` is new to its parent — true at the top, and
    /// below any folder that was already there.
    fn copy_tree(
        &self,
        from: &Path,
        found: Found,
        to: PathBuf,
        record: bool,
        tasks: &Sender<Task>,
        walk: &mut Walk,
    ) {
        let mut stack = vec![(from.to_path_buf(), found, to, record)];
        while let Some((from, found, to, record)) = stack.pop() {
            if self.transfer.cancelled() {
                return;
            }
            if !found.is_dir() {
                self.transfer.found(1, found.size);
                // Said before a byte is written, as the shell says it, rather than after the copy
                // has filled the stick to the limit and failed.
                if walk.space.limit.is_some_and(|limit| found.size > limit) {
                    self.transfer.failed(
                        &from,
                        "too big for this drive, whose FAT32 file system holds files of up to 4 GB"
                            .to_owned(),
                    );
                    self.transfer.moved_bytes(found.size);
                    self.transfer.finished_one();
                    continue;
                }
                walk.space.needed += found.size;
                if !self.room_for(&mut walk.space) {
                    return;
                }
                let _ = tasks.send(Task {
                    from,
                    to,
                    found,
                    record,
                });
                continue;
            }
            if found.is_link() {
                self.transfer.found(1, 0);
                match copy_link(&from, &to) {
                    Ok(()) => {
                        if record {
                            self.record(&from, &to);
                        }
                        if self.moving {
                            remove(&from, found);
                        }
                    }
                    Err(why) => self.transfer.failed(&from, why),
                }
                self.transfer.finished_one();
                continue;
            }
            // The template carries the folder's attributes across — hidden, compressed, encrypted.
            let made = unsafe {
                CreateDirectoryExW(
                    verbatim(&from).as_ptr(),
                    verbatim(&to).as_ptr(),
                    std::ptr::null(),
                )
            } != 0;
            if !made {
                let code = unsafe { GetLastError() };
                let already =
                    code == ERROR_ALREADY_EXISTS && stat(&to).is_some_and(|there| there.is_dir());
                if !already {
                    let why = if code == ERROR_ALREADY_EXISTS {
                        "a file with that name is already there".to_owned()
                    } else {
                        message(code)
                    };
                    self.transfer.failed(&from, why);
                    continue;
                }
            }
            if made {
                if record {
                    self.record(&from, &to);
                }
                walk.made.push((to.clone(), found));
            }
            if self.moving {
                walk.emptied.push((from.clone(), found));
            }
            let listed = list(&from, |name, child| {
                let name = OsString::from_wide(name);
                stack.push((from.join(&name), child, to.join(&name), !made));
            });
            if let Err(code) = listed {
                self.transfer.failed(&from, message(code));
            }
        }
    }

    /// Whether there is room for what the walk has queued, asking when there is not. `false` when
    /// the job was cancelled instead.
    fn room_for(&self, space: &mut Space) -> bool {
        let into = &self.transfer.into;
        loop {
            if !space.asking || space.needed <= space.free {
                return true;
            }
            // Looked at again rather than believed: something may have been cleared since the job
            // began, and the job's own writes are what used the rest.
            let written = self.transfer.written();
            let Some(now) = (space.measure)(into) else {
                space.asking = false;
                return true;
            };
            space.free = now.saturating_add(written);
            if space.needed <= space.free {
                return true;
            }
            let short = Short {
                into: into.clone(),
                needed: space.needed.saturating_sub(written),
                free: now,
            };
            match self.transfer.ask_room(short) {
                None => return false,
                Some(Room::Anyway) => space.asking = false,
                Some(Room::TryAgain) => {}
            }
        }
    }

    /// Move one item: a rename where the volume allows it, and otherwise the copy above with each
    /// source removed as it lands.
    fn move_item(
        &self,
        from: &Path,
        found: Found,
        to: PathBuf,
        tasks: &Sender<Task>,
        walk: &mut Walk,
    ) {
        let mut stack = vec![(from.to_path_buf(), found, to)];
        while let Some((from, found, mut to)) = stack.pop() {
            if self.transfer.cancelled() {
                return;
            }
            let mut flags = 0;
            loop {
                self.transfer.hold();
                let moved = unsafe {
                    MoveFileExW(verbatim(&from).as_ptr(), verbatim(&to).as_ptr(), flags)
                } != 0;
                if moved {
                    self.transfer.passed_one(found.size);
                    self.record(&from, &to);
                    break;
                }
                let code = unsafe { GetLastError() };
                if code == ERROR_NOT_SAME_DEVICE {
                    self.copy_tree(&from, found, to, true, tasks, walk);
                    break;
                }
                if in_use(code) {
                    match snag(self.transfer, &from, Trouble::InUse, code) {
                        Some(Mend::TryAgain) => continue,
                        None => return,
                        Some(_) => {
                            self.transfer.passed_one(found.size);
                            break;
                        }
                    }
                }
                if code != ERROR_ALREADY_EXISTS && code != ERROR_FILE_EXISTS {
                    self.transfer.failed(&from, message(code));
                    break;
                }
                let Some(there) = stat(&to) else {
                    self.transfer.failed(&from, message(code));
                    break;
                };
                // A folder onto a folder is a merge, as it is in Explorer: what is inside goes in,
                // one item at a time, and the source folder goes at the end if that emptied it.
                if found.is_dir() && there.is_dir() && !found.is_link() && !there.is_link() {
                    walk.emptied.push((from.clone(), found));
                    let listed = list(&from, |name, child| {
                        let name = OsString::from_wide(name);
                        stack.push((from.join(&name), child, to.join(&name)));
                    });
                    if let Err(code) = listed {
                        self.transfer.failed(&from, message(code));
                    }
                    break;
                }
                if found.is_dir() || there.is_dir() {
                    self.transfer
                        .failed(&from, "a file and a folder cannot replace each other".to_owned());
                    break;
                }
                match self.transfer.ask(clash(&to, &found, &there)) {
                    None => return,
                    Some(Choice::Skip) => {
                        self.transfer.passed_one(found.size);
                        break;
                    }
                    Some(Choice::Replace) => {
                        writable(&to, there);
                        flags = MOVEFILE_REPLACE_EXISTING;
                    }
                    Some(Choice::KeepBoth) => to = free_name(&to, exists),
                }
            }
        }
    }

    // ---- The pool ------------------------------------------------------------

    fn copy_file(&self, task: Task) {
        let Task {
            from,
            mut to,
            found,
            mut record,
        } = task;
        if let Some(name) = from.file_name() {
            self.transfer.now_on(&name.to_string_lossy());
        }
        let mut flags = COPY_FILE_FAIL_IF_EXISTS;
        if found.is_link() {
            flags |= COPY_FILE_COPY_SYMLINK;
        } else if found.size >= UNBUFFERED {
            flags |= COPY_FILE_NO_BUFFERING;
        }
        let progress = Progress {
            transfer: self.transfer,
            reported: std::cell::Cell::new(0),
        };
        let landed = loop {
            match copy(&from, &to, flags, &progress) {
                Ok(()) => break true,
                Err(code) if code == ERROR_REQUEST_ABORTED => break false,
                Err(code) if code == ERROR_FILE_EXISTS || code == ERROR_ALREADY_EXISTS => {
                    let there = stat(&to).unwrap_or_default();
                    match self.transfer.ask(clash(&to, &found, &there)) {
                        None | Some(Choice::Skip) => break false,
                        Some(Choice::Replace) => {
                            writable(&to, there);
                            flags &= !COPY_FILE_FAIL_IF_EXISTS;
                            // A copy that replaced made nothing new: the name was there before,
                            // and undo recycling it would take away the file the user kept. A move
                            // still records, because putting the moved file back is an undo of it.
                            record &= self.moving;
                        }
                        Some(Choice::KeepBoth) => to = free_name(&to, exists),
                    }
                }
                // Asked about, as the shell asks, rather than written down as failed: both are
                // things the user can put right while the copy waits.
                Err(code) if in_use(code) => {
                    match snag(self.transfer, &from, Trouble::InUse, code) {
                        Some(Mend::TryAgain) => {}
                        _ => break false,
                    }
                }
                Err(code)
                    if code == ERROR_ENCRYPTION_FAILED
                        && flags & COPY_FILE_ALLOW_DECRYPTED_DESTINATION == 0 =>
                {
                    match snag(self.transfer, &from, Trouble::Encrypted, code) {
                        Some(Mend::Decrypt) => flags |= COPY_FILE_ALLOW_DECRYPTED_DESTINATION,
                        Some(Mend::TryAgain) => {}
                        _ => break false,
                    }
                }
                Err(code) => {
                    self.transfer.failed(&from, message(code));
                    break false;
                }
            }
        };
        // Whatever the callback did not see — a file too small for a chunk, or one skipped.
        let rest = found.size.saturating_sub(progress.reported.get());
        self.transfer.moved_bytes(rest);
        self.transfer.finished_one();
        if !landed {
            return;
        }
        self.transfer.wrote(rest);
        // Recorded below whatever happens here: the copy exists, and undo putting it back is what
        // would be asked.
        while self.moving && !remove(&from, found) {
            let code = unsafe { GetLastError() };
            if in_use(code) {
                match snag(self.transfer, &from, Trouble::InUse, code) {
                    Some(Mend::TryAgain) => continue,
                    // Skipped: the original stays, which is what the user asked for.
                    _ => break,
                }
            }
            self.transfer.failed(
                &from,
                format!("copied, but the original could not be removed: {}", message(code)),
            );
            break;
        }
        if record {
            self.record(&from, &to);
        }
    }

    // ---- Afterwards ----------------------------------------------------------

    fn settle(&self, walk: &Walk) {
        for (folder, found) in &walk.made {
            stamp(folder, found);
        }
        // Children first. A folder that still holds something — a file skipped, or one that
        // failed — stays, which is the point: nothing is removed that was not moved.
        for (folder, found) in walk.emptied.iter().rev() {
            remove(folder, *found);
        }
    }
}

/// What the conflict question shows about the two files.
fn clash(to: &Path, incoming: &Found, existing: &Found) -> Clash {
    Clash {
        name: to
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        into: to.parent().map(Path::to_path_buf).unwrap_or_default(),
        incoming: incoming.facts(),
        existing: existing.facts(),
    }
}

// ---------------------------------------------------------------------------
// Permanent delete
// ---------------------------------------------------------------------------

/// Shift+Delete: confirm, then list and delete in parallel, folders last. `None` for a job the
/// shell should have — the same top-level rules as a copy's, and the same elevation check.
fn erase(items: &[PathBuf], transfer: &Transfer) -> Option<Ran> {
    if !erasable(items) {
        return None;
    }
    let top = look(items)?;
    let parent = items[0].parent()?;
    if !crate::shell::over_network(parent) && !may_take_out(&top) {
        return None;
    }

    #[cfg(test)]
    crate::sandbox::guard("the fast delete", items);

    transfer.set_phase(Phase::Running);
    let confirm = Confirm {
        count: items.len(),
        name: (items.len() == 1).then(|| crate::fs::display_name(&items[0])),
        folders: top.iter().any(|(_, found)| found.is_dir()),
    };
    if !transfer.ask_confirm(confirm) {
        // A no is the job stopped before it started, which is what a cancel is: nothing done, and
        // nothing for Ctrl+Z either.
        transfer.cancel();
    } else {
        // Every folder the walk went into, parents before children — so backwards is the order
        // they can be removed in.
        let mut folders: Vec<(PathBuf, Found)> = Vec::new();
        pool(
            transfer,
            workers(items, parent),
            "fast-delete",
            |(path, found): (PathBuf, Found)| erase_one(transfer, &path, found),
            |tasks| {
                for (item, found) in &top {
                    if transfer.cancelled() {
                        break;
                    }
                    if found.is_dir() {
                        erase_walk(transfer, item, *found, tasks, &mut folders);
                    } else {
                        transfer.found(1, 0);
                        let _ = tasks.send(((*item).clone(), *found));
                    }
                }
            },
        );
        // Not after a cancel: what was asked to stop should stop, and a folder emptied by the part
        // that ran is no worse left standing.
        if !transfer.cancelled() {
            for (folder, found) in folders.iter().rev() {
                erase_one(transfer, folder, *found);
            }
        }
        tell_the_shell(items.iter().filter_map(|item| item.parent()));
    }
    let ran = transfer.ran(Outcome::default());
    transfer.set_phase(Phase::Finished);
    Some(ran)
}

/// List a folder to be deleted, all the way down: every file and every link onto the queue, every
/// folder into `folders` for the end. A link to a folder is queued as the entry it is and never
/// gone into, because what it points at is somewhere else.
fn erase_walk(
    transfer: &Transfer,
    root: &Path,
    found: Found,
    tasks: &Sender<(PathBuf, Found)>,
    folders: &mut Vec<(PathBuf, Found)>,
) {
    let mut stack = vec![(root.to_path_buf(), found)];
    while let Some((dir, found)) = stack.pop() {
        if transfer.cancelled() {
            return;
        }
        transfer.found(1, 0);
        let listed = list(&dir, |name, child| {
            let path = dir.join(OsString::from_wide(name));
            if child.is_dir() && !child.is_link() {
                stack.push((path, child));
            } else {
                transfer.found(1, 0);
                let _ = tasks.send((path, child));
            }
        });
        if let Err(code) = listed {
            transfer.failed(&dir, message(code));
        }
        folders.push((dir, found));
    }
}

/// Delete one entry: a file, a link, or a folder that should by now be empty.
///
/// A folder that is not empty is not an error here — something in it failed or was skipped, and
/// that has been said already. Something that is already gone is not one either.
fn erase_one(transfer: &Transfer, path: &Path, found: Found) {
    if let Some(name) = path.file_name() {
        transfer.now_on(&name.to_string_lossy());
    }
    while !remove(path, found) {
        let code = unsafe { GetLastError() };
        let gone = code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND;
        let holding = code == ERROR_DIR_NOT_EMPTY && found.is_dir() && !found.is_link();
        if gone || holding {
            break;
        }
        if in_use(code) && snag(transfer, path, Trouble::InUse, code) == Some(Mend::TryAgain) {
            continue;
        }
        if !in_use(code) {
            transfer.failed(path, message(code));
        }
        break;
    }
    transfer.finished_one();
}

// ---------------------------------------------------------------------------
// One entry
// ---------------------------------------------------------------------------

/// Another program has the file open in a way that keeps this one out.
fn in_use(code: u32) -> bool {
    code == ERROR_SHARING_VIOLATION || code == ERROR_LOCK_VIOLATION
}

/// Ask about a file that cannot be got at. See [`Trouble`].
fn snag(transfer: &Transfer, path: &Path, trouble: Trouble, code: u32) -> Option<Mend> {
    transfer.ask_snag(Snag {
        path: path.to_path_buf(),
        trouble,
        why: message(code),
    })
}

/// What `CopyFile2`'s callback is handed: where to count, and how much of this file it already has.
struct Progress<'a> {
    transfer: &'a Transfer,
    reported: std::cell::Cell<u64>,
}

/// Called by `CopyFile2` on the thread doing the copy, after every chunk.
///
/// **Pausing is waiting here.** Blocking in the callback holds the file where it is, so a pause
/// takes effect inside a 40 GB file rather than after it.
unsafe extern "system" fn progress(
    message: *const COPYFILE2_MESSAGE,
    context: *const c_void,
) -> COPYFILE2_MESSAGE_ACTION {
    // SAFETY: `context` is the `Progress` that [`copy`] passed in, which outlives the call; the
    // message is valid for the length of the callback.
    let (progress, message) = unsafe { (&*(context as *const Progress), &*message) };
    if message.Type == COPYFILE2_CALLBACK_CHUNK_FINISHED {
        let total = unsafe { message.Info.ChunkFinished.uliTotalBytesTransferred };
        let new = total.saturating_sub(progress.reported.replace(total));
        progress.transfer.moved_bytes(new);
        progress.transfer.wrote(new);
    }
    progress.transfer.hold();
    if progress.transfer.cancelled() {
        COPYFILE2_PROGRESS_CANCEL
    } else {
        COPYFILE2_PROGRESS_CONTINUE
    }
}

/// `CopyFile2`, answering the Win32 error when it fails.
fn copy(from: &Path, to: &Path, flags: u32, context: &Progress) -> Result<(), u32> {
    let params = COPYFILE2_EXTENDED_PARAMETERS {
        dwSize: std::mem::size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
        dwCopyFlags: flags,
        // Read by `CopyFile2` between its own steps, which is what makes a cancel reach a file that
        // is still being opened — the callback above only runs once bytes are moving.
        pfCancel: context.transfer.cancel_flag(),
        pProgressRoutine: Some(progress),
        pvCallbackContext: context as *const Progress as *mut c_void,
    };
    // SAFETY: both paths are null-terminated and outlive the call; `params` and the context it
    // points at are on this stack frame for all of it.
    let hr = unsafe { CopyFile2(verbatim(from).as_ptr(), verbatim(to).as_ptr(), &params) };
    if hr >= 0 {
        return Ok(());
    }
    let hr = hr as u32;
    // `HRESULT_FROM_WIN32`, unwrapped; anything else is passed on whole for the message.
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        Err(hr & 0xFFFF)
    } else {
        Err(hr)
    }
}

/// Remove a file, a folder that is empty, or a link as the link — which `RemoveDirectoryW` and
/// `DeleteFileW` both do, leaving what it points at alone. Read-only is cleared first if it has
/// to be, as Explorer's own delete and move do. The error is left in `GetLastError`.
fn remove(path: &Path, found: Found) -> bool {
    let wide = verbatim(path);
    let attempt = || unsafe {
        if found.is_dir() {
            RemoveDirectoryW(wide.as_ptr()) != 0
        } else {
            DeleteFileW(wide.as_ptr()) != 0
        }
    };
    if attempt() {
        return true;
    }
    if found.attributes & FILE_ATTRIBUTE_READONLY == 0 {
        return false;
    }
    unsafe { SetFileAttributesW(wide.as_ptr(), found.attributes & !FILE_ATTRIBUTE_READONLY) };
    attempt()
}

/// Clear read-only on a file about to be replaced, which neither `CopyFile2` nor `MoveFileExW` will
/// overwrite otherwise.
fn writable(path: &Path, there: Found) {
    if there.attributes & FILE_ATTRIBUTE_READONLY != 0 {
        let attributes = match there.attributes & !FILE_ATTRIBUTE_READONLY {
            0 => FILE_ATTRIBUTE_NORMAL,
            rest => rest,
        };
        unsafe { SetFileAttributesW(verbatim(path).as_ptr(), attributes) };
    }
}

/// Give a folder this program made the times of the one it was made from.
fn stamp(folder: &Path, found: &Found) {
    let Some(handle) = open(&verbatim(folder), FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS)
    else {
        return;
    };
    unsafe { SetFileTime(handle.0, &found.created, &found.accessed, &found.written) };
}

/// Tell every Explorer window — and the desktop, and the file dialogs — which folders changed.
///
/// `IFileOperation` does this as part of the job, and `CopyFile2` does not: a window showing the
/// destination would otherwise wait for the file system's own notification, which on a share is
/// late or never. One `SHCNE_UPDATEDIR` per folder, each once. Queued rather than flushed, so the
/// job's thread does not wait on the windows being told.
///
/// **Not from a test.** It is a broadcast to the user's own Explorer windows, which a test process
/// has no business making — the same reasoning as [`crate::shell::ops::FOR_REAL`].
fn tell_the_shell<'a>(folders: impl IntoIterator<Item = &'a Path>) {
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEDIR, SHCNF_PATHW};
    if cfg!(test) {
        return;
    }
    let mut told: Vec<String> = Vec::new();
    for folder in folders {
        let key = folder.as_os_str().to_string_lossy().to_lowercase();
        if told.contains(&key) {
            continue;
        }
        told.push(key);
        let wide = crate::shell::wide(folder);
        // SAFETY: a null-terminated path that outlives the call, which copies it.
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEDIR as i32,
                SHCNF_PATHW,
                wide.as_ptr() as *const c_void,
                std::ptr::null(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
/// `MAXIMUM_REPARSE_DATA_BUFFER_SIZE`.
const REPARSE_MAX: usize = 16 * 1024;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// Copy a junction or a directory link *as* one: a new empty folder carrying the same reparse data.
///
/// What `robocopy /SL` does. A junction made like this points where the original pointed, absolute
/// target and all — which is what a copy of a link is.
fn copy_link(from: &Path, to: &Path) -> Result<(), String> {
    use windows::Win32::System::IO::DeviceIoControl;

    let as_link = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
    let last = || message(unsafe { GetLastError() });

    let mut data = vec![0u8; REPARSE_MAX];
    let mut length = 0u32;
    let source = open(&verbatim(from), GENERIC_READ, as_link).ok_or_else(last)?;
    // SAFETY: an open handle; the buffer is as long as said.
    unsafe {
        DeviceIoControl(
            source.raw(),
            FSCTL_GET_REPARSE_POINT,
            None,
            0,
            Some(data.as_mut_ptr() as *mut c_void),
            data.len() as u32,
            Some(&mut length),
            None,
        )
    }
    .map_err(|e| e.message())?;
    drop(source);

    if unsafe { CreateDirectoryW(verbatim(to).as_ptr(), std::ptr::null()) } == 0 {
        let code = unsafe { GetLastError() };
        return Err(if code == ERROR_ALREADY_EXISTS {
            "something with that name is already there".to_owned()
        } else {
            message(code)
        });
    }
    let written = open(&verbatim(to), GENERIC_WRITE, as_link)
        .ok_or_else(last)
        .and_then(|target| {
            // SAFETY: as above.
            unsafe {
                DeviceIoControl(
                    target.raw(),
                    FSCTL_SET_REPARSE_POINT,
                    Some(data.as_ptr() as *const c_void),
                    length,
                    None,
                    0,
                    None,
                    None,
                )
            }
            .map_err(|e| e.message())
        });
    if written.is_err() {
        // An empty folder where a link should be is worse than nothing at all.
        unsafe { RemoveDirectoryW(verbatim(to).as_ptr()) };
    }
    written
}

// ---------------------------------------------------------------------------
// Looking
// ---------------------------------------------------------------------------

/// What the walk knows about an entry, from the directory read alone.
#[derive(Clone, Copy, Default)]
struct Found {
    attributes: u32,
    /// The reparse tag, when [`Self::attributes`] says there is one.
    tag: u32,
    size: u64,
    created: FILETIME,
    accessed: FILETIME,
    written: FILETIME,
}

impl Found {
    fn of(data: &WIN32_FIND_DATAW) -> Self {
        Self {
            attributes: data.dwFileAttributes,
            tag: data.dwReserved0,
            size: (data.nFileSizeHigh as u64) << 32 | data.nFileSizeLow as u64,
            created: data.ftCreationTime,
            accessed: data.ftLastAccessTime,
            written: data.ftLastWriteTime,
        }
    }

    fn is_dir(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    fn is_reparse(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    /// A link of some kind — a junction, a symbolic link — rather than a reparse point that stores
    /// the file itself, like a cloud placeholder or a deduplicated file.
    ///
    /// The name-surrogate bit of the tag is the documented test (`IsReparseTagNameSurrogate`), and
    /// it is what separates `IO_REPARSE_TAG_MOUNT_POINT` and `_SYMLINK` from `_CLOUD_*` and
    /// `_DEDUP`.
    fn is_link(&self) -> bool {
        self.is_reparse() && self.tag & 0x2000_0000 != 0
    }

    /// Somebody else's file standing in for the real one: reading it means a download.
    fn is_placeholder(&self) -> bool {
        let remote = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_OFFLINE;
        self.attributes & remote != 0
    }

    fn facts(&self) -> Facts {
        let written = &self.written;
        Facts {
            size: self.size,
            modified: (written.dwHighDateTime as u64) << 32 | written.dwLowDateTime as u64,
        }
    }
}

/// One item's own entry, by asking the find API for exactly its name.
///
/// The same answer the walk gets for everything below it — including the reparse tag, which
/// `GetFileAttributesExW` does not give.
fn stat(path: &Path) -> Option<Found> {
    let pattern = verbatim(path);
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: a null-terminated pattern, and a buffer of the type the info level writes.
    let handle = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            &mut data as *mut WIN32_FIND_DATAW as *mut c_void,
            FindExSearchNameMatch,
            std::ptr::null(),
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // A drive root is not an entry anyone lists, so ask for its attributes instead.
        let attributes = unsafe { GetFileAttributesW(pattern.as_ptr()) };
        return (attributes != INVALID_FILE_ATTRIBUTES).then(|| Found {
            attributes,
            ..Found::default()
        });
    }
    unsafe { FindClose(handle) };
    Some(Found::of(&data))
}

fn exists(path: &Path) -> bool {
    unsafe { GetFileAttributesW(verbatim(path).as_ptr()) != INVALID_FILE_ATTRIBUTES }
}

/// Every entry of a folder but `.` and `..`, by name. The error is the Win32 code.
fn list(dir: &Path, mut each: impl FnMut(&[u16], Found)) -> Result<(), u32> {
    let mut pattern = verbatim(dir);
    pattern.pop();
    if pattern.last() != Some(&(b'\\' as u16)) {
        pattern.push(b'\\' as u16);
    }
    pattern.extend([b'*' as u16, 0]);
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: as in [`stat`].
    let handle = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            &mut data as *mut WIN32_FIND_DATAW as *mut c_void,
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let code = unsafe { GetLastError() };
        return match code {
            ERROR_FILE_NOT_FOUND | ERROR_NO_MORE_FILES => Ok(()),
            code => Err(code),
        };
    }
    loop {
        let name = until_nul(&data.cFileName);
        let dot = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
        if !dot {
            each(name, Found::of(&data));
        }
        if unsafe { FindNextFileW(handle, &mut data) } == 0 {
            break;
        }
    }
    unsafe { FindClose(handle) };
    Ok(())
}

/// Whether the destination is one of the items under another name, or inside one, or the folder an
/// item is already in.
///
/// [`suits`] has asked the same of the paths as written; this asks the volume, through
/// `GetFinalPathNameByHandleW`, which resolves a `subst` drive, a junction and a mapped drive to
/// the one name each place really has. Folders only, and each parent once: a file cannot contain
/// the destination, and a selection of ten thousand files out of one folder is one open here.
fn aliased(top: &[(&PathBuf, Found)], into: &Path) -> bool {
    let Some(into) = real(into) else {
        return false;
    };
    let mut parents: Vec<&Path> = Vec::new();
    for (item, found) in top {
        if let Some(parent) = item.parent() {
            if !parents.contains(&parent) {
                parents.push(parent);
                if real(parent).as_deref() == Some(into.as_str()) {
                    return true;
                }
            }
        }
        if found.is_dir() {
            if let Some(item) = real(item) {
                if into == item || into.starts_with(&format!("{item}\\")) {
                    return true;
                }
            }
        }
    }
    false
}

/// The one name a folder or file really has: its volume's GUID path, or for a share the UNC path,
/// lowercased and without a trailing separator. `None` when it cannot be opened to ask.
fn real(path: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS, VOLUME_NAME_GUID,
    };
    let handle = open(&verbatim(path), 0, FILE_FLAG_BACKUP_SEMANTICS)?;
    let mut buffer = vec![0u16; 32_768];
    // The GUID first, because it is the one name a `subst` drive and its folder share; a share has
    // no volume GUID on this machine, and its UNC form is then the one to compare.
    [VOLUME_NAME_GUID, VOLUME_NAME_DOS].into_iter().find_map(|volume| {
        let len = unsafe {
            GetFinalPathNameByHandleW(
                handle.0,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                FILE_NAME_NORMALIZED | volume,
            )
        } as usize;
        (len != 0 && len < buffer.len()).then(|| {
            String::from_utf16_lossy(&buffer[..len])
                .trim_end_matches('\\')
                .to_lowercase()
        })
    })
}

// ---------------------------------------------------------------------------
// Asking the system
// ---------------------------------------------------------------------------

/// Whether this process may write into `into` — and, for a move, take the items out of the folders
/// they are in — without being elevated.
///
/// `AccessCheck` against the folder's own security descriptor, with this process's token: which,
/// under UAC, is the filtered one, so `Program Files` and the root of the system drive answer no
/// here exactly as they make the shell ask for elevation. A descriptor that cannot be read for any
/// reason but being refused is not a no: the copy then finds out for itself, file by file.
fn allowed(top: &[(&PathBuf, Found)], into: &Path, moving: bool) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY};
    may(into, FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY) && (!moving || may_take_out(top))
}

/// Whether this process may take each of these items out of the folder it is in, which a move and
/// a delete both have to.
///
/// Either of two grants allows it: `DELETE` on the item, or `FILE_DELETE_CHILD` on the folder. An
/// ordinary user folder grants only the first — *Modify* has no delete-child in it — so asking for
/// the second alone sent every move to the shell.
///
/// One item per folder is asked, not every item: a selection is ten thousand files out of one
/// folder as easily as one, and the items of a folder inherit the same grants nearly always. One
/// that does not fails by itself, and says so.
fn may_take_out(top: &[(&PathBuf, Found)]) -> bool {
    use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;
    const DELETE: u32 = 0x0001_0000;
    let mut parents: Vec<&Path> = Vec::new();
    for (item, _) in top {
        if let Some(parent) = item.parent() {
            if !parents.contains(&parent) {
                parents.push(parent);
                if !may(parent, FILE_DELETE_CHILD) && !may(item, DELETE) {
                    return false;
                }
            }
        }
    }
    true
}

/// Whether this process's token is granted `wanted` on `path`. See [`allowed`].
pub(crate) fn may(path: &Path, wanted: u32) -> bool {
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
    use windows_sys::Win32::Security::{
        AccessCheck, DuplicateToken, GetFileSecurityW, MapGenericMask, SecurityImpersonation,
        DACL_SECURITY_INFORMATION, GENERIC_MAPPING, GROUP_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, TOKEN_DUPLICATE, TOKEN_QUERY,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let wide = verbatim(path);
    let what = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    unsafe {
        let mut needed = 0u32;
        GetFileSecurityW(wide.as_ptr(), what, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            return GetLastError() != ERROR_ACCESS_DENIED;
        }
        // `u64`s, for the descriptor's alignment.
        let mut descriptor = vec![0u64; (needed as usize).div_ceil(8)];
        let descriptor_ptr = descriptor.as_mut_ptr() as *mut c_void;
        if GetFileSecurityW(wide.as_ptr(), what, descriptor_ptr, needed, &mut needed) == 0 {
            return GetLastError() != ERROR_ACCESS_DENIED;
        }

        let mut process: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut process) == 0 {
            return true;
        }
        let process = Handle(process);
        // `AccessCheck` wants an impersonation token, and a process's own is a primary one.
        let mut token: HANDLE = std::ptr::null_mut();
        if DuplicateToken(process.0, SecurityImpersonation, &mut token) == 0 {
            return true;
        }
        let token = Handle(token);
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        let mut wanted = wanted;
        MapGenericMask(&mut wanted, &mapping);
        // Room for the privileges the check may report having used; none are asked for.
        let mut privileges = [0u64; 32];
        let mut privileges_len = std::mem::size_of_val(&privileges) as u32;
        let mut granted = 0u32;
        let mut status = 0;
        let checked = AccessCheck(
            descriptor_ptr,
            token.0,
            wanted,
            &mapping,
            privileges.as_mut_ptr() as *mut _,
            &mut privileges_len,
            &mut granted,
            &mut status,
        ) != 0;
        !checked || status != 0
    }
}

/// What is free at `into` for this user, quotas included. `None` when it will not say.
fn free_space(into: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut free = 0u64;
    let asked = unsafe {
        GetDiskFreeSpaceExW(
            verbatim(into).as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (asked != 0).then_some(free)
}

/// The largest file the file system at `into` can hold, when that is a limit a copy could meet:
/// FAT and FAT32, at a byte short of 4 GB. exFAT and NTFS have none worth checking.
fn largest_file(into: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let root = volume_root(into)?;
    let mut name = [0u16; 64];
    let asked = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if asked == 0 {
        return None;
    }
    let name = String::from_utf16_lossy(until_nul(&name));
    matches!(name.as_str(), "FAT" | "FAT32").then_some(u32::MAX as u64)
}

/// How many workers this job gets. See [`OVER_NETWORK`].
fn workers(items: &[PathBuf], into: &Path) -> usize {
    let ends = items.iter().map(PathBuf::as_path).chain(std::iter::once(into));
    if ends.clone().any(crate::shell::over_network) {
        return OVER_NETWORK;
    }
    // Only two volumes are asked about however many items there are — the first item's and the
    // destination's — since a selection comes out of one folder nearly always.
    if seeks(&items[0]) || seeks(into) {
        return SPINNING;
    }
    SOLID_STATE
}

/// Whether the disk under `path` incurs a seek penalty: a spinning disk, by the drive's own
/// account.
///
/// `false` whenever the question cannot be asked — a volume mounted in a folder, a RAID controller
/// that will not say — which gives the solid-state count. Wrong for a spinning disk behind such a
/// controller, and the cost of that is a slower copy, never a wrong one.
fn seeks(path: &Path) -> bool {
    use windows::Win32::System::IO::DeviceIoControl;

    #[repr(C)]
    struct Query {
        property: u32,
        kind: u32,
        extra: [u8; 4],
    }
    #[repr(C)]
    #[derive(Default)]
    struct Penalty {
        version: u32,
        size: u32,
        incurs: u8,
    }
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;

    let Some(root) = volume_root(path) else {
        return false;
    };
    let root = String::from_utf16_lossy(until_nul(&root));
    let letter = root.trim_start_matches(r"\\?\").as_bytes();
    if letter.len() != 3 || letter[1] != b':' {
        return false;
    }
    let device: Vec<u16> = format!(r"\\.\{}:", letter[0] as char)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let Some(handle) = open(&device, 0, 0) else {
        return false;
    };
    let query = Query {
        property: STORAGE_DEVICE_SEEK_PENALTY_PROPERTY,
        kind: 0,
        extra: [0; 4],
    };
    let mut answer = Penalty::default();
    // SAFETY: an open handle, and two buffers of the sizes said.
    let asked = unsafe {
        DeviceIoControl(
            handle.raw(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const Query as *const c_void),
            std::mem::size_of::<Query>() as u32,
            Some(&mut answer as *mut Penalty as *mut c_void),
            std::mem::size_of::<Penalty>() as u32,
            None,
            None,
        )
    };
    asked.is_ok() && answer.incurs != 0
}

/// The root of the volume `path` is on, null-terminated, as `GetVolumePathNameW` gives it.
fn volume_root(path: &Path) -> Option<Vec<u16>> {
    let mut root = vec![0u16; 1024];
    let wide = verbatim(path);
    let asked = unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
    (asked != 0).then_some(root)
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// A handle that is closed when it goes.
struct Handle(HANDLE);

impl Handle {
    /// The same handle, as the `windows` crate's type, for `DeviceIoControl`.
    fn raw(&self) -> windows::Win32::Foundation::HANDLE {
        windows::Win32::Foundation::HANDLE(self.0)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Open a file, a folder or a device — given null-terminated — for `access`, sharing it every way
/// so that nothing else is kept out by this program looking.
fn open(wide: &[u16], access: u32, flags: u32) -> Option<Handle> {
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    (handle != INVALID_HANDLE_VALUE).then_some(Handle(handle))
}

/// A path in the `\\?\` form, null-terminated: no 260-character limit, and no Win32 parsing of
/// names that end in a dot or a space.
fn verbatim(path: &Path) -> Vec<u16> {
    let raw: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|c| if c == b'/' as u16 { b'\\' as u16 } else { c })
        .collect();
    let text = String::from_utf16_lossy(&raw);
    let mut out: Vec<u16> = Vec::with_capacity(raw.len() + 9);
    if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        out.extend_from_slice(&raw);
    } else if text.starts_with(r"\\") {
        out.extend(r"\\?\UNC".encode_utf16());
        out.extend_from_slice(&raw[1..]);
    } else if raw.len() >= 2 && raw[1] == b':' as u16 {
        out.extend(r"\\?\".encode_utf16());
        out.extend_from_slice(&raw);
    } else {
        out.extend_from_slice(&raw);
    }
    out.push(0);
    out
}

/// The part of a fixed-size wide buffer before its terminator.
fn until_nul(wide: &[u16]) -> &[u16] {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    &wide[..len]
}

/// A Win32 error as a sentence, without the `(os error 5)` the standard library adds.
fn message(code: u32) -> String {
    let text = std::io::Error::from_raw_os_error(code as i32).to_string();
    match text.rfind(" (os error") {
        Some(at) => text[..at].trim_end_matches('.').to_owned(),
        None => text,
    }
}
