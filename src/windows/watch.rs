//! `ReadDirectoryChangesW`: hearing about a folder changing without polling it.
//!
//! The Windows half of [`crate::watch`].

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
