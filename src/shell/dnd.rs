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
