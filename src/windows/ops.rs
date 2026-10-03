//! `IFileOperation`: copy, move, delete, rename and new folder, with the shell's own
//! progress, conflict prompts, undo and Recycle Bin.
//!
//! The Windows half of [`crate::shell::ops`]. Re-implementing any of this would mean a file
//! manager whose Delete is *nearly* Explorer's, and nearly is the wrong target for something
//! that moves a user's files.

use super::*;

/// Do the work. Runs on its own thread, with its own apartment.
#[cfg(windows)]
pub(crate) fn run(job: &Job, owner: Owner) -> (Option<String>, Option<String>) {
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
