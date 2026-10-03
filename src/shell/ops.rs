//! Copy, move, delete, rename and new-folder — through `IFileOperation`.
//!
//! This is the shell's own engine, which is not a detail. Going through it means this
//! program gets, for free and *correctly*:
//!
//! - the **Recycle Bin**, and with it undo — a delete that cannot be undone is a
//!   different feature from the one users expect Delete to be;
//! - the **progress dialog**, with its estimate, its pause and its cancel;
//! - the **conflict prompts** — "there is already a file with this name", with the
//!   comparison of both and the keep-both option;
//! - the **elevation prompt**, when a destination needs administrator rights;
//! - **long path, junction, reparse point and cloud placeholder** handling, none of
//!   which a hand-rolled recursive copy gets right;
//! - correct behaviour on a collision that differs only in case, on a file in use, and
//!   on a path over 260 characters.
//!
//! Re-implementing that over `std::fs` would produce something that looks the same in
//! the easy cases and quietly loses data in the hard ones.
//!
//! What the shell puts on screen, and when, is worth knowing rather than assuming —
//! `what_the_shell_puts_on_screen` lists it:
//!
//! | | |
//! | --- | --- |
//! | a copy onto an existing name | `Replace or Skip Files`, in an `OperationStatusWindow` |
//! | Shift+Delete | `Delete File`, a plain `#32770` confirmation |
//! | Delete to the Recycle Bin | nothing, which is Windows’ own default |
//! | a copy into the folder it is already in | nothing, and a `one - Copy.txt` appears |
//!
//! The last row is the one that needed doing. Copying something into the folder it already
//! lives in cannot mean “replace it with itself”, so Explorer does not ask: it makes
//! `one - Copy.txt` and gets on with it. Without `FOF_RENAMEONCOLLISION` the same paste stopped
//! on the Replace-or-Skip dialog, which is not what Ctrl+C Ctrl+V does anywhere else in
//! Windows. The name is the shell’s own and localised — measured as `one - Copie.txt` here.
//!
//! # Why each operation gets its own thread
//!
//! `PerformOperations` is synchronous: it shows the progress dialog and does not
//! return until the work is done or cancelled. Called on the UI thread it would freeze
//! this window for the duration — so each operation runs on a thread of its own, which
//! initialises COM as its own apartment and pumps the dialog there. The window stays
//! live, the dialog is parented to it, and the affected folders are re-read when the
//! thread reports back.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::Owner;

/// What an operation did, once it has finished.
#[derive(Clone, Debug)]
pub struct Done {
    /// The folders whose contents may have changed, so they can be re-read.
    pub touched: Vec<PathBuf>,
    /// Empty when it worked — including when the user cancelled, which is an answer
    /// rather than a failure.
    pub error: Option<String>,
    /// What still has to happen now that it has.
    pub after: After,
    /// The name the shell gave a newly created folder.
    ///
    /// Asked for rather than guessed. `NewItem` is given the name to *start* from and the shell
    /// picks the first free one, so what actually appears may be `New folder (3)` — and
    /// nothing on this side can know which without being told. `IFileOperationProgressSink`
    /// is how it is told.
    pub created: Option<String>,
}

/// What is left to do once an operation finishes, beyond re-reading the folders.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum After {
    #[default]
    Nothing,
    /// This was a paste of a cut: if it worked, the clipboard has to be told so, and then
    /// emptied. Carried through the operation rather than done when it starts, because a
    /// paste the user cancels at the conflict dialog has to leave the cut where it was. The
    /// number is the clipboard's sequence when the paste began, so a clipboard that has since
    /// changed is left alone.
    FinishCut(u32),
    /// A folder was created: select it in this pane and open its name for editing, once the
    /// re-read brings it in. Which name that is comes back on [`Done::created`].
    NameIt(crate::pane::PaneId),
}

/// The work to do.
#[derive(Clone, Debug)]
pub enum Job {
    Copy { items: Vec<PathBuf>, into: PathBuf },
    Move { items: Vec<PathBuf>, into: PathBuf },
    /// `to_bin` sends them to the Recycle Bin, which is what Delete does; `false` is
    /// Shift+Delete.
    Delete { items: Vec<PathBuf>, to_bin: bool },
    Rename { item: PathBuf, name: String },
    NewFolder { parent: PathBuf, name: String },
}

