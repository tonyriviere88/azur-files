//! `IFileOperation`: copy, move, delete, rename and new folder, with the shell's own
//! progress, conflict prompts, undo and Recycle Bin.
//!
//! The Windows half of [`crate::shell::ops`]. Re-implementing any of this would mean a file
//! manager whose Delete is *nearly* Explorer's, and nearly is the wrong target for something
//! that moves a user's files.
//!
//! # What the shell will and will not do about undo
//!
//! Every operation here is issued with `FOF_ALLOWUNDO | FOFX_ADDUNDORECORD`, which puts it in the
//! shell's *own* undo stack — the one Explorer's Ctrl+Z reads. That is worth having and it is not
//! reachable from here: **shell32 exports nothing that replays it.** There is no `SHUndo`, no
//! undo interface on `IFileOperation`, and nothing in the namespace that stands for the stack. So
//! the history is this program's — [`crate::shell::ops::history`] — and Windows' contribution is the
//! two things it does expose:
//!
//! | | |
//! | --- | --- |
//! | **saying what happened** | `IFileOperationProgressSink`, which reports each item's fate and the item it produced — see [`Sink`] |
//! | **performing the inverse** | `IFileOperation` again for a move or a rename back, and the binned item's own `undelete` verb for a delete — see [`crate::shell::ops::bin`] |

use super::*;
use crate::shell::ops::{Outcome, Recycled};

/// Do the work. Runs on its own thread, with its own apartment.
#[cfg(windows)]
pub(crate) fn run(job: &Job, owner: Owner) -> (Option<String>, Outcome) {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    // SAFETY: this thread exists for this call, initialises its own apartment, and
    // uninitialises it before returning. Nothing else here touches COM.
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return (
                Some("Could not talk to the shell".to_owned()),
                Outcome::default(),
            );
        }
        let recorded = Recorder::default();
        // Two jobs are not `IFileOperation` at all, and each says so where it is implemented:
        //
        // - a **restore**, because the Recycle Bin is a namespace and taking something out of it is
        //   a context-menu verb. It records nothing — an `undelete` reports only whether it ran,
        //   and nothing needs more, because an undo's own outcome is never put in the history.
        // - a **link**, because there is no create-shortcut operation to ask for. It records what
        //   it wrote, which is what makes an Alt-drag undoable like everything else.
        let error = match job {
            Job::Restore { items } => super::bin::restore(items, owner),
            Job::Link { items, into } => {
                let (error, made) = crate::shell::links::shortcuts_into(items, into);
                recorded.with(|outcome| outcome.created = made);
                error
            }
            _ => perform(job, owner, &recorded),
        };
        CoUninitialize();
        (error, recorded.take())
    }
}

/// Where [`Sink`] leaves what the shell told it, shared with the thread's own [`run`].
///
/// A `Mutex` because the sink is a COM object and the shell owns when it is called; in practice
/// every callback arrives on this thread, and a lock that is never contended costs nothing.
#[cfg(windows)]
#[derive(Clone, Default)]
struct Recorder(std::sync::Arc<std::sync::Mutex<Outcome>>);

#[cfg(windows)]
impl Recorder {
    /// Do something to what has been recorded so far.
    fn with(&self, edit: impl FnOnce(&mut Outcome)) {
        if let Ok(mut held) = self.0.lock() {
            edit(&mut held);
        }
    }

    /// Everything recorded, leaving the recorder empty.
    fn take(&self) -> Outcome {
        self.0
            .lock()
            .map(|mut held| std::mem::take(&mut *held))
            .unwrap_or_default()
    }
}

/// What the shell says it did with each item.
///
/// `IFileOperation` reports every item's fate to one of these, and **what it reports is not
/// derivable from what it was asked for** — the table on [`Outcome`] is the list of ways the two
/// come apart. So all ten `Post…` callbacks are implemented rather than the one, which is what
/// makes undo act on the file the shell actually produced instead of on the path this program
/// hoped for.
///
/// The `Pre…` callbacks and the timers are the progress a copy dialog would draw. The shell is
/// drawing its own, so there is nothing for this program to add and they are three lines each.
#[cfg(windows)]
#[windows::core::implement(windows::Win32::UI::Shell::IFileOperationProgressSink)]
struct Sink {
    /// Where what it hears goes.
    into: Recorder,
    /// Whether a delete in this operation is going to the Recycle Bin.
    ///
    /// The sink cannot tell: `PostDeleteItem` is the same callback either way. A permanent
    /// delete is the one operation with nothing to record, so this is how it records nothing —
    /// rather than recording a [`Recycled`] naming a bin item that was never made, which undo
    /// would then try and fail to find.
    to_bin: bool,
}

