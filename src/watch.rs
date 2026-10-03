//! Notice when a folder changes on disk, so a listing is never stale.
//!
//! Without this, a listing is only as fresh as the last thing this program did to it. Anything
//! anybody *else* did went unseen: a file dragged out to Explorer stayed on screen because
//! Explorer's move finishes after our drag does and there was nothing to wait on; a build
//! writing into the folder you were watching showed the folder as it had been; a file deleted
//! from a terminal left a row that opened nothing. Worse than looking wrong, it made the *next*
//! gesture fail — dragging a row that no longer names a file cannot start a drag, so the window
//! appeared to have stopped responding.
//!
//! # One thread, not one per folder
//!
//! Navigating changes the watched set constantly — every folder you open and leave — so a thread
//! per folder would mean a thread spawned and joined per click. Instead there is one thread
//! holding a directory handle and an event per watched folder, parked in
//! `WaitForMultipleObjects` over all of them plus one more event the UI thread signals when the
//! set changes. Nothing spins and nothing polls: the thread wakes when a folder changes or when
//! the set does.
//!
//! # What it does not do
//!
//! It does not read the notifications. `ReadDirectoryChangesW` will say which file changed and
//! how, and none of that is worth having here: the answer to any of it is to re-read the folder,
//! which is one scan either way. Not parsing the buffer also removes the two ways this API is
//! usually got wrong — the alignment of `FILE_NOTIFY_INFORMATION` and the overflow case where
//! the buffer was too small and the contents are gone but the fact of the change is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// How long to let a burst of changes settle before re-reading, in seconds.
///
/// A single copy fires several notifications — the file appears, its size changes, its timestamp
/// changes — and one folder scan answers all of them. Long enough to collapse a burst, short
/// enough that a file appearing looks immediate.
const SETTLE: f64 = 0.15;

/// The most folders one thread can watch, since it waits on them all at once.
///
/// `MAXIMUM_WAIT_OBJECTS` is 64 and one of those is the wake event. Reaching this needs 63 tabs
/// open at once; the ones past it simply are not watched, which is the behaviour this had for
/// every folder before.
#[cfg(windows)]
const MAX_WATCHED: usize = 63;

#[derive(Default)]
struct Shared {
    /// The folders the UI wants watched, replaced wholesale by [`Watch::keep`].
    wanted: Vec<PathBuf>,
    /// Folders seen to change, waiting to be asked for.
    changed: Vec<PathBuf>,
    quit: bool,
}

