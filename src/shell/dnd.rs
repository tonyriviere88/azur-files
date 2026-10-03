//! Drag and drop, both ways, through OLE.
//!
//! The same mechanism Explorer uses, which is what makes it interoperate: files
//! dragged out of this window land in Explorer, in an archiver, in an editor's tab
//! bar, in an upload field; files dragged in from any of those arrive here. Nothing
//! about it is specific to this program.
//!
//! # Dragging out
//!
//! [`drag_out`] builds the shell's own data object for the selection and hands it to
//! `DoDragDrop`, with a tiny `IDropSource` to answer the two questions OLE asks during
//! a drag — has the gesture been abandoned, and what cursor to show. `DoDragDrop` is
//! modal: it runs its own message loop and does not return until the drop happens or the
//! drag is cancelled, so whichever thread calls it does nothing else until then. It is not
//! the UI thread, for the reason set out on [`Drag`]: a window that cannot paint during a
//! drag cannot show what the drag is about to do.
//!
//! # Dropping in
//!
//! winit already registers a drop target on the window to produce its own
//! `HoveredFile` / `DroppedFile` events, and those events carry only paths — no
//! modifier state, so no way to tell a copy from a move, and no way to answer with an
//! effect so the cursor shows what will happen. That is most of what a drop *is*.
//!
//! So [`Zone::attach`] revokes winit's target and registers this one, which answers
//! `DragOver` with a real `DROPEFFECT` and reports the modifiers on `Drop`. The rule it
//! applies is Explorer's:
//!
//! | | |
//! | --- | --- |
//! | `Ctrl` held | copy |
//! | `Shift` held | move |
//! | neither, same volume | move |
//! | neither, different volume | copy |
//! | neither, source under `%TEMP%` | copy — see [`under_temp`] |
//!
//! Whatever comes out of that is then narrowed to what the source said it would allow.
//! `pdwEffect` arrives holding the effects the source passed to `DoDragDrop`, and answering
//! with one that is not among them is not a harmless liberty: `DROPEFFECT_MOVE` returned to a
//! source is an *instruction* to delete what it handed over.
//!
//! The callbacks arrive on the UI thread from inside winit's message pump, where the
//! application state is not reachable — so they read and write a small shared block
//! instead, which the frame loop publishes into and drains from.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::shell::clipboard::Effect;

/// What a zone does with whatever lands on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Onto {
    /// Copy or move the items into this folder.
    Folder(PathBuf),
    /// Pin them in the sidebar. Nothing is copied and nothing is moved, which is why it
    /// reports itself to the pointer as a link — the same answer Explorer gives when you
    /// drag a folder onto Quick Access.
    Bookmarks,
}

/// What a completed drop asks for.
#[derive(Clone, Debug)]
pub struct Dropped {
    pub items: Vec<PathBuf>,
    pub effect: Effect,
    /// Where it landed, in physical pixels.
    pub at: (i32, i32),
    /// What the zone under it was for.
    pub onto: Onto,
    /// Whether the *right* button carried the drag, which in Windows means "ask me what to do
    /// with it" rather than "do the obvious thing".
    ///
    /// Read from the last `DragOver` rather than from the drop, because by the time `Drop` is
    /// called the button has been released and its bit is gone.
    pub asked: bool,
}

/// Where a drop would go, published by the frame loop for the drop target to read.
///
/// The target callbacks cannot reach the application, and they have to answer
/// `DragOver` *immediately* with an effect — so the answer has to already be here.
#[derive(Clone, Default)]
pub struct Targets {
    /// Each droppable region and what it is for, in physical pixels, back to front.
    pub zones: Vec<((i32, i32, i32, i32), Onto)>,
}

impl Targets {
    /// What is at a point, if anything.
    pub fn at(&self, (x, y): (i32, i32)) -> Option<&Onto> {
        self.zones
            .iter()
            .rev()
            .find(|((l, t, r, b), _)| x >= *l && x < *r && y >= *t && y < *b)
            .map(|(_, onto)| onto)
    }
}

/// The block the OLE callbacks and the frame loop share.
#[derive(Default)]
pub struct Shared {

    /// Published by the frame loop.
    pub targets: Targets,
    /// Whether the right button was down the last time the drag was seen moving.
    pub right_button: bool,
    /// Where a drag is hovering, for the frame loop to highlight.
    pub hovering: Option<(i32, i32)>,
    /// Completed drops, waiting to be acted on.
    pub dropped: Vec<Dropped>,
}

/// The receiving side.
pub struct Zone {
    shared: Arc<Mutex<Shared>>,
    #[cfg(windows)]
    registered: bool,
    #[cfg(windows)]
    hwnd: isize,
}