#[cfg(windows)]
impl windows::Win32::UI::Shell::IFileOperationProgressSink_Impl for Sink_Impl {
    /// A folder or file this program made. Its path *and* the name the shell settled on.
    ///
    /// `NewItem` is given the name to start from and the shell picks the first free one, so what
    /// appears may be `New folder (3)`; nothing on this side can know which without being told.
    fn PostNewItem(
        &self,
        _flags: u32,
        into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows_core::PCWSTR,
        _template: &windows_core::PCWSTR,
        _attributes: u32,
        result: windows_core::HRESULT,
        made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows_core::Result<()> {
        if result.is_err() {
            return Ok(());
        }
        if let Some(made) = landed(made, into, name) {
            self.into.with(|outcome| outcome.created.push(made));
        }
        Ok(())
    }

    /// A copy that arrived. Undone by recycling what arrived, so its path is what matters.
    fn PostCopyItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows_core::PCWSTR,
        result: windows_core::HRESULT,
        made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows_core::Result<()> {
        if result.is_err() {
            return Ok(());
        }
        if let Some(made) = landed(made, into, name) {
            self.into.with(|outcome| outcome.created.push(made));
        }
        Ok(())
    }

    /// A move that arrived: both ends, because undoing it means moving it back to the first.
    fn PostMoveItem(
        &self,
        _flags: u32,
        item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows_core::PCWSTR,
        result: windows_core::HRESULT,
        made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows_core::Result<()> {
        if result.is_err() {
            return Ok(());
        }
        if let (Some(was), Some(now)) = (path_of(item.as_ref()), landed(made, into, name)) {
            self.into.with(|outcome| outcome.moved.push((was, now)));
        }
        Ok(())
    }

    /// A rename, which is a move inside one folder and is recorded as one.
    fn PostRenameItem(
        &self,
        _flags: u32,
        item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows_core::PCWSTR,
        result: windows_core::HRESULT,
        made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows_core::Result<()> {
        if result.is_err() {
            return Ok(());
        }
        let Some(was) = path_of(item.as_ref()) else {
            return Ok(());
        };
        // The renamed item for preference; failing that, the new name in the folder it was
        // already in, which for a rename is where it still is.
        let now = path_of(made.as_ref()).or_else(|| {
            // Null when the shell has no name to give, exactly as in [`landed`], where a replace
            // walking one of these is what took the program down.
            if name.is_null() {
                return None;
            }
            // SAFETY: non-null, checked above, and a null-terminated string owned by the caller
            // for the length of the call.
            let name = unsafe { name.to_string() }.ok()?;
            Some(was.parent()?.join(name))
        });
        if let Some(now) = now.filter(|now| *now != was) {
            self.into.with(|outcome| outcome.moved.push((was, now)));
        }
        Ok(())
    }

    /// An item that has gone: where it was, and — when the shell says so — what it became in
    /// the Recycle Bin.
    ///
    /// Nothing is recorded for a permanent delete. There is no way back from one, so an entry
    /// in the history would be an offer this program cannot honour.
    fn PostDeleteItem(
        &self,
        _flags: u32,
        item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        result: windows_core::HRESULT,
        made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows_core::Result<()> {
        if result.is_err() || !self.to_bin {
            return Ok(());
        }
        if let Some(from) = path_of(item.as_ref()) {
            // **`psiNewlyCreated` here is the `$R…` file, not the bin's item**, and the two are
            // not interchangeable — see the header of [`crate::shell::ops::bin`], which is where
            // treating it as the latter went wrong. As a path it is exactly what is wanted: it
            // names one deleted item and no other. `None` is fine; it is documented as optional,
            // and `from` is enough to search by.
            let bin = path_of(made.as_ref());
            self.into
                .with(|outcome| outcome.recycled.push(Recycled { from, bin }));
        }
        Ok(())
    }

    // The rest of the interface. A copy dialog would use these to draw progress and per-item
    // state; the shell is drawing its own, so there is nothing for this program to add.
    fn StartOperations(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _result: windows_core::HRESULT) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _name: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _name: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _name: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreDeleteItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreNewItem(
        &self,
        _flags: u32,
        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _name: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _total: u32, _so_far: u32) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
}

/// Where an item ended up: what the shell handed over, or the folder and name it was given.
///
/// The item is the answer used, and the composition is the fallback for when it is absent —
/// `psiNewlyCreated` was present on every callback measured by
/// `the_sink_reports_what_the_shell_actually_did`, and it is documented as optional.
///
/// **Both of them can be absent at once, and it took a crash to establish that.** Replacing a
/// file on a network share faulted at `ucrtbase!wcslen`, reached from `PostCopyItem` through
/// here: `psiNewlyCreated` yielded no path *and* `psznewname` was null, so the fallback ran and
/// walked a null pointer. `PCWSTR::to_string` is `wcslen` on whatever it is handed and there is
/// no check inside it — the null is this function's to catch. A replace produces no *new* item,
/// which is the likely reason the shell has neither to give: there was already a file there and
/// afterwards there still is, the same one, with different contents.
///
/// So a null name is `None`, an answer this sink already has a meaning for: the item is left out
/// of the [`Outcome`], and undo declines to offer a step rather than acting on a path nobody
/// confirmed. That is the same call the `result.is_err()` arm makes at every callback.
///
/// The fallback is otherwise *nearly* as good as the item, which was a surprise worth writing
/// down: `psznewname` turns out to be the name the shell settled on and not the one it was asked
/// for — measured as `one - Copie.txt` for a copy into its own folder, and `made (2)` for the
/// second `New folder`. Nearly, because there is nothing in the contract that says so, and the
/// one thing undo cannot afford is a path that names the wrong file.
#[cfg(windows)]
pub(crate) fn landed(
    made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    name: &windows_core::PCWSTR,
) -> Option<PathBuf> {
    if let Some(path) = path_of(made.as_ref()) {
        return Some(path);
    }
    let folder = path_of(into.as_ref())?;
    if name.is_null() {
        return None;
    }
    // SAFETY: non-null, checked above, and a null-terminated string owned by the caller for the
    // length of the call.
    let name = unsafe { name.to_string() }.ok()?;
    (!name.is_empty()).then(|| folder.join(name))
}

/// An item's path on disk, if it has one.
///
/// `SIGDN_FILESYSPATH` and not the display name: the display name of `one.txt` is `one` on a
/// machine with known extensions hidden, and a path is being asked for rather than a label.
#[cfg(windows)]
fn path_of(item: Option<&windows::Win32::UI::Shell::IShellItem>) -> Option<PathBuf> {
    let item = item?;
    // SAFETY: an item the shell has just handed over, and the string it returns is
    // freed here — `GetDisplayName` allocates with the task allocator.
    unsafe {
        let wide = item
            .GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH)
            .ok()?;
        // An `S_OK` with nothing at the pointer, which `shell::clipboard` guards the same way.
        // Not seen from this call; the reason it is checked anyway is that the alternative,
        // measured in `landed`, is `wcslen` walking address zero and taking the process with it.
        if wide.is_null() {
            return None;
        }
        let text = wide.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(wide.0 as *const std::ffi::c_void));
        text.map(PathBuf::from)
    }
}

/// The body, split out so the apartment is torn down on every path out.
#[cfg(windows)]
unsafe fn perform(job: &Job, owner: Owner, recorded: &Recorder) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem, FILEOPERATION_FLAGS,
        FOFX_ADDUNDORECORD, FOFX_RECYCLEONDELETE, FOFX_SHOWELEVATIONPROMPT, FOF_ALLOWUNDO,
        FOF_NOCONFIRMMKDIR, FOF_RENAMEONCOLLISION,
    };