/// Watches the folders on screen and reports the ones that change.
pub struct Watch {
    shared: Arc<Mutex<Shared>>,
    /// The last set handed to [`Watch::keep`], so an unchanged frame takes no lock.
    mine: Vec<PathBuf>,
    /// Changes waiting out their settle window, and when each is due.
    pending: HashMap<PathBuf, f64>,
    /// The event that wakes the thread, as an integer because a `HANDLE` is not `Send`.
    #[cfg(windows)]
    wake: isize,
    #[cfg(windows)]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    pub fn new(ctx: &egui::Context) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        #[cfg(windows)]
        {
            let (wake, thread) = win::start(shared.clone(), ctx.clone());
            Self {
                shared,
                mine: Vec::new(),
                pending: HashMap::new(),
                wake,
                thread,
            }
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            Self {
                shared,
                mine: Vec::new(),
                pending: HashMap::new(),
            }
        }
    }

    /// Watch exactly these folders and no others.
    ///
    /// Called every frame with whatever is on screen, so it has to be cheap when nothing has
    /// moved: the set is compared against a local copy first and the lock is only taken when it
    /// actually differs.
    pub fn keep(&mut self, folders: &[PathBuf]) {
        let mut wanted: Vec<PathBuf> = folders
            .iter()
            .filter(|path| !path.as_os_str().is_empty())
            .cloned()
            .collect();
        wanted.sort();
        wanted.dedup();
        if wanted == self.mine {
            return;
        }
        self.mine = wanted.clone();
        if let Ok(mut shared) = self.shared.lock() {
            shared.wanted = wanted;
        }
        #[cfg(windows)]
        win::signal(self.wake);
    }

    /// Folders that have changed and have settled, so are due a re-read.
    ///
    /// `now` is egui's own clock, which is the one the caller can wake itself against.
    pub fn changed(&mut self, now: f64) -> Vec<PathBuf> {
        if let Ok(mut shared) = self.shared.lock() {
            for path in shared.changed.drain(..) {
                // First notification of a burst starts the clock; the rest ride along with it.
                self.pending.entry(path).or_insert(now + SETTLE);
            }
        }
        if self.pending.is_empty() {
            return Vec::new();
        }
        let due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, &at)| at <= now)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &due {
            self.pending.remove(path);
        }
        due
    }

    /// Whether anything is still waiting out its settle window, and so whether the caller has to
    /// come back for it.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if let Ok(mut shared) = self.shared.lock() {
                shared.quit = true;
            }
            win::signal(self.wake);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            win::close(self.wake);
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    /// Everything worth being told about a folder's contents.
    ///
    /// Not `FILE_NOTIFY_CHANGE_LAST_ACCESS`: reading a file in the folder would then re-read the
    /// folder, and merely showing the listing reads it.
    const FILTER: windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE =
        windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE(
            FILE_NOTIFY_CHANGE_FILE_NAME.0
                | FILE_NOTIFY_CHANGE_DIR_NAME.0
                | FILE_NOTIFY_CHANGE_ATTRIBUTES.0
                | FILE_NOTIFY_CHANGE_SIZE.0
                | FILE_NOTIFY_CHANGE_LAST_WRITE.0,
        );

    /// One watched folder: the handle, the event it completes on, and the read in flight.
    struct Watched {
        path: PathBuf,
        dir: HANDLE,
        event: HANDLE,
        /// Boxed because the kernel keeps the address until the read completes, and the struct
        /// itself moves when it goes into the vector.
        overlapped: Box<OVERLAPPED>,
        /// `u32` so the buffer is `DWORD`-aligned, which the API requires even though nothing
        /// here ever reads it.
        buffer: Box<[u32; 256]>,
    }

    impl Watched {
        fn open(path: &Path) -> Option<Self> {
            let wide = crate::shell::wide(path);
            // SAFETY: a null-terminated path that outlives the call. `FILE_SHARE_DELETE` is not
            // optional: without it this handle would stop the folder from being deleted or
            // renamed, so watching a folder would break the very operations it is watching for.
            let dir = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    FILE_LIST_DIRECTORY.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    None,
                )
                .ok()?
            };
            if dir == INVALID_HANDLE_VALUE {
                return None;
            }
            // Auto-reset: the wait that observes it clears it, so re-arming needs nothing.
            // SAFETY: an unnamed event, released in `Drop`.
            let event = match unsafe { CreateEventW(None, false, false, PCWSTR::null()) } {
                Ok(event) => event,
                Err(_) => {
                    // SAFETY: opened just above and not handed anywhere.
                    let _ = unsafe { CloseHandle(dir) };
                    return None;
                }
            };
            let mut overlapped = Box::new(OVERLAPPED::default());
            overlapped.hEvent = event;
            let mut watched = Self {
                path: path.to_path_buf(),
                dir,
                event,
                overlapped,
                buffer: Box::new([0u32; 256]),
            };
            watched.arm().then_some(watched)
        }

        /// Ask for the next change. False if the folder has gone.
        fn arm(&mut self) -> bool {
            let len = std::mem::size_of_val(self.buffer.as_ref()) as u32;
            // SAFETY: the buffer and the `OVERLAPPED` are boxed, so their addresses stand still
            // until the read completes — which is what makes this sound at all. The kernel
            // writes into the buffer and nothing here reads it while the read is in flight.
            unsafe {
                ReadDirectoryChangesW(
                    self.dir,
                    self.buffer.as_mut().as_mut_ptr().cast(),
                    len,
                    false,
                    FILTER,
                    None,
                    Some(self.overlapped.as_mut() as *mut OVERLAPPED),
                    None,
                )
                .is_ok()
            }
        }

        /// Complete the read that just signalled, and ask for the next.
        fn consume(&mut self) -> bool {
            let mut written = 0u32;
            // SAFETY: the read this completes was issued by `arm` on this same `OVERLAPPED`.
            // The byte count is ignored: zero means the buffer overflowed, which is still a
            // change, and any other value describes files this does not need to know about.
            unsafe {
                let _ = GetOverlappedResult(self.dir, self.overlapped.as_ref(), &mut written, false);
            }
            self.arm()
        }
    }

    impl std::ops::Drop for Watched {
        fn drop(&mut self) {
            // SAFETY: cancel the read *before* closing anything, or the kernel would complete
            // into a buffer this is about to free.
            unsafe {
                let _ = CancelIoEx(self.dir, Some(self.overlapped.as_ref()));
                let _ = CloseHandle(self.dir);
                let _ = CloseHandle(self.event);
            };
        }
    }

    /// The wake event and the thread waiting on it.
    pub fn start(
        shared: Arc<Mutex<Shared>>,
        ctx: egui::Context,
    ) -> (isize, Option<std::thread::JoinHandle<()>>) {
        // Manual reset, so a signal is not lost if the thread is between waits.
        // SAFETY: an unnamed event, closed by `Watch::drop`.
        let Ok(wake) = (unsafe { CreateEventW(None, true, false, PCWSTR::null()) }) else {
            return (0, None);
        };
        // Carried across as an integer: a `HANDLE` is a raw pointer and so not `Send`, which
        // is a rule about the type rather than about this handle — a kernel object is shared
        // between threads by design, and this one is closed only after the thread is joined.
        let raw = wake.0 as isize;
        let thread = std::thread::Builder::new()
            .name("folder-watch".to_owned())
            .spawn(move || run(shared, raw, ctx))
            .ok();
        (raw, thread)
    }

    pub fn signal(wake: isize) {
        if wake == 0 {
            return;
        }
        // SAFETY: the handle belongs to `Watch`, which outlives every call to this.
        let _ = unsafe { SetEvent(HANDLE(wake as *mut std::ffi::c_void)) };
    }

    pub fn close(wake: isize) {
        if wake == 0 {
            return;
        }
        // SAFETY: closed once, from `Watch::drop`, after the thread has been joined.
        let _ = unsafe { CloseHandle(HANDLE(wake as *mut std::ffi::c_void)) };
    }

    fn run(shared: Arc<Mutex<Shared>>, wake: isize, ctx: egui::Context) {
        use windows::Win32::System::Threading::{ResetEvent, INFINITE};

        let wake = HANDLE(wake as *mut std::ffi::c_void);

        let mut watches: Vec<Watched> = Vec::new();
        loop {
            // ---- Reconcile the set -------------------------------------
            let wanted = {
                let Ok(shared) = shared.lock() else { return };
                if shared.quit {
                    return;
                }
                shared.wanted.clone()
            };
            // SAFETY: manual-reset, so it is cleared here rather than by the wait — before the
            // set is read, so a change arriving during reconciliation wakes the next wait
            // instead of being dropped.
            let _ = unsafe { ResetEvent(wake) };
            // Dropping a `Watched` cancels its read and closes its handles.
            watches.retain(|w| wanted.contains(&w.path));
            for path in wanted.iter().take(MAX_WATCHED) {
                if !watches.iter().any(|w| &w.path == path) {
                    if let Some(watched) = Watched::open(path) {
                        watches.push(watched);
                    }
                }
            }

            // ---- Wait --------------------------------------------------
            let mut handles = Vec::with_capacity(watches.len() + 1);
            handles.push(wake);
            handles.extend(watches.iter().map(|w| w.event));
            // SAFETY: every handle is live — the wake event belongs to `Watch`, which joins this
            // thread before closing it, and the rest are owned by `watches`.
            let got = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
            if got == WAIT_FAILED {
                // Nothing to retry against, and spinning on it would burn a core.
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
            let index = got.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            if index == 0 || index > watches.len() {
                // The wake event, or something unexpected: reconcile and wait again.
                continue;
            }

            // ---- Report ------------------------------------------------
            let watched = &mut watches[index - 1];
            let path = watched.path.clone();
            if !watched.consume() {
                // The folder has gone. Its own disappearance is a change in its parent, which
                // is watched separately if it is on screen.
                watches.remove(index - 1);
            }
            if let Ok(mut shared) = shared.lock() {
                if shared.quit {
                    return;
                }
                if !shared.changed.contains(&path) {
                    shared.changed.push(path);
                }
            }
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change waits out its settle window, then comes back exactly once.
    ///
    /// The window is what keeps one copy from becoming five folder scans: writing a file fires
    /// several notifications — the name, then the size, then the timestamp — and one scan answers
    /// all of them.
    #[test]
    fn a_burst_of_changes_becomes_one_re_read() {
        let ctx = egui::Context::default();
        let mut watch = Watch::new(&ctx);
        let folder = PathBuf::from(r"C:\Temp");

        // Three notifications, as a single file being written would produce.
        if let Ok(mut shared) = watch.shared.lock() {
            shared.changed.push(folder.clone());
            shared.changed.push(folder.clone());
            shared.changed.push(folder.clone());
        }

        assert!(
            watch.changed(0.0).is_empty(),
            "nothing is due until the burst has settled"
        );
        assert!(watch.waiting(), "and the caller is told to come back");
        assert_eq!(
            watch.changed(SETTLE + 0.001),
            vec![folder],
            "one re-read for the burst, not three"
        );
        assert!(!watch.waiting());
        assert!(
            watch.changed(10.0).is_empty(),
            "and it does not come back again"
        );
    }

    /// The empty path is This PC, which is a list of volumes rather than a folder.
    #[test]
    fn nothing_watches_this_pc() {
        let ctx = egui::Context::default();
        let mut watch = Watch::new(&ctx);
        watch.keep(&[PathBuf::new(), PathBuf::from(r"C:\Temp")]);
        assert_eq!(watch.mine, vec![PathBuf::from(r"C:\Temp")]);
    }

    /// Two panes on one folder ask for it once.
    #[test]
    fn the_same_folder_twice_is_watched_once() {
        let ctx = egui::Context::default();
        let mut watch = Watch::new(&ctx);
        let one = PathBuf::from(r"C:\Temp");
        watch.keep(&[one.clone(), one.clone()]);
        assert_eq!(watch.mine, vec![one]);
    }
}