impl Job {
    /// A present-tense description, for the status line while it runs.
    pub fn describe(&self) -> String {
        let count = |items: &Vec<PathBuf>| {
            if items.len() == 1 {
                "1 item".to_owned()
            } else {
                format!("{} items", items.len())
            }
        };
        match self {
            Self::Copy { items, .. } => format!("Copying {}…", count(items)),
            Self::Move { items, .. } => format!("Moving {}…", count(items)),
            Self::Delete {
                items,
                to_bin: true,
            } => format!("Recycling {}…", count(items)),
            Self::Delete { items, .. } => format!("Deleting {}…", count(items)),
            Self::Rename { .. } => "Renaming…".to_owned(),
            Self::NewFolder { .. } => "Creating a folder…".to_owned(),
        }
    }

    /// The folders this will change, so they can be re-read afterwards.
    pub fn touches(&self) -> Vec<PathBuf> {
        let parents = |items: &Vec<PathBuf>| -> Vec<PathBuf> {
            items
                .iter()
                .filter_map(|p| p.parent().map(Path::to_path_buf))
                .collect()
        };
        let mut touched = match self {
            Self::Copy { items, into } | Self::Move { items, into } => {
                let mut all = parents(items);
                all.push(into.clone());
                all
            }
            Self::Delete { items, .. } => parents(items),
            Self::Rename { item, .. } => item.parent().map(Path::to_path_buf).into_iter().collect(),
            Self::NewFolder { parent, .. } => vec![parent.clone()],
        };
        touched.sort();
        touched.dedup();
        touched
    }
}

/// Whether a test is allowed to hand a job to the real shell. **Off by default.**
///
/// This is not caution, it is a bug that was shipped and found — the same one, in the same shape,
/// as the guard on [`crate::config::Config::save`]. `the_clipboard_events_map_to_the_right_actions`
/// selects the first row of the harness's folder, which is `CARGO_MANIFEST_DIR`, and feeds the
/// window a Shift+Delete to check that the event maps to `Action::Delete` rather than to a cut.
/// The frame it does that in *applies* the action, so a green test quietly asked `IFileOperation`
/// to **permanently delete `.cargo` from this repository** and put the shell's confirmation dialog
/// up to ask about it. It ran on its own thread, so the test finished and passed while the prompt
/// was still on screen; whether the folder survived came down to a human not clicking Yes.
///
/// A test process has no business asking the shell to move or delete a user's files. The one test
/// that must — `copy_cut_paste_and_delete_end_to_end`, which works inside `target/sandbox` — turns
/// this on for its duration through [`for_real`].
#[cfg(test)]
pub(crate) static FOR_REAL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Let the real shell do the work for as long as the returned guard is alive.
///
/// A guard rather than a pair of calls, so a test that fails an assertion half way through still
/// leaves the flag off for whatever runs next.
#[cfg(test)]
pub(crate) fn for_real() -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            FOR_REAL.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    FOR_REAL.store(true, std::sync::atomic::Ordering::SeqCst);
    Guard
}

/// Runs jobs, one thread each, and reports back.
pub struct Operations {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// What is running, oldest first, for the status line.
    running: Vec<String>,
}