    let op: IFileOperation = match CoCreateInstance(&FileOperation, None, CLSCTX_ALL) {
        Ok(op) => op,
        Err(e) => return friendly(&e),
    };

    // Parented to this window, or the progress dialog comes up behind it and the
    // application looks hung while it waits for an answer nobody can see.
    if owner.0 != 0 {
        let _ = op.SetOwnerWindow(owner.hwnd());
    }

    // One sink for the whole operation rather than one per item, which is the difference between
    // `Advise` and the last argument of `CopyItem`: this way an operation that queues a hundred
    // items has one place its answers arrive, and a job added below cannot forget to pass it.
    // What lands there is what undo works from — see [`Sink`]. Measured to hear about `NewItem`
    // as well, which is why that call now passes `None` where it used to pass a sink of its own.
    let sink: IFileOperationProgressSink = Sink {
        into: recorded.clone(),
        to_bin: matches!(job, Job::Delete { to_bin: true, .. }),
    }
    .into();
    let listening = op.Advise(&sink).ok();

    // `ALLOWUNDO` with `ADDUNDORECORD` puts the operation in the shell's undo stack,
    // so Ctrl+Z in Explorer takes it back. `RECYCLEONDELETE` is the difference between
    // Delete and Shift+Delete.
    //
    // **Nothing is added for `PutBack`**, and that is a decision rather than an omission: it is an
    // undo, so it takes the defaults. In particular not `FOF_RENAMEONCOLLISION` — a put-back that
    // finds something already sitting on the name it wants has hit a real question, because the
    // file it is putting back and the file in its place are two different files. Renaming would
    // answer it by inventing `one - Copy.txt`, which is the one answer nobody asked for; the
    // conflict dialog is the user's to see.
    let mut flags = FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0 | FOFX_SHOWELEVATIONPROMPT.0;
    match job {
        Job::Delete { to_bin: false, .. } => flags &= !FOF_ALLOWUNDO.0,
        Job::Delete { .. } => flags |= FOFX_RECYCLEONDELETE.0,
        Job::NewFolder { .. } => flags |= FOF_NOCONFIRMMKDIR.0,
        // Copying something into the folder it is already in cannot mean "replace it with
        // itself", so Explorer does not ask: it makes `one - Copy.txt` and gets on with it.
        // `FOF_RENAMEONCOLLISION` is what produces that name — the shell's own, localised,
        // measured to come out as `one - Copie.txt` on this machine — and without it a paste
        // into the current folder stops on the Replace-or-Skip dialog, which is not what
        // Ctrl+C Ctrl+V does anywhere else in Windows.
        //
        // Only when *every* item is already there. A batch from more than one folder that
        // happens to include one of the destination's own is a genuine name clash for the
        // others, and those are the user's to answer.
        Job::Copy { items, into } if all_already_in(items, into) => {
            flags |= FOF_RENAMEONCOLLISION.0
        }
        _ => {}
    }
    if let Err(e) = op.SetOperationFlags(FILEOPERATION_FLAGS(flags)) {
        return friendly(&e);
    }