impl Zone {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared::default())),
            #[cfg(windows)]
            registered: false,
            #[cfg(windows)]
            hwnd: 0,
        }
    }

    /// Tell the target where drops may land this frame.
    /// What a drop at this point would be for, as the OLE callbacks see it.
    ///
    /// For tests: the callbacks answer from the published zones on another stack entirely, and
    /// this is the only way to ask them the same question from here.
    #[cfg(test)]
    pub fn resolve(&self, at: (i32, i32)) -> Option<Onto> {
        self.shared.lock().ok()?.targets.at(at).cloned()
    }

    pub fn publish(&self, targets: Targets) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.targets = targets;
        }
    }

    /// Where a drag is currently hovering, for the highlight.
    pub fn hovering(&self) -> Option<(i32, i32)> {
        self.shared.lock().ok().and_then(|shared| shared.hovering)
    }

    /// Take any completed drops.
    pub fn take_drops(&self) -> Vec<Dropped> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.dropped))
            .unwrap_or_default()
    }

    /// Register on the window, replacing the one winit installed.
    ///
    /// Idempotent, and safe to call before the window exists — it does nothing until
    /// there is a handle.
    pub fn attach(&mut self, owner: super::Owner) {
        #[cfg(windows)]
        {
            if self.registered || owner.0 == 0 {
                return;
            }
            use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};

            let target: IDropTarget = win::Target::new(self.shared.clone(), owner.0).into();
            // SAFETY: winit registered its own target on this window; ours replaces it.
            // OLE takes a reference of its own, and the local one is deliberately leaked
            // so the target outlives this scope — `Drop` revokes it, which releases it.
            unsafe {
                let _ = RevokeDragDrop(owner.hwnd());
                if RegisterDragDrop(owner.hwnd(), &target).is_ok() {
                    self.registered = true;
                    self.hwnd = owner.0;
                    std::mem::forget(target);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = owner;
    }
}

impl Default for Zone {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl Drop for Zone {
    fn drop(&mut self) {
        if self.registered {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Ole::RevokeDragDrop;
            // SAFETY: registered by `attach` on this handle, revoked once.
            let _ = unsafe { RevokeDragDrop(HWND(self.hwnd as *mut std::ffi::c_void)) };
        }
    }
}

/// A drag this program started, in flight on an apartment of its own.
///
/// `DoDragDrop` is modal — it runs its own message loop and does not return until the drop
/// lands or the gesture is abandoned — so the thread that calls it does nothing else for the
/// length of the drag. On the UI thread that means no frames: not one repaint reaches the
/// window while a drag started *inside* it is running, so the row a drop would land in could
/// not be highlighted and a file selected by the drag itself could not be seen to be selected.
/// Neither is cosmetic. They are the only feedback the gesture has.
///
/// So it runs here instead, and the UI thread keeps painting underneath it. The drop target is
/// registered in the UI thread's apartment, and OLE marshals the callbacks back into it — the
/// same machinery that lets Explorer call into this process at all — so the highlight is driven
/// by the same `DragOver` that answers the cursor.
pub struct Drag {
    done: std::sync::mpsc::Receiver<Option<Effect>>,
}

impl Drag {
    /// What the target did with the files, once the drag has ended.
    ///
    /// `None` while it is still in flight. `Some(None)` for a drag that was abandoned, or
    /// dropped somewhere that took nothing.
    pub fn finished(&self) -> Option<Option<Effect>> {
        match self.done.try_recv() {
            Ok(effect) => Some(effect),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            // The thread went away without answering. Still an ended drag, and leaving the
            // handle in place would wedge every later one.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
        }
    }

    /// A drag with nothing behind it, and the end of the wire to finish it from.
    ///
    /// For the test that a drag in flight keeps the window painting — the property this whole
    /// arrangement exists for, and one no unit test could reach if the only way to have a drag
    /// were to hold a real mouse button down.
    #[cfg(test)]
    pub fn pretend() -> (Self, std::sync::mpsc::Sender<Option<Effect>>) {
        let (tx, done) = std::sync::mpsc::channel();
        (Self { done }, tx)
    }
}

/// Pick these files up and start dragging them.
///
/// Returns as soon as the drag is under way. Ask the handle for the outcome — a move has taken
/// the files out of the folder they were in, so the source needs re-reading.
pub fn drag_out(items: Vec<PathBuf>) -> Option<Drag> {
    if items.is_empty() {
        return None;
    }
    #[cfg(windows)]
    {
        // The thread that owns the window and its input, for the attachment below.
        let ui_thread = unsafe {
            windows::Win32::System::Threading::GetCurrentThreadId()
        };
        let (tx, done) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("drag-source".to_owned())
            .spawn(move || {
                // This thread's own apartment: the data object, the drop source and the modal
                // loop all belong to it.
                crate::shell::init();
                let _ = tx.send(win::drag_out(&items, ui_thread));
            })
            .ok()?;
        Some(Drag { done })
    }
    #[cfg(not(windows))]
    {
        let _ = items;
        None
    }
}

/// Explorer's rule for what an unmodified drag means.
///
/// Within a volume a drag moves; across volumes it copies. Which is not arbitrary — a
/// move within a volume is a rename of a directory entry and effectively free, while
/// across volumes it is a copy followed by a delete, and defaulting to that would make
/// an accidental drag both slow and destructive.
pub fn default_effect(source: Option<&std::path::Path>, target: &std::path::Path) -> Effect {
    let volume = |path: &std::path::Path| -> Option<String> {
        let text = path.to_string_lossy();
        // A drive letter, or the `\\server\share` of a UNC path.
        if text.len() >= 2 && text.as_bytes()[1] == b':' {
            return Some(text[..2].to_lowercase());
        }
        if let Some(rest) = text.strip_prefix(r"\\") {
            let mut parts = rest.split(['\\', '/']);
            let server = parts.next()?;
            let share = parts.next()?;
            return Some(format!(r"\\{server}\{share}").to_lowercase());
        }
        None
    };
    match (source.and_then(volume), volume(target)) {
        (Some(from), Some(to)) if from == to => Effect::Move,
        _ => Effect::Copy,
    }
}

/// Whether a path is inside the user's temporary directory.
///
/// This is the signal that what a source is offering is a *materialisation* rather than the
/// user's own files: the contents of an archive, a mail attachment, anything a source had to
/// unpack somewhere before it had a path to put in a `CF_HDROP` at all. It deletes them again
/// once the drag is over, so the same-volume rule above — which would call a drop into any
/// folder on `C:` a move, `%TEMP%` being on `C:` — is answering a question the user cannot
/// have asked. Dragging the contents of a 7-Zip archive into a folder reported
/// `DROPEFFECT_MOVE` to 7-Zip, which took it as leave to delete its extraction, and it did so
/// while the copy was still reading out of it.
///
/// Only the *default* is decided here. `Shift` still asks for a move and still gets one, if
/// the source allows one.
pub fn under_temp(path: &std::path::Path) -> bool {
    use std::path::{Path, PathBuf};

    let temp = std::env::temp_dir();
    if temp.as_os_str().is_empty() {
        return false;
    }
    // Case-folded, because `starts_with` is not; and still component-wise through it, so
    // `C:\Temporary` is not taken for something inside `C:\Temp`.
    let fold = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
    if fold(path).starts_with(fold(&temp)) {
        return true;
    }
    // `%TEMP%` is an 8.3 short path on some machines while `CF_HDROP` carries the long form.
    // They are the same directory and only resolving both shows it. Worth a pair of syscalls
    // because this runs once per drag, on `DragEnter`, and not once per mouse move.
    match (std::fs::canonicalize(path), std::fs::canonicalize(&temp)) {
        (Ok(path), Ok(temp)) => fold(&path).starts_with(fold(&temp)),
        _ => false,
    }
}

/// What marks a staging directory as this program's own. See [`claim`].
const STAGING: &str = "yafe-drop-";

/// Take a source's temporary files, before it takes them back.
///
/// The problem this solves is one of timing and cannot be solved by being quick. A source that
/// materialises its data — an archiver — deletes it again as soon as `DoDragDrop` returns,
/// which is as soon as `IDropTarget::Drop` returns. So the whole of the copy would have to
/// happen inside `Drop`, on the UI thread, with the window unable to paint: the drop of a large
/// archive would freeze everything, and no amount of scoping helps, because a thread stuck
/// inside a callback cannot paint one tab and not another.
///
/// What it *can* do inside `Drop` is stop being the source's problem. The files are in `%TEMP%`
/// by definition — that is what [`under_temp`] established — so a staging directory made
/// alongside them is on the same volume, and moving them into it is a directory-entry rename:
/// microseconds, whatever the archive weighs. The source is then welcome to delete a folder
/// that is empty, and the copy to the real destination runs on the ops thread like every other
/// one, with the window live and every tab usable.
///
/// It also makes the right-button menu safe, which it could not otherwise be: `Copy here` is
/// answered whenever the user gets round to it, long after any source has cleaned up.
///
/// Non-temporary items are returned untouched — the same rename applied to a drag from
/// Explorer would move the user's actual files into a scratch folder. That is the whole reason
/// the test is narrow.
pub(crate) fn claim(items: Vec<PathBuf>) -> Vec<PathBuf> {
    if !items.first().is_some_and(|first| under_temp(first)) {
        return items;
    }
    let Some(staging) = staging() else {
        return items;
    };
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| claim_one(&staging, index, item))
        .collect()
}

/// One item into the staging directory, under its own name.
///
/// The name has to survive: `IFileOperation` names what it copies after the source, so an item
/// staged under a different name would arrive at the destination with it.
fn claim_one(staging: &std::path::Path, index: usize, item: PathBuf) -> PathBuf {
    let Some(name) = item.file_name() else {
        return item;
    };
    let mut to = staging.join(name);
    // Two items with one name, which means they came from different folders. Resolved with a
    // folder rather than a suffix, for the reason above.
    if to.exists() {
        let nested = staging.join(index.to_string());
        if std::fs::create_dir_all(&nested).is_err() {
            return item;
        }
        to = nested.join(name);
    }
    match std::fs::rename(&item, &to) {
        Ok(()) => to,
        // Nothing moved, so the original is still the right answer — and still a race.
        Err(_) => item,
    }
}

/// A directory of this program's own, directly inside `%TEMP%` — so on the same volume as
/// anything [`claim`] will put in it, which is what makes the claim a rename.
fn staging() -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);
    let temp = std::env::temp_dir();
    // Bounded rather than `loop`: if something is answering `AlreadyExists` to every name this
    // can produce, the answer is to give up and let the drop go on unclaimed.
    for _ in 0..64 {
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = temp.join(format!("{STAGING}{}-{next}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Some(dir),
            Err(why) if why.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Whether a directory is a staging directory belonging to *this* process.
///
/// This is the test that authorises a recursive delete — `crate::shell::ops` removes the
/// directory a job's items were claimed into once the job is done — so it asks for both halves:
/// the name, and that the thing is sitting directly in the temporary directory. Neither on its
/// own would be enough to be trusted with `remove_dir_all`.
pub(crate) fn is_staging(dir: &std::path::Path) -> bool {
    if dir.parent() != Some(std::env::temp_dir().as_path()) {
        return false;
    }
    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(rest) = name.strip_prefix(STAGING) else {
        return false;
    };
    let Some((pid, _)) = rest.split_once('-') else {
        return false;
    };
    pid.parse::<u32>().is_ok_and(|pid| pid == std::process::id())
}

/// Remove staging directories left behind by a run that is over.
///
/// Called once at startup. Nothing should ever be left — the job that consumes a claim takes
/// the directory with it — but being killed between the claim and the copy would otherwise
/// leave an extracted archive in `%TEMP%` for good. A directory belonging to a process that is
/// still running is left alone, which is the safe way round: another window may be copying out
/// of it, and that is the exact bug all of this exists to fix.
pub fn sweep() {
    let temp = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&temp) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(rest) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(STAGING))
        else {
            continue;
        };
        let Some(pid) = rest.split_once('-').and_then(|(pid, _)| pid.parse::<u32>().ok()) else {
            continue;
        };
        if pid == std::process::id() || running(pid) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Whether a process id is still in use.
///
/// A handle that opens means yes, and a recycled id means yes as well — both answers keep the
/// directory, which is the harmless mistake to make. Only an id nothing answers to gets swept.
#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: a query for a handle that is closed again immediately.
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

/// Nothing is ever claimed off Windows, so nothing is ever swept.
#[cfg(not(windows))]
fn running(_pid: u32) -> bool {
    true
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::{implement, Ref, BOOL, HRESULT};
    use windows::Win32::Foundation::{POINTL, S_OK};
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{
        DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, DROPEFFECT,
        DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    };
    use windows::Win32::System::SystemServices::{
        MK_CONTROL, MK_LBUTTON, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
    };

    /// The three `DRAGDROP_S_*` values a source returns. Success codes, not errors,
    /// which is why they are spelled out rather than gone looking for.
    const DRAGDROP_S_DROP: HRESULT = HRESULT(0x0004_0100u32 as i32);
    const DRAGDROP_S_CANCEL: HRESULT = HRESULT(0x0004_0101u32 as i32);
    const DRAGDROP_S_USEDEFAULTCURSORS: HRESULT = HRESULT(0x0004_0102u32 as i32);

    // ---- Dragging out --------------------------------------------------

    /// The source half of a drag: the two questions OLE asks while one is running.
    #[implement(IDropSource)]
    struct Source;

    impl IDropSource_Impl for Source_Impl {
        fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
            if escape.as_bool() {
                return DRAGDROP_S_CANCEL;
            }
            // The drag ends when the button that started it comes up. Both are checked
            // because a right-drag is a legitimate gesture — it is what produces
            // Explorer's "copy here / move here / create shortcut" menu on drop.
            if keys.0 & (MK_LBUTTON.0 | MK_RBUTTON.0) == 0 {
                return DRAGDROP_S_DROP;
            }
            S_OK
        }

        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
            // Let OLE show the standard copy, move and no-entry cursors rather than
            // inventing a set that would not match anything else on the desktop.
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    /// Run the drag to its end. Called on the drag's own thread, never on the UI one.
    ///
    /// `ui_thread` owns the window the gesture started in, and its input queue is the one that
    /// knows the button is down. `DoDragDrop` reads that state to decide which button it is
    /// following and when to stop following it, and on Windows key state and mouse capture are
    /// per *input queue* rather than per process — so from a fresh thread it would see no button
    /// held and end the drag before the pointer had moved. `AttachThreadInput` joins the two
    /// queues for the length of the drag, which is what makes the capture and the button state
    /// reachable from here. It is undone on the way out, including when the drag fails.
    pub fn drag_out(items: &[PathBuf], ui_thread: u32) -> Option<Effect> {
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

        if items.is_empty() {
            return None;
        }
        let data = data_object(items)?;
        let source: IDropSource = Source.into();

        /// Undoes the attachment however this function leaves.
        struct Attached(u32, u32);
        impl std::ops::Drop for Attached {
            fn drop(&mut self) {
                // SAFETY: undoes exactly the attachment made below, once.
                let _ = unsafe { AttachThreadInput(self.0, self.1, false) };
            }
        }
        // SAFETY: both threads are alive for the length of the drag — this one by definition,
        // the UI one because it is the one waiting on the outcome.
        let attached = unsafe {
            let mine = GetCurrentThreadId();
            AttachThreadInput(mine, ui_thread, true)
                .as_bool()
                .then(|| Attached(mine, ui_thread))
        };

        let mut effect = DROPEFFECT_NONE;
        // SAFETY: both interfaces outlive the call, and `DoDragDrop` runs its own modal
        // loop on the calling thread — which is the one holding the apartment.
        let hr = unsafe {
            DoDragDrop(
                &data,
                &source,
                // `LINK` as well, because this window has a target of its own that answers with
                // it: the Bookmarks group, which pins a folder rather than copying it. A target
                // returning an effect the source never offered is a target OLE refuses, so
                // leaving it out made dragging a folder onto Bookmarks do nothing at all.
                DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
                &mut effect,
            )
        };
        drop(attached);
        if hr != DRAGDROP_S_DROP {
            return None;
        }
        if effect.0 & DROPEFFECT_MOVE.0 != 0 {
            Some(Effect::Move)
        } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
            Some(Effect::Copy)
        } else {
            None
        }
    }

    /// The shell's own data object for a selection, so a target gets every format
    /// Explorer would have offered rather than only the one this program knows about.
    fn data_object(items: &[PathBuf]) -> Option<IDataObject> {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::Common::ITEMIDLIST;
        use windows::Win32::UI::Shell::{
            SHCreateShellItemArrayFromIDLists, SHParseDisplayName, BHID_DataObject,
        };

        struct Pidl(*mut ITEMIDLIST);
        impl std::ops::Drop for Pidl {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    // SAFETY: allocated by `SHParseDisplayName`, freed once.
                    unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
                }
            }
        }

        let pidls: Vec<Pidl> = items
            .iter()
            .filter_map(|path| {
                let wide = crate::shell::wide(path);
                let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
                // SAFETY: `wide` is null-terminated and outlives the call.
                let ok = unsafe {
                    SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok()
                };
                (ok && !raw.is_null()).then_some(Pidl(raw))
            })
            .collect();
        if pidls.is_empty() {
            return None;
        }
        let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();

        // SAFETY: the PIDLs outlive the array, which copies what it needs.
        unsafe {
            let array = SHCreateShellItemArrayFromIDLists(&raw).ok()?;
            array.BindToHandler(None, &BHID_DataObject).ok()
        }
    }

    // ---- Dropping in ---------------------------------------------------

    /// The receiving half. Every method runs on the UI thread, inside the window's
    /// message pump, where the application is not reachable — so all it does is read and
    /// write the shared block.
    #[implement(IDropTarget)]
    pub struct Target {
        shared: Arc<Mutex<Shared>>,
        /// What the drag in flight is carrying, read between `DragEnter` and `Drop` so the
        /// effect rules can see where it came from.
        held: Mutex<Option<Incoming>>,
        /// The window this is registered on, for turning a screen point into a client one.
        /// Held here rather than in `shared` because the callbacks convert points while
        /// holding that lock, and a `Mutex` is not reentrant.
        hwnd: isize,
    }

    impl Target {
        pub fn new(shared: Arc<Mutex<Shared>>, hwnd: isize) -> Self {
            Self {
                shared,
                held: Mutex::new(None),
                hwnd,
            }
        }
    }

    /// What a drag is carrying, as far as it can be known *while it is still moving* — read
    /// once, when it arrives, to answer `DragOver` with.
    ///
    /// `GetData` is not a getter. It is a request that the source *render* what it is
    /// offering, and a source may do arbitrary work to answer one — an archiver renders
    /// `CF_HDROP` by extracting files to a temporary folder, because until it has it has no
    /// paths to put in one. This was being called from `effect_at`, which runs on every
    /// `DragOver`, so a drag crossing the window asked the source to render its data dozens of
    /// times a second.
    ///
    /// Which cuts the other way too, and is why this is not what the drop then acts on: a
    /// source is entitled to have nothing to give until the drop is real, and answering a
    /// speculative request during the drag is the part it is allowed to skip. `Drop` asks
    /// again for that reason and falls back to this only if the second answer is empty.
    struct Incoming {
        /// Every path the data object offered, via `CF_HDROP`. Empty is not an error — see
        /// above.
        items: Vec<PathBuf>,
        /// Whether those paths are a temporary the source is going to take back — see
        /// [`super::under_temp`]. Decided from the first path: a data object carrying files
        /// from two places at once is not a thing any source produces.
        temporary: bool,
    }

    impl Incoming {
        fn read(data: Option<&IDataObject>) -> Self {
            let items = data.and_then(paths_of).unwrap_or_default();
            let temporary = items.first().is_some_and(|first| super::under_temp(first));
            Self { items, temporary }
        }
    }

    /// The nearest thing to `wanted` that the source is willing to allow.
    ///
    /// `pdwEffect` is in/out on all three callbacks: on the way in it holds the effects the
    /// source passed to `DoDragDrop`. Answering with one that is not in that set was how
    /// dragging the contents of an archive into a folder on the same volume told the archiver
    /// to delete its extraction — the drag had only ever been offered as a copy.
    ///
    /// A move degrades to a copy and never the other way round. The fallback for an effect the
    /// source will not allow has to be the one that destroys nothing, which is also why
    /// pinning — which copies nothing at all — may report a copy but must never report a move.
    fn permitted(wanted: DROPEFFECT, allowed: DROPEFFECT) -> DROPEFFECT {
        // A source that fills this in as nothing has told us nothing, rather than that it
        // refuses every drop. Taken as no restriction, which is what ignoring the field
        // altogether amounted to.
        if allowed == DROPEFFECT_NONE {
            return wanted;
        }
        let offers = |effect: DROPEFFECT| allowed.0 & effect.0 != 0;
        match wanted {
            asked if offers(asked) => asked,
            DROPEFFECT_MOVE | DROPEFFECT_LINK if offers(DROPEFFECT_COPY) => DROPEFFECT_COPY,
            _ => DROPEFFECT_NONE,
        }
    }

    impl Target_Impl {
        /// A screen point in the window's own coordinates, which is what the zones are in.
        ///
        /// `IDropTarget` is handed **screen** coordinates and [`Targets`] is published in the
        /// window's own. They were compared directly, and the only reason anything worked at all
        /// is that the two overlap when a window sits near the top left of the screen: a drop
        /// resolved to whichever zone the *screen* point happened to fall in, which was almost
        /// always the whole pane rather than the folder row under the pointer. So a file dropped
        /// on a folder went into the folder already being shown, where it was filtered out as a
        /// no-op — and dragging appeared to do nothing whatsoever.
        ///
        /// The handle is the target's own and not the shared block's, deliberately: the callbacks
        /// below call this *while holding* that lock, and a `Mutex` is not reentrant.
        fn in_client(&self, pt: &POINTL) -> (i32, i32) {
            use windows::Win32::Foundation::{HWND, POINT};
            use windows::Win32::Graphics::Gdi::ScreenToClient;

            if self.hwnd == 0 {
                return (pt.x, pt.y);
            }
            let hwnd = self.hwnd;
            let mut point = POINT { x: pt.x, y: pt.y };
            // SAFETY: a coordinate conversion against a live window handle.
            unsafe {
                let _ = ScreenToClient(HWND(hwnd as *mut std::ffi::c_void), &mut point);
            }
            (point.x, point.y)
        }

        /// What this drag would do at a point, given the keys held and what the source allows.
        fn effect_at(
            &self,
            keys: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            allowed: DROPEFFECT,
        ) -> DROPEFFECT {
            let at = self.in_client(pt);
            let onto = {
                let Ok(mut shared) = self.shared.lock() else {
                    return DROPEFFECT_NONE;
                };
                shared.hovering = Some(at);
                // Remembered here because `Drop` is called with the button already released.
                if keys.0 & MK_RBUTTON.0 != 0 {
                    shared.right_button = true;
                }
                shared.targets.at(at).cloned()
            };
            let target = match onto {
                Some(Onto::Folder(path)) => path,
                // Pinning moves nothing, so it answers `LINK` whatever is held down. It is
                // also the only honest answer: a copy cursor over the sidebar would be
                // promising a copy that is not going to happen.
                Some(Onto::Bookmarks) => return permitted(DROPEFFECT_LINK, allowed),
                None => return DROPEFFECT_NONE,
            };

            if keys.0 & MK_CONTROL.0 != 0 {
                return permitted(DROPEFFECT_COPY, allowed);
            }
            if keys.0 & MK_SHIFT.0 != 0 {
                return permitted(DROPEFFECT_MOVE, allowed);
            }
            // Where it came from, read from the data object when the drag arrived rather than
            // guessed — a drag can come from anywhere, including from nowhere with a path.
            let (source, temporary) = self
                .held
                .lock()
                .ok()
                .and_then(|held| {
                    let incoming = held.as_ref()?;
                    Some((incoming.items.first().cloned(), incoming.temporary))
                })
                .unwrap_or((None, false));
            // Nothing the source is about to delete out from under the copy is a move.
            if temporary {
                return permitted(DROPEFFECT_COPY, allowed);
            }
            let wanted = match super::default_effect(source.as_deref(), &target) {
                Effect::Move => DROPEFFECT_MOVE,
                Effect::Copy => DROPEFFECT_COPY,
            };
            permitted(wanted, allowed)
        }
    }

    impl IDropTarget_Impl for Target_Impl {
        fn DragEnter(
            &self,
            data: Ref<IDataObject>,
            keys: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            // Once, here — see [`Incoming`]. The lock is taken and let go before `effect_at`,
            // which takes it again and would deadlock on a `Mutex` that is not reentrant.
            if let Ok(mut held) = self.held.lock() {
                *held = Some(Incoming::read(data.as_ref()));
            }
            // SAFETY: OLE always passes a valid out-pointer here. It is read before it is
            // written because on the way in it holds the effects the source allows.
            unsafe {
                let allowed = *effect;
                *effect = self.effect_at(keys, pt, allowed);
            }
            Ok(())
        }

        fn DragOver(
            &self,
            keys: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            // SAFETY: as above.
            unsafe {
                let allowed = *effect;
                *effect = self.effect_at(keys, pt, allowed);
            }
            Ok(())
        }

        fn DragLeave(&self) -> windows::core::Result<()> {
            if let Ok(mut held) = self.held.lock() {
                *held = None;
            }
            if let Ok(mut shared) = self.shared.lock() {
                shared.hovering = None;
                shared.right_button = false;
            }
            Ok(())
        }

        fn Drop(
            &self,
            data: Ref<IDataObject>,
            keys: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            // What `DragEnter` read, unless it somehow did not run — in which case read it now
            // rather than leave the drop with nothing. Filled before `effect_at`, which takes
            // this same lock, and taken back out after.
            if let Ok(mut held) = self.held.lock() {
                if held.is_none() {
                    *held = Some(Incoming::read(data.as_ref()));
                }
            }
            // SAFETY: OLE always passes a valid out-pointer here, holding on the way in the
            // effects the source allows.
            let chosen = unsafe {
                let allowed = *effect;
                let chosen = self.effect_at(keys, pt, allowed);
                *effect = chosen;
                chosen
            };

            // Asked for again here rather than reused from `DragEnter`, because for a source
            // that renders on demand *this* is the call that matters: 7-Zip extracts the
            // archive to answer it, and has nothing to give until the drop is real. Reusing
            // the drag-time read left the drop with an empty list, so nothing was extracted
            // and nothing was copied. The drag-time read stays as the fallback, for a source
            // that renders once and not again.
            let cached = self
                .held
                .lock()
                .ok()
                .and_then(|mut held| held.take())
                .map(|incoming| incoming.items)
                .unwrap_or_default();
            let items = match data.as_ref().and_then(paths_of) {
                Some(fresh) if !fresh.is_empty() => fresh,
                _ => cached,
            };
            // Converted before the lock is taken, not inside it.
            let at = self.in_client(pt);
            // Where it landed, and the drag forgotten, in a turn of the lock of its own — the
            // claim below touches the filesystem, and doing that while the frame loop waits on
            // this lock would stall the window for exactly as long as the claim takes.
            let landing = self.shared.lock().ok().map(|mut shared| {
                shared.hovering = None;
                let onto = shared.targets.at(at).cloned();
                (onto, std::mem::take(&mut shared.right_button))
            });
            let Some((Some(onto), asked)) = landing else {
                return Ok(());
            };
            if items.is_empty() || chosen == DROPEFFECT_NONE {
                return Ok(());
            }
            // The one thing that has to happen before this returns: see [`super::claim`]. Not
            // for a pin, which copies nothing and would otherwise bookmark a scratch folder.
            let items = match onto {
                Onto::Folder(_) => super::claim(items),
                Onto::Bookmarks => items,
            };
            if let Ok(mut shared) = self.shared.lock() {
                shared.dropped.push(Dropped {
                    items,
                    effect: if chosen.0 & DROPEFFECT_MOVE.0 != 0 {
                        Effect::Move
                    } else {
                        Effect::Copy
                    },
                    at,
                    onto,
                    asked,
                });
            }
            Ok(())
        }
    }

    /// Every path a data object is offering, via `CF_HDROP`.
    fn paths_of(data: &IDataObject) -> Option<Vec<PathBuf>> {
        use windows::Win32::System::Com::{FORMATETC, TYMED_HGLOBAL};
        use windows::Win32::System::Ole::ReleaseStgMedium;
        use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

        const CF_HDROP: u16 = 15;
        let format = FORMATETC {
            cfFormat: CF_HDROP,
            ptd: std::ptr::null_mut(),
            dwAspect: 1,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        // SAFETY: the medium is released on every path out, and the handle is only read
        // while it is held.
        unsafe {
            let medium = data.GetData(&format).ok()?;
            let handle = medium.u.hGlobal;
            if handle.is_invalid() {
                return None;
            }
            let drop = HDROP(handle.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let mut items = Vec::with_capacity(count as usize);
            for index in 0..count {
                // The length first: a path can be longer than `MAX_PATH`, and a fixed
                // buffer would silently truncate one.
                let len = DragQueryFileW(drop, index, None);
                if len == 0 {
                    continue;
                }
                let mut buffer = vec![0u16; len as usize + 1];
                let written = DragQueryFileW(drop, index, Some(&mut buffer));
                if written > 0 {
                    buffer.truncate(written as usize);
                    items.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
                }
            }
            let mut medium = medium;
            ReleaseStgMedium(&mut medium);
            Some(items)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_drag_within_a_volume_moves_and_across_copies() {
        // The rule that makes an accidental drag cheap rather than expensive.
        assert_eq!(
            default_effect(Some(Path::new(r"C:\a\one.txt")), Path::new(r"C:\b")),
            Effect::Move
        );
        assert_eq!(
            default_effect(Some(Path::new(r"C:\a\one.txt")), Path::new(r"D:\b")),
            Effect::Copy
        );
        // Case is not a volume difference.
        assert_eq!(
            default_effect(Some(Path::new(r"c:\a\one.txt")), Path::new(r"C:\b")),
            Effect::Move
        );
    }

    #[test]
    fn a_network_share_is_its_own_volume() {
        assert_eq!(
            default_effect(
                Some(Path::new(r"\\server\share\one.txt")),
                Path::new(r"\\server\share\sub")
            ),
            Effect::Move
        );
        assert_eq!(
            default_effect(
                Some(Path::new(r"\\server\share\one.txt")),
                Path::new(r"\\server\other\sub")
            ),
            Effect::Copy,
            "two shares on one server are still two volumes"
        );
        assert_eq!(
            default_effect(
                Some(Path::new(r"\\server\share\one.txt")),
                Path::new(r"C:\b")
            ),
            Effect::Copy
        );
    }

    #[test]
    fn an_unknown_source_copies() {
        // A drag from somewhere with no volume — a virtual folder, a browser — cannot be
        // a move, and guessing otherwise would be the destructive guess.
        assert_eq!(default_effect(None, Path::new(r"C:\b")), Effect::Copy);
    }

    #[test]
    fn zones_resolve_the_front_one_first() {
        let under = Onto::Folder(PathBuf::from(r"C:\under"));
        let over = Onto::Folder(PathBuf::from(r"C:\over"));
        let targets = Targets {
            zones: vec![
                ((0, 0, 100, 100), Onto::Bookmarks),
                ((0, 0, 100, 100), under.clone()),
                ((50, 50, 150, 150), over.clone()),
            ],
        };
        assert_eq!(
            targets.at((60, 60)),
            Some(&over),
            "the later zone is the one in front"
        );
        assert_eq!(
            targets.at((10, 10)),
            Some(&under),
            "a listing over the sidebar's own zone means the listing"
        );
        assert_eq!(targets.at((200, 200)), None);
    }
}