impl Operations {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            running: Vec::new(),
        }
    }

    /// Start a job. Returns immediately; the window stays live while it runs.
    pub fn start(&mut self, job: Job, owner: Owner, ctx: &egui::Context) {
        self.start_then(job, After::Nothing, owner, ctx);
    }

    /// The same, with something to do once it has finished.
    pub fn start_then(&mut self, job: Job, after: After, owner: Owner, ctx: &egui::Context) {
        self.running.push(job.describe());
        let touched = job.touches();
        let tx = self.tx.clone();
        let ctx = ctx.clone();

        // **Not from a test, unless a test asked for it.** See [`FOR_REAL`]. This is the choke
        // point every copy, move, delete, rename and new folder goes through, which is why the
        // guard is here and not at the five call sites.
        #[cfg(test)]
        if !FOR_REAL.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = tx.send(Done {
                touched,
                error: None,
                after,
                created: None,
            });
            return;
        }

        // Taken here and dropped on the job's own thread, so that a directory a drop claimed
        // into goes when the job that consumes it is done — and goes there rather than on the
        // UI thread, because removing a staged archive is real work. Built after the guard
        // above so a test can never make one.
        let scratch = claimed(&job).map(Scratch);
        let spawned = std::thread::Builder::new()
            .name("file-operation".to_owned())
            .spawn(move || {
                let _scratch = scratch;
                let (error, created) = run(&job, owner);
                let _ = tx.send(Done {
                    touched,
                    error,
                    after,
                    created,
                });
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.running.pop();
            let _ = self.tx.send(Done {
                touched: Vec::new(),
                error: Some("Could not start the operation".to_owned()),
                after: After::Nothing,
                created: None,
            });
        }
    }

    /// Anything that has finished since the last frame.
    pub fn drain(&mut self) -> Vec<Done> {
        let finished: Vec<Done> = self.rx.try_iter().collect();
        for _ in 0..finished.len() {
            if !self.running.is_empty() {
                self.running.remove(0);
            }
        }
        finished
    }

    /// What is in progress, for the status line.
    pub fn in_progress(&self) -> Option<&str> {
        self.running.first().map(String::as_str)
    }
}

impl Default for Operations {
    fn default() -> Self {
        Self::new()
    }
}

/// The staging directory a drop claimed its items into, if that is where they live.
///
/// Recognised from the path rather than carried down from the drop, so that every route which
/// ends in a job gets the cleanup without knowing about it — including the right-button menu,
/// which can sit open for as long as the user likes before it becomes one. What makes that safe
/// to act on is [`crate::shell::dnd::is_staging`], which asks both for the name and for the
/// directory to be sitting in `%TEMP%`.
fn claimed(job: &Job) -> Option<PathBuf> {
    let items = match job {
        Job::Copy { items, .. } | Job::Move { items, .. } => items,
        _ => return None,
    };
    let dir = items.first()?.parent()?;
    if crate::shell::dnd::is_staging(dir) {
        return Some(dir.to_path_buf());
    }
    // One level up as well, for an item that had to be nested to keep its name.
    let up = dir.parent()?;
    crate::shell::dnd::is_staging(up).then(|| up.to_path_buf())
}

/// A directory that goes when this does.
///
/// A guard rather than a statement so that a panic in the job, or a thread that never starts,
/// cannot leak an extracted archive into `%TEMP%`.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Do the work. Runs on its own thread, with its own apartment.
#[cfg(windows)]
fn run(job: &Job, owner: Owner) -> (Option<String>, Option<String>) {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    // SAFETY: this thread exists for this call, initialises its own apartment, and
    // uninitialises it before returning. Nothing else here touches COM.
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return (Some("Could not talk to the shell".to_owned()), None);
        }
        let created: NewName = Default::default();
        let result = perform(job, owner, &created);
        let name = created.lock().ok().and_then(|held| held.clone());
        CoUninitialize();
        (result, name)
    }
}

/// Where [`Sink`] leaves the name the shell chose.
#[cfg(windows)]
type NewName = std::sync::Arc<std::sync::Mutex<Option<String>>>;

/// Catches the name the shell gives a new folder.
///
/// `IFileOperation` takes one of these per item and reports what it did with it. Only
/// `PostNewItem` is of any interest here — the rest are the progress and per-item callbacks a
/// copy dialog would use, and the shell is showing its own. They are here because the interface
/// requires them, and they are all the same three lines.
#[cfg(windows)]
#[windows::core::implement(windows::Win32::UI::Shell::IFileOperationProgressSink)]
struct Sink(NewName);