    let queued = match job {
        Job::Copy { items, into } => {
            let dest: IShellItem = match item(into) {
                Ok(dest) => dest,
                Err(e) => return friendly(&e),
            };
            queue(items, |src| op.CopyItem(src, &dest, PCWSTR::null(), None))
        }
        Job::Move { items, into } => {
            let dest: IShellItem = match item(into) {
                Ok(dest) => dest,
                Err(e) => return friendly(&e),
            };
            queue(items, |src| op.MoveItem(src, &dest, PCWSTR::null(), None))
        }
        Job::Delete { items, .. } => queue(items, |src| op.DeleteItem(src, None)),
        Job::Rename { item: path, name } => {
            let src: IShellItem = match item(path) {
                Ok(src) => src,
                Err(e) => return friendly(&e),
            };
            match op.RenameItem(&src, &HSTRING::from(name.as_str()), None) {
                Ok(()) => 1,
                Err(e) => return friendly(&e),
            }
        }
        Job::NewFolder { parent, name } => {
            let dest: IShellItem = match item(parent) {
                Ok(dest) => dest,
                Err(e) => return friendly(&e),
            };
            // `None`: the sink registered with `Advise` above hears about this too, which
            // `the_sink_reports_what_the_shell_actually_did` is what establishes.
            match op.NewItem(
                &dest,
                FILE_ATTRIBUTE_DIRECTORY.0,
                &HSTRING::from(name.as_str()),
                PCWSTR::null(),
                None,
            ) {
                Ok(()) => 1,
                Err(e) => return friendly(&e),
            }
        }
        // An undo of a move or of a rename. One item at a time, because each has a destination of
        // its own — that is what `PutBack` carries pairs for.
        Job::PutBack { items } => {
            let mut queued = 0;
            for (now, back) in items {
                let (Some(parent), Some(name)) = (back.parent(), back.file_name()) else {
                    continue;
                };
                let name = HSTRING::from(name.to_string_lossy().as_ref());
                let Ok(src) = item(now) else { continue };
                // A pair whose two ends share a folder is a **rename**, not a move into the folder
                // it is already in. The shell would very likely do the right thing with the
                // latter; "very likely" is not what an undo of F2 should rest on, and
                // `RenameItem` is unambiguous.
                let asked = if now.parent() == Some(parent) {
                    op.RenameItem(&src, &name, None)
                } else {
                    match item(parent) {
                        Ok(into) => op.MoveItem(&src, &into, &name, None),
                        Err(_) => continue,
                    }
                };
                if asked.is_ok() {
                    queued += 1;
                }
            }
            queued
        }
        // Unreachable: [`run`] sends both of these elsewhere, because neither is something
        // `IFileOperation` can be asked for — see the note there. Here so the match stays
        // exhaustive, and returning rather than falling through to the count, because the message
        // below would blame missing items for what would actually be a wrong turn in `run`. A
        // plausible error is worse than an impossible one: the plausible one gets believed.
        Job::Restore { .. } | Job::Link { .. } => {
            return Some(format!(
                "{} does not go through IFileOperation",
                job.describe().trim_end_matches('…')
            ))
        }
    };

