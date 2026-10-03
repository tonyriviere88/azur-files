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
//! instead, which the frame loop publishes into and drains from. **And then ask for a frame**,
//! because for a drag from another program nothing else will: OLE has the pointer, so no mouse
//! event reaches winit and nothing wakes the window to look at what the callbacks just wrote. See
//! [`Shared::hovering`].

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
    ///
    /// **Writing this is not enough on its own: the frame loop has to be woken to read it.** For a
    /// drag this window started that happens anyway — [`crate::app::App::pump_drag`] asks for a
    /// repaint on every frame for the length of it — but a drag from *another* program has nothing
    /// driving the window at all. OLE holds the pointer, so not one mouse event reaches winit;
    /// `DragOver` arrives instead, wrote this, and nothing ever came to look. The highlight
    /// therefore appeared for a drag between two panes and not for one out of an archive or out of
    /// Explorer, which is the same window and the same folder row answering the same question two
    /// different ways. So the callbacks request a repaint of their own — see `Target::wake`.
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
    ///
    /// `ctx` is how the callbacks wake the frame loop, and without it a drag from another program
    /// is invisible: see [`Shared::hovering`].
    pub fn attach(&mut self, owner: super::Owner, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            if self.registered || owner.0 == 0 {
                return;
            }
            use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};

            let target: IDropTarget =
                win::Target::new(self.shared.clone(), owner.0, ctx.clone()).into();
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
        let _ = (owner, ctx);
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

    let temp = temp_root();
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

/// `%TEMP%`, or wherever a test has pointed it.
///
/// Every part of a claim is decided against this directory: whether a drop's items are a source's
/// materialisation ([`under_temp`]), where the staging directory is made ([`staging`]), and what
/// [`is_staging`] will authorise a `remove_dir_all` of. A test that could not move it would have to
/// build its fixture in the real `%TEMP%` to reach any of that, and the containment rule in
/// `crate::sandbox` does not allow it — which is why the claim went untested through two rounds of
/// the same bug.
fn temp_root() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = TEMP_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return root;
    }
    std::env::temp_dir()
}

#[cfg(test)]
thread_local! {
    /// See [`temp_root`]. Thread-local rather than an environment variable, so that two tests
    /// running at once cannot move each other's `%TEMP%` — and neither can move the real one.
    static TEMP_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Treat `root` as `%TEMP%` for as long as the returned guard is alive.
#[cfg(test)]
fn temp_here(root: &std::path::Path) -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            TEMP_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
        }
    }
    let root = root.to_path_buf();
    TEMP_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(root));
    Guard
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
/// one, with the window live and every tab usable. When the rename is refused — which is a
/// case, not a theory, and is [`take`] — there is more to it than that.
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
    let claimed: Vec<PathBuf> = items
        .into_iter()
        .enumerate()
        .map(|(index, item)| claim_one(&staging, index, item))
        .collect();
    // A claim that got nothing leaves an empty directory in `%TEMP%` that nothing will ever come
    // back for: the job only takes one with it when its items are *in* it — see
    // `crate::shell::ops::claimed` — and [`sweep`] leaves alone anything belonging to a process
    // that is still running. Which is how the failure that prompted all this was found in the
    // first place, sitting next to the archiver's own leftovers.
    if !claimed.iter().any(|item| item.starts_with(&staging)) {
        let _ = std::fs::remove_dir(&staging);
    }
    claimed
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
    if take(&item, &to) {
        to
    } else {
        // Nothing moved, so the original is still the right answer — and still a race.
        item
    }
}