#[cfg(windows)]
impl windows::Win32::UI::Shell::IFileOperationProgressSink_Impl for Sink_Impl {
    /// The one that matters: the name the shell settled on, and the item it made.
    fn PostNewItem(
        &self,
        _flags: u32,
        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows_core::PCWSTR,
        _template: &windows_core::PCWSTR,
        _attributes: u32,
        result: windows_core::HRESULT,
        item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows_core::Result<()> {
        if result.is_err() {
            return Ok(());
        }
        // The item's own name for preference, since it is what the folder now holds; the
        // `psznewname` the shell passes alongside is the same string in every case seen, and is
        // the fallback for a shell that hands over an item this cannot name.
        let settled = item
            .as_ref()
            .and_then(|item| {
                // SAFETY: a display name read from an item the shell has just handed over.
                unsafe {
                    item.GetDisplayName(windows::Win32::UI::Shell::SIGDN_PARENTRELATIVEPARSING)
                        .ok()
                        .and_then(|wide| {
                            let text = wide.to_string().ok();
                            windows::Win32::System::Com::CoTaskMemFree(Some(
                                wide.0 as *const std::ffi::c_void,
                            ));
                            text
                        })
                }
            })
            // SAFETY: a null-terminated string owned by the caller for the length of the call.
            .or_else(|| unsafe { name.to_string().ok() });

        if let (Some(settled), Ok(mut held)) = (settled, self.0.lock()) {
            *held = Some(settled);
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
    fn PostRenameItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _name: &windows_core::PCWSTR,
        _result: windows_core::HRESULT,
        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
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
    fn PostMoveItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _name: &windows_core::PCWSTR,
        _result: windows_core::HRESULT,
        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
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
    fn PostCopyItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _name: &windows_core::PCWSTR,
        _result: windows_core::HRESULT,
        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
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
    fn PostDeleteItem(
        &self,
        _flags: u32,
        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _result: windows_core::HRESULT,
        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
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

/// The body, split out so the apartment is torn down on every path out.
#[cfg(windows)]
unsafe fn perform(job: &Job, owner: Owner, created: &NewName) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IShellItem, FILEOPERATION_FLAGS, FOFX_ADDUNDORECORD,
        FOFX_RECYCLEONDELETE, FOFX_SHOWELEVATIONPROMPT, FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR,
        FOF_RENAMEONCOLLISION,
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

    // `ALLOWUNDO` with `ADDUNDORECORD` puts the operation in the shell's undo stack,
    // so Ctrl+Z in Explorer takes it back. `RECYCLEONDELETE` is the difference between
    // Delete and Shift+Delete.
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
            // The sink is how the real name comes back; see `Sink`.
            let sink: windows::Win32::UI::Shell::IFileOperationProgressSink =
                Sink(created.clone()).into();
            match op.NewItem(
                &dest,
                FILE_ATTRIBUTE_DIRECTORY.0,
                &HSTRING::from(name.as_str()),
                PCWSTR::null(),
                Some(&sink),
            ) {
                Ok(()) => 1,
                Err(e) => return friendly(&e),
            }
        }
    };

    if queued == 0 {
        return Some("None of those items could be found".to_owned());
    }

    // Blocks on *this* thread while the shell shows its progress, asks about conflicts
    // and does the work.
    if let Err(e) = op.PerformOperations() {
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
    let wide = super::wide(path);
    SHCreateItemFromParsingName(windows::core::PCWSTR(wide.as_ptr()), None)
}

/// Turn an `HRESULT` into something worth showing, or `None` when it is not worth
/// showing at all.
///
/// The shell's own dialog has already explained anything the user can act on, and a
/// cancel is a decision rather than a fault — so both come back as `None` and the
/// status line stays quiet.
#[cfg(windows)]
fn friendly(error: &windows::core::Error) -> Option<String> {
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

#[cfg(not(windows))]
fn run(_job: &Job, _owner: Owner) -> (Option<String>, Option<String>) {
    (
        Some("File operations are implemented against the Windows shell only".to_owned()),
        None,
    )
}

#[cfg(test)]
mod tests {
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
            let mut root = std::env::temp_dir();
            root.push(format!("yafe-guard-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
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
            let _ = std::fs::remove_dir_all(&root);
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

        let mut root = std::env::temp_dir();
        root.push(format!("yafe-ops-{}", std::process::id()));
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

        let _ = std::fs::remove_dir_all(&root);
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
        let _ = std::fs::remove_dir_all(&root);
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

        let _ = std::fs::remove_dir_all(&root);
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

        let _ = std::fs::remove_dir_all(&root);
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

        let _ = std::fs::remove_dir_all(&root);
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
        let _ = std::fs::remove_dir_all(&root);
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
}