    if queued == 0 {
        return Some("None of those items could be found".to_owned());
    }

    // Blocks on *this* thread while the shell shows its progress, asks about conflicts
    // and does the work.
    let outcome = op.PerformOperations();
    // Said explicitly rather than left to the drop. Releasing `op` releases the sink with it, so
    // nothing leaks either way — but the sink writes into a `Recorder` this thread is about to
    // read, and "the shell has stopped calling it" is worth being a statement rather than a
    // consequence.
    if let Some(cookie) = listening {
        let _ = op.Unadvise(cookie);
    }
    if let Err(e) = outcome {
        return friendly(&e);
    }
    None
}

/// Whether every one of these items already lives in `into`.
///
/// Compared case-insensitively, because Windows paths are, and a copy of `C:\\Temp\\a.txt`
/// into `C:\\temp` is the same copy-into-its-own-folder as any other.
pub(crate) fn all_already_in(items: &[PathBuf], into: &Path) -> bool {
    let same = |a: &Path, b: &Path| {
        a.as_os_str()
            .to_string_lossy()
            .replace('/', "\\")
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy().replace('/', "\\"))
    };
    !items.is_empty() && items.iter().all(|i| i.parent().is_some_and(|p| same(p, into)))
}

/// Add every item to the operation, reporting how many were accepted.
///
/// An item that has gone missing between the click and here is skipped rather than
/// failing the batch, which is what happens when a selection is a few seconds stale.
#[cfg(windows)]
unsafe fn queue(
    items: &[PathBuf],
    mut add: impl FnMut(&windows::Win32::UI::Shell::IShellItem) -> windows::core::Result<()>,
) -> usize {
    let mut queued = 0;
    for path in items {
        if let Ok(shell_item) = item(path) {
            if add(&shell_item).is_ok() {
                queued += 1;
            }
        }
    }
    queued
}

/// A path as an `IShellItem`.
#[cfg(windows)]
pub(crate) unsafe fn item(
    path: &Path,
) -> windows::core::Result<windows::Win32::UI::Shell::IShellItem> {
    use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
    let wide = crate::shell::wide(path);
    SHCreateItemFromParsingName(windows::core::PCWSTR(wide.as_ptr()), None)
}

/// Turn an `HRESULT` into something worth showing, or `None` when it is not worth
/// showing at all.
///
/// The shell's own dialog has already explained anything the user can act on, and a
/// cancel is a decision rather than a fault — so both come back as `None` and the
/// status line stays quiet.
#[cfg(windows)]
pub(super) fn friendly(error: &windows::core::Error) -> Option<String> {
    const E_ACCESSDENIED: i32 = -2147024891; // 0x80070005
    const ERROR_CANCELLED: i32 = -2147023673; // 0x800704C7
    const COPYENGINE_E_USER_CANCELLED: i32 = -2144927744; // 0x80270000
    match error.code().0 {
        ERROR_CANCELLED | COPYENGINE_E_USER_CANCELLED => None,
        E_ACCESSDENIED => Some("Access denied".to_owned()),
        code => {
            let message = error.message();
            Some(if message.is_empty() {
                format!("The shell refused: 0x{code:08x}")
            } else {
                message
            })
        }
    }
}