/// Get whatever is at `from` over to `to`, by whatever means the filesystem will allow.
///
/// **A directory cannot be renamed while a single file anywhere beneath it is open.** Windows
/// answers `ERROR_ACCESS_DENIED` — 5, not the sharing violation the situation sounds like — and it
/// does so however that handle was opened: `FILE_SHARE_DELETE` and all, a plain reader is enough.
/// Renaming that same open file on its own succeeds. It is only the directory above it that
/// becomes unmovable.
///
/// That is the whole of why this bug came back after [`under_temp`] and [`claim`] had between them
/// already fixed it. Dragging *files* out of an archive claims them one rename at a time, so a held
/// file costs that file; dragging a *folder* out staked the entire tree on one rename, and anything
/// that had so much as looked at a freshly extracted file — a virus scanner is enough, and takes as
/// long as it likes over a folder full of them — refused it. The claim then gave up and returned
/// the archiver's own paths, the copy ran out of the archiver's temporary directory, and the
/// archiver deleted it part way through. Which is the original bug exactly: half the files arrive.
///
/// So a refusal is not the end of it. In order:
///
/// | | |
/// | --- | --- |
/// | rename | the whole item in one directory entry, and what nearly every claim still is |
/// | recurse, for a directory | the tree recreated and each child claimed in turn, so one held file costs one file |
/// | hard link | a second name for the same data: the source deleting *its* name leaves ours, and this is allowed where a rename is refused |
/// | copy | the bytes, when nothing cheaper is permitted — reading is the one thing a scanner's handle still allows |
///
/// Returns whether `to` is now where the item is to be found.
fn take(from: &std::path::Path, to: &std::path::Path) -> bool {
    // The fast path, and the one this is nearly always on: one directory entry rewritten,
    // microseconds, whatever the item weighs.
    if std::fs::rename(from, to).is_ok() {
        return true;
    }
    let Ok(what) = std::fs::symlink_metadata(from) else {
        return false;
    };
    // A junction or a symlink, whose rename has just been refused. A copy would follow it and
    // duplicate what it points at — which for a link to somewhere outside the extraction would be
    // copying the user's own files into a scratch folder. Left where it is instead.
    if what.file_type().is_symlink() {
        return false;
    }
    if what.is_dir() {
        return take_dir(from, to);
    }
    // A file whose rename was refused: a hard link is a second name for the same data, so the
    // source deleting its own name leaves the data reachable under ours. Failing that, the bytes.
    std::fs::hard_link(from, to).is_ok() || std::fs::copy(from, to).is_ok()
}

/// A directory whose rename was refused, one child at a time.
///
/// The children are what is claimed; the directories themselves are recreated. That loses the
/// timestamps the archive carried for the folder — the files keep theirs, since they are moved and
/// not remade — which is a real if small difference from what the copy would otherwise have
/// arrived with, and worth strictly less than the files this exists to save. Setting them would
/// mean a directory handle opened with `FILE_FLAG_BACKUP_SEMANTICS`, so it belongs in
/// `crate::windows` and not here, on a path taken only when the rename has already failed.
fn take_dir(from: &std::path::Path, to: &std::path::Path) -> bool {
    if std::fs::create_dir_all(to).is_err() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(from) else {
        return false;
    };
    let (mut arrived, mut refused) = (false, false);
    for entry in entries.flatten() {
        if take(&entry.path(), &to.join(entry.file_name())) {
            arrived = true;
        } else {
            refused = true;
        }
    }
    // Nothing came across at all and there was something to come: the source is still the better
    // answer of the two, and the empty shell made here is not one. A directory that was *already*
    // empty is a different thing, and is claimed — neither flag set.
    if !arrived && refused {
        let _ = std::fs::remove_dir(to);
        return false;
    }
    // Anything at all having moved settles it: reporting `from` now would send the copy to a
    // directory this had just emptied, which is a worse version of the bug being fixed.
    true
}

/// A directory of this program's own, directly inside `%TEMP%` — so on the same volume as
/// anything [`claim`] will put in it, which is what makes the claim a rename.
fn staging() -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);
    let temp = temp_root();
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
    if dir.parent() != Some(temp_root().as_path()) {
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
    let temp = temp_root();
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
        // Deliberately *not* `crate::sandbox::remove`: this is the running program clearing its
        // own staging directories out of `%TEMP%`, not a test tidying a fixture. The sandbox rule
        // is about where tests are allowed to reach, and a guard here would both fail to compile
        // in a release build and panic on the one job this function exists to do.
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
#[path = "../windows/dnd.rs"]
mod win;

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

    /// A sandbox directory standing in for `%TEMP%`, and the guard that makes it one.
    ///
    /// Everything a claim reads comes from [`temp_root`], so this is all it takes to hold the whole
    /// mechanism inside `target/sandbox` — the reason it can be tested at all now, where before
    /// checking any of it would have meant building fixtures in the real `%TEMP%`.
    fn sandboxed_temp(name: &str) -> (PathBuf, impl Drop) {
        let temp = crate::sandbox::fresh(name).join("temp");
        std::fs::create_dir_all(&temp).unwrap();
        let guard = temp_here(&temp);
        (temp, guard)
    }

    /// **A folder dragged out of an archive while a file inside it is open.**
    ///
    /// The gesture that has now lost half a decompression twice, and the second time is not a
    /// regression in this file: [`under_temp`] still calls the drop a copy, and [`claim`] still
    /// stages it. What failed is narrower and is in [`take`] — **Windows refuses to rename a
    /// directory that has any open file beneath it**, with `ERROR_ACCESS_DENIED`, whatever sharing
    /// that handle was opened with. A drag of *files* never noticed, because each one is claimed by
    /// a rename of its own; a drag of a *folder* staked the entire tree on one rename, and one
    /// handle anywhere under it — a scanner reading a freshly extracted file is enough — sent the
    /// copy back to reading out of the archiver's directory, which the archiver then deleted part
    /// way through.
    ///
    /// So the assertion is not "the claim succeeded". It is that the files are somewhere the
    /// archiver is not about to delete, under the name they have to keep.
    #[test]
    fn a_folder_is_claimed_even_when_a_file_inside_it_is_open() {
        let (temp, _temp) = sandboxed_temp("claim-a-held-folder");

        // What an archiver leaves for a folder drag: a directory of its own in `%TEMP%` with the
        // dragged folder inside it, and a tree under that.
        let master = temp.join("7zE436F3BDA").join("master");
        std::fs::create_dir_all(master.join("qml").join("QtQuick")).unwrap();
        std::fs::write(master.join("qml").join("QtQuick").join("one.qml"), b"one").unwrap();
        std::fs::write(master.join("two.txt"), b"two").unwrap();

        // The handle that does it. Nothing about it is exotic — a reader, sharing everything it is
        // able to share, which is what a scanner or a thumbnailer holds.
        let held = std::fs::File::open(master.join("qml").join("QtQuick").join("one.qml")).unwrap();
        // Stated rather than assumed: the test is worth nothing if the platform has stopped
        // refusing this, because the refusal is the thing being survived.
        assert!(
            std::fs::rename(&master, temp.join("moved")).is_err(),
            "the directory rename was allowed, so this is no longer a test of what it was written for"
        );

        let claimed = claim(vec![master.clone()]);
        drop(held);

        assert_eq!(claimed.len(), 1);
        let landed = &claimed[0];
        assert!(
            landed
                .parent()
                .is_some_and(|parent| is_staging(parent) || parent.parent().is_some_and(is_staging)),
            "the folder was left with the archiver, at {}",
            landed.display()
        );
        assert_eq!(
            landed.file_name(),
            master.file_name(),
            "the name has to survive, or the copy arrives at the destination under another one"
        );
        // The whole tree, the file that was open included.
        assert_eq!(
            std::fs::read_to_string(landed.join("qml").join("QtQuick").join("one.qml")).unwrap(),
            "one",
            "the file that was open did not come across"
        );
        assert_eq!(
            std::fs::read_to_string(landed.join("two.txt")).unwrap(),
            "two"
        );
        assert!(
            !master.join("two.txt").exists(),
            "a file was left where the archiver is about to delete it"
        );
    }

    /// The fast path, which is the one nearly every claim is still on: one rename, and the
    /// archiver no longer has it.
    #[test]
    fn a_file_is_claimed_by_moving_it_rather_than_copying_it() {
        let (temp, _temp) = sandboxed_temp("claim-a-file");

        let extracted = temp.join("7zE00E731CF");
        std::fs::create_dir_all(&extracted).unwrap();
        let one = extracted.join("one.txt");
        std::fs::write(&one, b"one").unwrap();

        let claimed = claim(vec![one.clone()]);
        assert_eq!(claimed.len(), 1);
        assert!(claimed[0].parent().is_some_and(is_staging));
        assert_eq!(std::fs::read_to_string(&claimed[0]).unwrap(), "one");
        assert!(
            !one.exists(),
            "the claim left a copy behind, so it cost the archive's weight rather than a rename"
        );
    }

    /// A file open with less sharing than that: its *own* rename is refused too, and the hard link
    /// is what gets it across. Which is the rung below the recursion and the reason there is one —
    /// a second name for the same data, so the archiver deleting its name leaves the data under
    /// ours, which is the half of it the second assertion is about.
    #[cfg(windows)]
    #[test]
    fn a_file_that_cannot_be_renamed_is_claimed_by_a_second_name_for_it() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let (temp, _temp) = sandboxed_temp("claim-an-unmovable-file");
        let master = temp.join("7zE8C57170C").join("master");
        std::fs::create_dir_all(&master).unwrap();
        let scanned = master.join("scanned.dll");
        std::fs::write(&scanned, b"bytes").unwrap();

        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(&scanned)
            .unwrap();
        assert!(
            std::fs::rename(&scanned, temp.join("moved")).is_err(),
            "the file's own rename was allowed, so this never reaches the hard link"
        );

        let claimed = claim(vec![master.clone()]);
        assert_eq!(claimed.len(), 1);
        let landed = &claimed[0];
        assert_eq!(
            std::fs::read_to_string(landed.join("scanned.dll")).unwrap(),
            "bytes",
            "the file nothing would move did not come across"
        );

        drop(held);
        crate::sandbox::remove_file(&scanned);
        assert_eq!(
            std::fs::read_to_string(landed.join("scanned.dll")).unwrap(),
            "bytes",
            "the archiver's own delete took the data with it"
        );
    }

    /// The narrow test that keeps a drag from Explorer out of all of this: a claim applied to the
    /// user's own files would move them into a scratch folder. And with nothing claimed there is
    /// nothing to stage, which is the litter this leaves in `%TEMP%` otherwise.
    #[test]
    fn files_that_are_not_a_source_s_temporary_are_left_where_they_are() {
        let (temp, _temp) = sandboxed_temp("claim-leaves-real-files");
        let theirs = temp.with_file_name("documents");
        std::fs::create_dir_all(&theirs).unwrap();

        let one = theirs.join("one.txt");
        std::fs::write(&one, b"one").unwrap();
        assert_eq!(claim(vec![one.clone()]), vec![one.clone()]);
        assert!(
            one.exists(),
            "a file that was not a materialisation was moved into a scratch folder anyway"
        );
        assert_eq!(
            std::fs::read_dir(&temp).unwrap().count(),
            0,
            "a staging directory was made for a drop that had nothing to claim"
        );
    }

    /// **A drag from another program has to wake the window, or its drop highlight never appears.**
    ///
    /// The highlight is drawn from `App::drop_hover`, which is refreshed in
    /// [`crate::app::App::collect_drops`] — inside a frame. A drag this window started keeps frames
    /// coming, because [`crate::app::App::pump_drag`] asks for one every frame for the length of it.
    /// A drag out of 7-Zip, or out of Explorer, has no such thing: OLE holds the pointer, so winit
    /// sees no mouse move, and `DragOver` wrote where the drag was to a block nothing came to read.
    /// The row lit up for a drag between two panes and stayed dark for a drag out of an archive.
    ///
    /// Driven through the real `IDropTarget` rather than through a stand-in, because the whole
    /// question is what *that object* does when Windows calls it: an `hwnd` of zero is the one
    /// concession, and it is one the point conversion already makes room for.
    #[cfg(windows)]
    #[test]
    fn a_drag_from_another_program_wakes_the_window() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{IDropTarget, DROPEFFECT_COPY};
        use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

        let shared = Arc::new(Mutex::new(Shared::default()));
        shared.lock().unwrap().targets = Targets {
            zones: vec![((0, 0, 100, 100), Onto::Folder(PathBuf::from(r"C:\into")))],
        };
        let ctx = egui::Context::default();
        let target: IDropTarget = win::Target::new(shared.clone(), 0, ctx.clone()).into();

        // Frames first, so that what is asserted below is this drag's doing and not the repaint
        // every freshly built context wants for its first paint. Bounded rather than `while`, so a
        // context that went on wanting one fails the suite instead of hanging it — and stated,
        // because a context still asking would make the assertion at the end pass for the wrong
        // reason and say nothing at all.
        for _ in 0..8 {
            if !ctx.has_requested_repaint() {
                break;
            }
            let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        }
        assert!(
            !ctx.has_requested_repaint(),
            "the context never settled, so a wake could not be told from what was already pending"
        );

        let mut effect = DROPEFFECT_COPY;
        // SAFETY: an out-parameter this call owns for its duration, and no data object — `DragOver`
        // is the callback that carries none, which is why the highlight can be tested without one.
        unsafe {
            target
                .DragOver(MODIFIERKEYS_FLAGS(0), POINTL { x: 10, y: 20 }, &mut effect)
                .expect("DragOver refused");
        }

        assert_eq!(
            shared.lock().unwrap().hovering,
            Some((10, 20)),
            "the drag was not recorded, so there would be nothing to highlight"
        );
        assert!(
            ctx.has_requested_repaint(),
            "the window was not woken, so the frame that draws the highlight never runs"
        );
        assert_eq!(effect, DROPEFFECT_COPY, "a folder should take a copy");
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
